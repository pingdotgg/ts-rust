//! Go `internal/project/ata/ata.go`.
//!
//! PORT: one thread (see `project/dirty/interfaces.rs`). `sync.Once` is an
//! `OnceState` cell, `atomic.Int32` a `Cell<i32>`, and
//! `collections.SyncMap` a `RefCell` map. `TypingsInstaller` methods take
//! `&self` like the Go pointer receiver. Go `logging.Logger` parameters are
//! `&dyn logging::Logger`, so a caller can pass `&Option<Rc<dyn Logger>>`
//! or `&Option<Rc<LogTree>>` (both implement the trait, as Go's nil-safe
//! loggers do). Go `vfs.FS` parameters are `&dyn vfs::Fs`.
//!
//! PORT: Go runs each ATA request on a goroutine that blocks while npm runs.
//! The port runs the request on the dispatch thread. Each Go function that
//! reaches npm takes a callback (`Then`) that gets its result: the rest of
//! the goroutine after that call. A request that waits is a `Waiting`
//! callback; it costs nothing until the event it waits for resumes it, as a
//! blocked goroutine does:
//! - npm: an executor that gives `npm_install_func` (the LSP server) runs
//!   npm on a helper thread, which posts the result to the dispatch thread
//!   once (`gostd::local::post_later`). Without it (the test mock) npm runs
//!   at once and the whole request ends before the first call returns.
//! - `initOnce.Do`: the request that runs `init` resumes the others when it
//!   is done (`init_waiters`).
//! - the `concurrencySemaphore` send: a body that ends gives its slot to the
//!   oldest waiting body, as a Go channel receive does (`semaphore_waiters`).
//!
//! The port has no `async` (lust, the dev compiler, does not support it).

use crate::project::ata::prelude::*;

use crate::frontend::core_ext::TypeAcquisition;
use crate::frontend::json::{self, JsonDecoder, JsonError, UnmarshalerFrom, json_unmarshal_decode};
use crate::frontend::json_ext::{LspAny, unmarshal_struct_fields};
use crate::frontend::{module, semver, tspath, vfs};
use crate::gostd::{self, Context, GoError};
use crate::project::logging::{self, Logger as _};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

// Go: project/ata/ata.go:21 TypingsInfo
// PORT: the Go pointers are shared and read only, so they are `Rc`; nil is
// `None`. `UnresolvedImports *collections.Set[string]` is an
// `FxHashSet`.
#[derive(Clone, Debug, Default)]
pub struct TypingsInfo {
    pub type_acquisition: Option<Rc<TypeAcquisition>>,
    pub compiler_options: Option<Rc<CompilerOptions>>,
    pub unresolved_imports: Option<Rc<FxHashSet<String>>>,
}

impl TypingsInfo {
    // Go: project/ata/ata.go:27 Equals
    pub fn equals(&self, other: &TypingsInfo) -> bool {
        TypeAcquisition::equals(
            self.type_acquisition.as_deref(),
            other.type_acquisition.as_deref(),
        ) && self
            .compiler_options
            .as_deref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .get_allow_js()
            == other
                .compiler_options
                .as_deref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .get_allow_js()
            // Go: collections.Set.Equals (pointer equality, then nil checks,
            // then maps.Equal).
            && match (&self.unresolved_imports, &other.unresolved_imports) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b) || **a == **b,
                _ => false,
            }
    }
}

// Go: project/ata/ata.go:33 CachedTyping
// PORT: Go `Version *semver.Version` is never nil here (every store takes
// the address of a parsed version), so the field is the value.
#[derive(Clone, Debug)]
pub struct CachedTyping {
    pub typings_location: String,
    pub version: semver::Version,
}

// Go: project/ata/ata.go:38 TypingsInstallerOptions
#[derive(Clone, Debug, Default)]
pub struct TypingsInstallerOptions {
    pub typings_location: String,
    pub throttle_limit: i32,
}

// Go: project/ata/ata.go:43 NpmExecutor
pub trait NpmExecutor {
    // PORT: Go returns `([]byte, error)` and reads the output when the error
    // is non-nil (installWorker logs it), so the result is a pair, not a
    // `Result`. `None` is Go's nil error. ts#64544 adds `ctx`.
    fn npm_install(&self, ctx: &Context, cwd: &str, args: &[String]) -> (Vec<u8>, Option<GoError>);

    /// PORT: no Go counterpart. A `Send` form of `npm_install` that ATA runs
    /// on a helper thread (`TypingsInstaller::npm_install`), or `None` to run
    /// `npm_install` on the calling thread.
    fn npm_install_func(&self) -> Option<NpmInstallFunc> {
        None
    }
}

/// PORT: the `Send` form of `NpmExecutor::npm_install`.
pub type NpmInstallFunc =
    Arc<dyn Fn(&Context, &str, &[String]) -> (Vec<u8>, Option<GoError>) + Send + Sync>;

/// PORT: Go `sync.Once` of `TypingsInstaller.initOnce`. `Running` while the
/// first `init` waits for npm; Go blocks the other callers of `Do` until the
/// body ends, and the port's callers wait for `Done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnceState {
    NotStarted,
    Running,
    Done,
}

// Go: project/ata/ata.go:47 TypingsInstallerHost (at 673a5f17d713; removed by ts#64159)
// PORT: Go interfaces are structural, so every type that implements both
// traits is a host (blanket impl). `Rc<dyn TypingsInstallerHost>` upcasts to
// `Rc<dyn module::ResolutionHost>` for `module::new_resolver`.
pub trait TypingsInstallerHost: NpmExecutor + module::ResolutionHost {}

impl<T: NpmExecutor + module::ResolutionHost> TypingsInstallerHost for T {}

// Go: project/ata/ata.go:47 TypingsInstaller
// PORT: `packageNameToTypingLocation` is ranged in DiscoverTypings, so it is
// an `IndexMap` (insertion order; Go map order is random). A
// `typesRegistry` entry is `Option`: Go keeps a JSON `null` entry as a nil
// map, and DiscoverTypings tests `registryEntry != nil`. The Go
// `concurrencySemaphore chan struct{}` is the `sync_channel` pair
// (PORTING "Go runtime"), and `semaphore_waiters` holds the bodies that
// block in a send on it.
pub struct TypingsInstaller {
    pub typings_location: String,
    pub host: Rc<dyn TypingsInstallerHost>,

    pub init_once: Cell<OnceState>,
    /// PORT: the requests that wait in `initOnce.Do` while `init` runs.
    pub init_waiters: RefCell<Vec<Waiting<()>>>,

    pub package_name_to_typing_location: RefCell<IndexMap<String, Rc<CachedTyping>>>,
    pub missing_typings_set: RefCell<FxHashMap<String, bool>>,

    pub types_registry: RefCell<FxHashMap<String, Option<FxHashMap<String, String>>>>,

    pub install_run_count: Cell<i32>,
    pub concurrency_semaphore: (SyncSender<()>, Receiver<()>),
    pub semaphore_waiters: SemaphoreWaiters,
}

// Go: project/ata/ata.go:63 resolutionHost (ts#64159)
// The module resolution host of ATA: the installer's file system, with the
// typings location as the current directory. N resolved with the session as
// the host, whose current directory is the workspace's.
// PORT: Go `TypingsInstaller` keeps `fs` and `npmExecutor` instead of the
// host (ts#64159 drops `TypingsInstallerHost`); the port keeps the host for
// both and gives the resolver this host.
struct ResolutionHost {
    host: Rc<dyn TypingsInstallerHost>,
    current_directory: String,
}

impl module::ResolutionHost for ResolutionHost {
    // Go: project/ata/ata.go:68 resolutionHost.FS
    fn fs(&self) -> &dyn vfs::Fs {
        self.host.fs()
    }

    // Go: project/ata/ata.go:72 resolutionHost.GetCurrentDirectory
    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

impl TypingsInstaller {
    /// Go `&resolutionHost{fs: ti.fs, currentDirectory: ti.typingsLocation}`.
    fn resolution_host(&self) -> Rc<dyn module::ResolutionHost> {
        Rc::new(ResolutionHost {
            host: self.host.clone(),
            current_directory: self.typings_location.clone(),
        })
    }
}

// Go: project/ata/ata.go:76 NewTypingsInstaller
pub fn new_typings_installer(
    options: &TypingsInstallerOptions,
    host: Rc<dyn TypingsInstallerHost>,
) -> Rc<TypingsInstaller> {
    Rc::new(TypingsInstaller {
        typings_location: options.typings_location.clone(),
        host,
        init_once: Cell::new(OnceState::NotStarted),
        init_waiters: RefCell::new(Vec::new()),
        package_name_to_typing_location: RefCell::new(IndexMap::default()),
        missing_typings_set: RefCell::new(FxHashMap::default()),
        types_registry: RefCell::new(FxHashMap::default()),
        install_run_count: Cell::new(0),
        concurrency_semaphore: std::sync::mpsc::sync_channel::<()>(options.throttle_limit as usize),
        semaphore_waiters: SemaphoreWaiters::default(),
    })
}

// Go: project/ata/ata.go:85 ProjectID (ts#64319)
// PORT: Go `interface { fmt.Stringer }`.
pub trait ProjectID {
    fn string(&self) -> String;
}

impl TypingsInstaller {
    // Go: project/ata/ata.go:89 IsKnownTypesPackageName
    // PORT: `fs` and `logger` are owned: `init` keeps them while npm runs.
    pub fn is_known_types_package_name(
        self: &Rc<Self>,
        project_id: &dyn ProjectID,
        name: &str,
        fs: Rc<dyn vfs::Fs>,
        logger: Option<Rc<dyn logging::Logger>>,
    ) -> bool {
        // We want to avoid looking this up in the registry as that is expensive. So first check that it's actually an NPM package.
        let (validation_result, _, _) = validate_package_name(name);
        if validation_result != NAME_OK {
            return false;
        }
        // Strada did this lazily - is that needed here to not waiting on and returning false on first request
        // PORT: no Go caller at pin N. Go blocks in `initOnce.Do` while an
        // ATA request runs init; `block_on` runs the dispatch thread's ready
        // work until that request is done.
        block_on(|then| {
            self.init(
                &gostd::context::background(),
                &project_id.string(),
                fs,
                logger,
                then,
            )
        });
        self.types_registry.borrow().contains_key(name)
    }
}

// Go: project/ata/ata.go:102 tsVersionToUse
// !!! sheetal currently we use latest instead of core.VersionMajorMinor()
pub const TS_VERSION_TO_USE: &str = "latest";

// Go: project/ata/ata.go:104 TypingsInstallRequest
// PORT: Go `TypingsInfo *TypingsInfo` is never nil (the session passes the
// address of a local), so it is an `Rc`. `GetScriptKind` is a stored func.
#[derive(Clone)]
pub struct TypingsInstallRequest {
    // ts#64319. PORT: Go `ProjectID` interface value.
    pub project_id: Rc<dyn ProjectID>,
    pub typings_info: Rc<TypingsInfo>,
    pub file_names: Vec<String>,
    pub project_root_path: String,
    pub compiler_options: Option<Rc<CompilerOptions>>,
    pub current_directory: String,
    pub get_script_kind: Rc<dyn Fn(&str) -> ScriptKind>,
    pub fs: Rc<dyn vfs::Fs>,
    pub logger: Option<Rc<dyn logging::Logger>>,
}

// Go: project/ata/ata.go:113 TypingsInstallResult
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypingsInstallResult {
    pub typings_files: Vec<String>,
    pub files_to_watch: Vec<String>,
}

impl TypingsInstaller {
    // Go: project/ata/ata.go:118 InstallTypings
    // PORT: Go also has the unexported `installTypings` on this type, so the
    // exported Go `InstallTypings` is `install_typings_exported` (PORTING
    // "Names"). Go returns `(*TypingsInstallResult, error)` with a nil
    // result on error, so the result is a `Result`, which `then` gets (see
    // the file comment).
    pub fn install_typings_exported(
        self: &Rc<Self>,
        ctx: &Context,
        request: TypingsInstallRequest,
        then: Then<Result<TypingsInstallResult, GoError>>,
    ) {
        let (logger, project_id) = (request.logger.clone(), request.project_id.clone());
        self.discover_and_install_typings(
            ctx,
            request,
            Box::new(move |mut result: Result<TypingsInstallResult, GoError>| {
                if let Ok(result) = &mut result {
                    result.typings_files.sort();
                    result.files_to_watch.sort();
                    logger.log(&format!(
                        "ATA:: Got install request for: {}",
                        project_id.string()
                    ));
                }
                then(result)
            }),
        );
    }

    // Go: project/ata/ata.go:128 discoverAndInstallTypings
    pub fn discover_and_install_typings(
        self: &Rc<Self>,
        ctx: &Context,
        request: TypingsInstallRequest,
        then: Then<Result<TypingsInstallResult, GoError>>,
    ) {
        let (ti, after_ctx) = (self.clone(), ctx.clone());
        let (fs, logger) = (request.fs.clone(), request.logger.clone());
        self.init(
            ctx,
            &request.project_id.string(),
            fs,
            logger,
            Box::new(move |()| {
                ti.discover_and_install_typings_after_init(&after_ctx, &request, then)
            }),
        );
    }

    /// PORT: the rest of Go `discoverAndInstallTypings` after `initOnce.Do`.
    fn discover_and_install_typings_after_init(
        self: &Rc<Self>,
        ctx: &Context,
        request: &TypingsInstallRequest,
        then: Then<Result<TypingsInstallResult, GoError>>,
    ) {
        let (cached_typing_paths, new_typing_names, files_to_watch) = discover_typings(
            &*request.fs,
            &request.logger,
            &request.typings_info,
            &request.file_names,
            &request.project_root_path,
            &self.package_name_to_typing_location,
            &self.types_registry.borrow(),
        );

        let request_id = self.install_run_count.get() + 1;
        self.install_run_count.set(request_id);
        // install typings
        if !new_typing_names.is_empty() {
            let filtered_typings = self.filter_typings(&request.logger, &new_typing_names);
            if !filtered_typings.is_empty() {
                return self.install_typings(
                    ctx,
                    request_id,
                    cached_typing_paths,
                    filtered_typings,
                    request.logger.clone(),
                    Box::new(move |typings_files: Result<Vec<String>, GoError>| {
                        then(typings_files.map(|typings_files| TypingsInstallResult {
                            typings_files,
                            files_to_watch,
                        }));
                    }),
                );
            }
            request.logger.log(
                "ATA:: All typings are known to be missing or invalid - no need to install more typings",
            );
        } else {
            request
                .logger
                .log("ATA:: No new typings were requested as a result of typings discovery");
        }

        then(Ok(TypingsInstallResult {
            typings_files: cached_typing_paths,
            files_to_watch,
        }));
        // !!! sheetal events to send
        // this.event(response, "setTypings");
    }

    // Go: project/ata/ata.go:168 installTypings
    // ts#64319: no project ID or typings info parameters.
    pub fn install_typings(
        self: &Rc<Self>,
        ctx: &Context,
        request_id: i32,
        currently_cached_typings: Vec<String>,
        filtered_typings: Vec<String>,
        logger: Option<Rc<dyn logging::Logger>>,
        then: Then<Result<Vec<String>, GoError>>,
    ) {
        // !!! sheetal events to send
        // send progress event
        // this.sendResponse({
        // 	kind: EventBeginInstallTypes,
        // 	eventId: requestId,
        // 	typingsInstallerVersion: version,
        // 	projectName: req.projectName,
        // } as BeginInstallTypes);

        // const body: protocol.BeginInstallTypesEventBody = {
        // 	eventId: response.eventId,
        // 	packages: response.packagesToInstall,
        // };
        // const eventName: protocol.BeginInstallTypesEventName = "beginInstallTypes";
        // this.event(body, eventName);

        let mut scoped_typings: Vec<String> = vec![String::new(); filtered_typings.len()];
        for (i, package_name) in filtered_typings.iter().enumerate() {
            scoped_typings[i] = format!("@types/{package_name}@{TS_VERSION_TO_USE}"); // @tscore.VersionMajorMinor) // This is normally @tsVersionMajorMinor but for now lets use latest
        }

        let ti = self.clone();
        self.install_worker(
            ctx,
            request_id,
            scoped_typings,
            logger.clone(),
            Box::new(move |(package_names, ok): (Vec<String>, bool)| {
                then(ti.install_typings_done(
                    &package_names,
                    ok,
                    &currently_cached_typings,
                    &filtered_typings,
                    &logger,
                ));
            }),
        );
    }

    /// PORT: the rest of Go `installTypings` after `installWorker` returns.
    fn install_typings_done(
        &self,
        package_names: &[String],
        ok: bool,
        currently_cached_typings: &[String],
        filtered_typings: &[String],
        logger: &dyn logging::Logger,
    ) -> Result<Vec<String>, GoError> {
        if ok {
            // PORT: Go `%v` of a slice; log text is not compared.
            logger.log(&format!("ATA:: Installed typings {package_names:?}"));
            let mut installed_typing_files: Vec<String> = Vec::new();
            // ts#64159
            let host = self.resolution_host();
            // ts#64299
            let resolver = module::new_resolver(module::ResolverOptions {
                host: Some(host),
                compiler_options: Some(Rc::new(CompilerOptions {
                    module_resolution: ModuleResolutionKind::NODE_NEXT,
                    ..CompilerOptions::default()
                })),
                ..Default::default()
            });
            for package_name in filtered_typings {
                let typing_file = self.typing_to_file_name(&resolver, package_name);
                if typing_file.is_empty() {
                    logger.log(&format!(
                        "ATA:: Failed to find typing file for package '{package_name}'"
                    ));
                    self.missing_typings_set
                        .borrow_mut()
                        .insert(package_name.clone(), true);
                    continue;
                }

                // packageName is guaranteed to exist in typesRegistry by filterTypings
                let types_registry = self.types_registry.borrow();
                let dist_tags = types_registry.get(package_name).and_then(Option::as_ref);
                let (mut use_version, ok) = match dist_tags
                    .and_then(|d| d.get(&format!("ts{}", crate::core::version_major_minor())))
                {
                    Some(v) => (v.clone(), true),
                    None => (String::new(), false),
                };
                if !ok {
                    use_version = dist_tags
                        .and_then(|d| d.get("latest"))
                        .cloned()
                        .unwrap_or_default();
                }
                let new_version = semver::must_parse_version(&use_version);
                let new_typing = Rc::new(CachedTyping {
                    typings_location: typing_file.clone(),
                    version: new_version,
                });
                self.package_name_to_typing_location
                    .borrow_mut()
                    .insert(package_name.clone(), new_typing);
                installed_typing_files.push(typing_file);
            }
            // PORT: Go `%v` of a slice; log text is not compared.
            logger.log(&format!(
                "ATA:: Installed typing files {installed_typing_files:?}"
            ));

            let mut result = currently_cached_typings.to_vec();
            result.extend(installed_typing_files);
            return Ok(result);
        }

        // DO we really need these events
        // this.event(response, "setTypings");
        // PORT: Go `%v` of a slice; log text is not compared.
        logger.log(&format!(
            "ATA:: install request failed, marking packages as missing to prevent repeated requests: {filtered_typings:?}"
        ));
        for typing in filtered_typings {
            self.missing_typings_set
                .borrow_mut()
                .insert(typing.clone(), true);
        }

        Err(gostd::errors::new("npm install failed"))

        // !!! sheetal events to send
        // const response: EndInstallTypes = {
        // 	kind: EventEndInstallTypes,
        // 	eventId: requestId,
        // 	projectName: req.projectName,
        // 	packagesToInstall: scopedTypings,
        // 	installSuccess: ok,
        // 	typingsInstallerVersion: version,
        // };
        // this.sendResponse(response);

        // if (this.telemetryEnabled) {
        // 	const body: protocol.TypingsInstalledTelemetryEventBody = {
        // 		telemetryEventName: "typingsInstalled",
        // 		payload: {
        // 			installedPackages: response.packagesToInstall.join(","),
        // 			installSuccess: response.installSuccess,
        // 			typingsInstallerVersion: response.typingsInstallerVersion,
        // 		},
        // 	};
        // 	const eventName: protocol.TelemetryEventName = "telemetry";
        // 	this.event(body, eventName);
        // }

        // const body: protocol.EndInstallTypesEventBody = {
        // 	eventId: response.eventId,
        // 	packages: response.packagesToInstall,
        // 	success: response.installSuccess,
        // };
        // const eventName: protocol.EndInstallTypesEventName = "endInstallTypes";
        // this.event(body, eventName);
    }

    // Go: project/ata/ata.go:268 installWorker
    // ts#64319: no project ID parameter.
    pub fn install_worker(
        self: &Rc<Self>,
        ctx: &Context,
        request_id: i32,
        package_names: Vec<String>,
        logger: Option<Rc<dyn logging::Logger>>,
        then: Then<(Vec<String>, bool)>,
    ) {
        // PORT: Go `%v` of a slice; log text is not compared.
        logger.log(&format!(
            "ATA:: #{request_id} with cwd: {} arguments: {package_names:?}",
            self.typings_location
        ));
        let (ti, npm_ctx, npm_logger) = (self.clone(), ctx.clone(), logger.clone());
        let install_packages: InstallPackages = Rc::new(move |package_names, done| {
            let mut npm_args: Vec<String> = Vec::new();
            npm_args.extend(["install".to_string(), "--ignore-scripts".to_string()]);
            npm_args.extend(package_names.iter().cloned());
            npm_args.extend([
                "--save-dev".to_string(),
                format!("--user-agent=\"typesInstaller/{}\"", crate::core::version()),
            ]);
            let logger = npm_logger.clone();
            ti.npm_install(
                &npm_ctx,
                &ti.typings_location,
                &npm_args,
                Box::new(move |(output, err): NpmResult| {
                    if let Some(err) = err {
                        // PORT: Go `%s` of a `[]byte`.
                        logger.log(&format!(
                            "ATA:: Output is: {}",
                            String::from_utf8_lossy(&output)
                        ));
                        return done(Err(err));
                    }
                    done(Ok(()))
                }),
            );
        });
        let batches = npm_install_batches(&package_names);
        install_npm_packages_async(
            ctx,
            self,
            batches,
            install_packages,
            Box::new(move |err: Result<(), GoError>| {
                logger.log(&format!("TI:: npm install #{request_id} completed"));
                then((package_names, err.is_ok()));
            }),
        );
    }

    /// PORT: Go `ti.host.NpmInstall(cwd, args)`; `then` gets the result.
    /// With the host's `npm_install_func`, npm runs on a helper thread (Go:
    /// the goroutine blocks in `exec.Cmd.Output`). When npm ends, the thread
    /// posts the result to this thread once, and the post resumes the
    /// request. Without it, npm runs at once.
    fn npm_install(&self, ctx: &Context, cwd: &str, args: &[String], then: Then<NpmResult>) {
        let Some(npm_install) = self.host.npm_install_func() else {
            return then(self.host.npm_install(ctx, cwd, args));
        };
        let waiting = Waiting::new(then);
        let (tx, rx) = std::sync::mpsc::channel::<NpmResult>();
        let post = gostd::local::post_later(Box::new(move || {
            // No result: the npm function panicked on the helper thread.
            let result = rx.try_recv().unwrap_or_else(|_| {
                (
                    Vec::new(),
                    Some(gostd::errors::new("npm install: the helper thread failed")),
                )
            });
            waiting.resume(result);
        }));
        let (ctx, cwd, args) = (ctx.clone(), cwd.to_string(), args.to_vec());
        crate::core::GoThread::new()
            .name("ata-npm".to_string())
            .spawn(move || {
                let _ = tx.send(npm_install(&ctx, &cwd, &args));
                post.post();
            });
    }
}

/// PORT: the result of `NpmExecutor::npm_install`.
type NpmResult = (Vec<u8>, Option<GoError>);

// Go: project/ata/ata.go:291 installNpmPackages
// PORT: the Go test entry, with `install_packages` as each body. The bodies
// do not wait, so each one runs at once while it holds a semaphore slot, in
// order, and the result is the first error, as `tg.Wait()` gives. On a full
// semaphore no work of this thread can give the body a slot (Go blocks
// forever): this runs the thread's ready work, as `block_on` does, then
// panics.
pub fn install_npm_packages(
    ctx: &Context,
    package_names: &[String],
    concurrency_semaphore: &(SyncSender<()>, Receiver<()>),
    install_packages: &dyn Fn(&[String]) -> Result<(), GoError>,
) -> Result<(), GoError> {
    // Go: tg := core.NewThrottleGroup(ctx, concurrencySemaphore)
    let _ = ctx;
    let mut first_err: Option<GoError> = None;
    for packages in npm_install_batches(package_names) {
        match concurrency_semaphore.0.try_send(()) {
            Ok(()) => {}
            Err(TrySendError::Full(())) => {
                while gostd::local::wait_pending() {
                    gostd::local::run_pending();
                }
                panic!("ata: a blocking call waits for work that can not finish on this thread")
            }
            Err(TrySendError::Disconnected(())) => panic!("ata: semaphore closed"),
        }
        // Go: defer func() { <-tg.semaphore }(), also on a panic.
        let slot = HeldSlot(&concurrency_semaphore.1);
        let result = install_packages(&packages);
        drop(slot);
        if let Err(err) = result {
            first_err.get_or_insert(err);
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// PORT: a semaphore slot of a body of the test entry `install_npm_packages`;
/// the drop frees it.
struct HeldSlot<'a>(&'a Receiver<()>);

impl Drop for HeldSlot<'_> {
    fn drop(&mut self) {
        let _ = self.0.try_recv();
    }
}

/// PORT: the batches of Go `installNpmPackages`: `package_names` in order,
/// cut so that each npm command line stays under 8000 bytes.
pub fn npm_install_batches(package_names: &[String]) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();

    let mut current_command_start: usize = 0;
    let mut current_command_end: usize = 0;
    let mut current_command_size: usize = 100;

    for package_name in package_names {
        current_command_size = current_command_size + package_name.len() + 1;
        if current_command_size < 8000 {
            current_command_end += 1;
        } else {
            batches.push(package_names[current_command_start..current_command_end].to_vec());
            current_command_start = current_command_end;
            current_command_size = 100 + package_name.len() + 1;
            current_command_end += 1;
        }
    }

    // Handle the final batch
    if current_command_start < package_names.len() {
        batches.push(package_names[current_command_start..current_command_end].to_vec());
    }
    batches
}

/// PORT: the body of a Go `tg.Go` in `installNpmPackages`: it installs one
/// batch of packages and gives the result to its callback.
pub type InstallPackages = Rc<dyn Fn(&[String], Then<Result<(), GoError>>)>;

// Go: project/ata/ata.go:291 installNpmPackages
// PORT: `batches` is `packageNames` cut by `npm_install_batches`. Each
// `tg.Go` body runs while it holds a slot of the semaphore of `ti`
// (`throttled`), so bodies that wait for npm run at the same time. When the
// last body ends, `then` gets the first error, as `tg.Wait()` gives.
pub fn install_npm_packages_async(
    ctx: &Context,
    ti: &Rc<TypingsInstaller>,
    batches: Vec<Vec<String>>,
    install_packages: InstallPackages,
    then: Then<Result<(), GoError>>,
) {
    // Go: tg := core.NewThrottleGroup(ctx, concurrencySemaphore)
    let _ = ctx;
    if batches.is_empty() {
        return then(Ok(()));
    }
    let group = Rc::new(ThrottleGroup {
        running: Cell::new(batches.len()),
        first_err: RefCell::new(None),
        then: RefCell::new(Some(then)),
    });
    for packages in batches {
        let (install_packages, group) = (install_packages.clone(), group.clone());
        throttled(
            ti,
            Box::new(move |done| install_packages(&packages, done)),
            Box::new(move |result| group.body_done(result)),
        );
    }
}

/// PORT: Go `core.ThrottleGroup` of `installNpmPackages`: the number of
/// bodies that still run, the first error, and the rest of the request after
/// `tg.Wait()`.
struct ThrottleGroup {
    running: Cell<usize>,
    first_err: RefCell<Option<GoError>>,
    then: RefCell<Option<Then<Result<(), GoError>>>>,
}

impl ThrottleGroup {
    /// A body ended with `result`. When it is the last one, `tg.Wait()`
    /// returns.
    fn body_done(&self, result: Result<(), GoError>) {
        if let Err(err) = result {
            self.first_err.borrow_mut().get_or_insert(err);
        }
        self.running.set(self.running.get() - 1);
        if self.running.get() > 0 {
            return;
        }
        let then = self
            .then
            .borrow_mut()
            .take()
            .expect("ata: a throttle group ended twice");
        let first_err = self.first_err.borrow_mut().take();
        then(match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        });
    }
}

/// PORT: the bodies that block in a send on a full Go
/// `concurrencySemaphore` channel, oldest first.
pub type SemaphoreWaiters = RefCell<VecDeque<Waiting<()>>>;

/// PORT: Go `ThrottleGroup.Go`. `body` starts when it holds a slot of the
/// semaphore of `ti`, and gives its result to its callback, which frees the
/// slot and then gives the result to `done`. A full semaphore (npm calls of
/// other ATA requests) makes `body` wait until a body ends; as with a Go
/// channel, the freed slot goes to the oldest waiting body.
fn throttled<T: 'static>(ti: &Rc<TypingsInstaller>, body: Box<dyn FnOnce(Then<T>)>, done: Then<T>) {
    let release_ti = ti.clone();
    let start: Then<()> = Box::new(move |()| {
        // Go: defer func() { <-tg.semaphore }()
        let release = SlotRelease(release_ti);
        body(Box::new(move |value: T| {
            drop(release);
            done(value);
        }));
    });
    match ti.concurrency_semaphore.0.try_send(()) {
        Ok(()) => start(()),
        Err(TrySendError::Full(())) => {
            ti.semaphore_waiters
                .borrow_mut()
                .push_back(Waiting::new(start));
        }
        Err(TrySendError::Disconnected(())) => panic!("ata: semaphore closed"),
    }
}

/// PORT: the semaphore slot of a body that runs. When the body ends (also on
/// a panic), the slot passes to the oldest waiting body, which resumes, or
/// goes back to the channel.
struct SlotRelease(Rc<TypingsInstaller>);

impl Drop for SlotRelease {
    fn drop(&mut self) {
        let next = self.0.semaphore_waiters.borrow_mut().pop_front();
        match next {
            // The slot stays taken and passes to the waiter.
            Some(next) => next.resume(()),
            None => {
                let _ = self.0.concurrency_semaphore.1.try_recv();
            }
        }
    }
}

/// PORT: the rest of an ATA request after a Go call that can block: the
/// callback gets the call's result.
pub type Then<T> = Box<dyn FnOnce(T)>;

/// PORT: the rest of an ATA request that waits for an event (Go: a blocked
/// goroutine): npm on a helper thread, the `init` of another request, or a
/// semaphore slot. `resume` queues it on this thread. One that never resumes
/// is never dropped (Go: a blocked goroutine ends with the process), so the
/// values it holds never resume another request when the thread ends.
pub struct Waiting<T: 'static>(Option<Then<T>>);

impl<T: 'static> Waiting<T> {
    fn new(then: Then<T>) -> Waiting<T> {
        Waiting(Some(then))
    }

    /// Queues the request to go on with `value` (`gostd::local::go`), as the
    /// wake of a goroutine. It runs under `core::go_wait_group_task`, as the
    /// background queue runs a task.
    fn resume(self, value: T) {
        gostd::local::go(Box::new(move || {
            let then = self.into_then();
            crate::core::go_wait_group_task(|| then(value));
        }));
    }

    fn into_then(mut self) -> Then<T> {
        self.0.take().expect("ata: a request resumed twice")
    }
}

impl<T: 'static> Drop for Waiting<T> {
    fn drop(&mut self) {
        if let Some(then) = self.0.take() {
            std::mem::forget(then);
        }
    }
}

/// PORT: runs `start` (an ATA call that gives its result to a callback) to
/// its end on this thread, for a Go caller that blocks
/// (`IsKnownTypesPackageName`). While the call waits, this runs the
/// thread's ready work (`gostd::local::run_pending`), as
/// `background::Queue::wait` does: npm results and the requests that the
/// call waits for run there.
pub fn block_on<T: 'static>(start: impl FnOnce(Then<T>)) -> T {
    let result: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let slot = result.clone();
    start(Box::new(move |value: T| *slot.borrow_mut() = Some(value)));
    loop {
        let value = result.borrow_mut().take();
        if let Some(value) = value {
            return value;
        }
        if !gostd::local::wait_pending() {
            panic!("ata: a blocking call waits for work that can not finish on this thread");
        }
        gostd::local::run_pending();
    }
}

impl TypingsInstaller {
    // Go: project/ata/ata.go:329 filterTypings
    // ts#64319: no project ID parameter.
    pub fn filter_typings(
        &self,
        logger: &dyn logging::Logger,
        typings_to_install: &[String],
    ) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for typing in typings_to_install {
            let typing_key = module::mangle_scoped_package_name(typing);
            if self.missing_typings_set.borrow().contains_key(&typing_key) {
                logger.log(&format!(
                    "ATA:: '{typing}':: '{typing_key}' is in missingTypingsSet - skipping..."
                ));
                continue;
            }
            let (validation_result, name, is_scope_name) = validate_package_name(typing);
            if validation_result != NAME_OK {
                // add typing name to missing set so we won't process it again
                self.missing_typings_set
                    .borrow_mut()
                    .insert(typing_key.clone(), true);
                logger.log(&format!(
                    "ATA:: {}",
                    render_package_name_validation_failure(
                        typing,
                        validation_result,
                        &name,
                        is_scope_name
                    )
                ));
                continue;
            }
            let types_registry = self.types_registry.borrow();
            let Some(types_registry_entry) = types_registry.get(&typing_key) else {
                logger.log(&format!(
                    "ATA:: '{typing}':: Entry for package '{typing_key}' does not exist in local types registry - skipping..."
                ));
                continue;
            };
            let typing_location = self
                .package_name_to_typing_location
                .borrow()
                .get(&typing_key)
                .cloned();
            if let Some(typing_location) = typing_location
                && is_typing_up_to_date(&typing_location, types_registry_entry.as_ref())
            {
                logger.log(&format!(
                    "ATA:: '{typing}':: '{typing_key}' already has an up-to-date typing - skipping..."
                ));
                continue;
            }
            result.push(typing_key);
        }
        result
    }

    // Go: project/ata/ata.go:361 init
    // PORT: `initOnce.Do` is the `OnceState` cell; `then` runs when it
    // returns. A caller that comes while another request's body waits for
    // npm waits in `init_waiters` until it is `Done`, as Go's `Do` blocks.
    // The body sets `Done` and resumes the waiters, oldest first, when it
    // ends, also on a panic (Go marks the Once done even if the body panics).
    pub fn init(
        self: &Rc<Self>,
        ctx: &Context,
        project_id: &str,
        fs: Rc<dyn vfs::Fs>,
        logger: Option<Rc<dyn logging::Logger>>,
        then: Then<()>,
    ) {
        match self.init_once.get() {
            OnceState::Done => return then(()),
            OnceState::Running => {
                self.init_waiters.borrow_mut().push(Waiting::new(then));
                return;
            }
            OnceState::NotStarted => {}
        }
        self.init_once.set(OnceState::Running);
        let done = InitDone(self.clone());

        logger.log(&format!(
            "ATA:: Global cache location '{}'",
            self.typings_location
        )); //, safe file path '" + safeListPath + "', types map path '" + typesMapLocation + "`")
        self.process_cache_location(project_id, &*fs, &logger);

        // !!! sheetal handle npm path here if we would support it
        //     // If the NPM path contains spaces and isn't wrapped in quotes, do so.
        //     if (this.npmPath.includes(" ") && this.npmPath[0] !== `"`) {
        //         this.npmPath = `"${this.npmPath}"`;
        //     }
        //     if (this.log.isEnabled()) {
        //         this.log.writeLine(`Process id: ${process.pid}`);
        //         this.log.writeLine(`NPM location: ${this.npmPath} (explicit '${ts.server.Arguments.NpmLocation}' ${npmLocation === undefined ? "not " : ""} provided)`);
        //         this.log.writeLine(`validateDefaultNpmLocation: ${validateDefaultNpmLocation}`);
        //     }

        self.ensure_typings_location_exists(&*fs, &logger);
        logger.log("ATA:: Updating types-registry@latest npm package...");
        let ti = self.clone();
        self.npm_install(
            ctx,
            &self.typings_location,
            &[
                "install".to_string(),
                "--ignore-scripts".to_string(),
                "types-registry@latest".to_string(),
            ],
            Box::new(move |(_, err): NpmResult| {
                ti.init_after_npm(err, &*fs, &logger);
                drop(done);
                then(());
            }),
        );
    }

    /// PORT: the rest of the Go `initOnce.Do` body after npm returns.
    fn init_after_npm(&self, err: Option<GoError>, fs: &dyn vfs::Fs, logger: &dyn logging::Logger) {
        match err {
            None => {
                logger.log("ATA:: Updated types-registry npm package");
            }
            Some(err) => {
                logger.log(&format!(
                    "ATA:: Error updating types-registry package: {err}"
                ));
                // !!! sheetal events to send
                //         // store error info to report it later when it is known that server is already listening to events from typings installer
                //         this.delayedInitializationError = {
                //             kind: "event::initializationFailed",
                //             message: (e as Error).message,
                //             stack: (e as Error).stack,
                //         };

                // const body: protocol.TypesInstallerInitializationFailedEventBody = {
                // 	message: response.message,
                // };
                // const eventName: protocol.TypesInstallerInitializationFailedEventName = "typesInstallerInitializationFailed";
                // this.event(body, eventName);
            }
        }

        let types_registry = self.load_types_registry_file(fs, logger);
        *self.types_registry.borrow_mut() = types_registry;
    }
}

/// PORT: the end of the `initOnce.Do` body of `TypingsInstaller::init`,
/// also on a panic: it sets `Done` and resumes the waiters, oldest first.
struct InitDone(Rc<TypingsInstaller>);

impl Drop for InitDone {
    fn drop(&mut self) {
        self.0.init_once.set(OnceState::Done);
        let waiters = self.0.init_waiters.take();
        for waiting in waiters {
            waiting.resume(());
        }
    }
}

// Go: project/ata/ata.go:402 npmConfig
// PORT: Go `map[string]any` is `IndexMap<String, LspAny>` (PORTING "JSON");
// nil is `None`. processCacheLocation ranges over it: insertion order, where
// Go map order is random.
#[derive(Clone, Debug, Default)]
pub struct NpmConfig {
    pub dev_dependencies: Option<IndexMap<String, LspAny>>,
}

// Go: project/ata/ata.go:406 npmDependecyEntry
#[derive(Clone, Debug, Default)]
pub struct NpmDependecyEntry {
    pub version: String,
}

// Go: project/ata/ata.go:409 npmLock
// PORT: nil maps are `None` (processCacheLocation tests them for nil).
#[derive(Clone, Debug, Default)]
pub struct NpmLock {
    pub dependencies: Option<FxHashMap<String, NpmDependecyEntry>>,
    pub packages: Option<FxHashMap<String, NpmDependecyEntry>>,
}

// PORT: Go decodes npmConfig, npmDependecyEntry and npmLock by reflection
// with the JSON v2 default struct rules (PORTING "JSON"): input names match
// exactly, unknown names are skipped, `null` sets the zero value. The Go
// `map[string]map[string]string` (types registry) uses the generic
// `FxHashMap<String, V>` and `Option<T>` impls in `frontend/json.rs` and
// `frontend/json_ext.rs`, so it has no impl here.
impl UnmarshalerFrom for NpmConfig {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "ata.npmConfig", |name, dec| {
            match name {
                "devDependencies" => json_unmarshal_decode(dec, &mut self.dev_dependencies)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = NpmConfig::default();
        }
        Ok(())
    }
}

impl UnmarshalerFrom for NpmDependecyEntry {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "ata.npmDependecyEntry", |name, dec| {
            match name {
                "version" => json_unmarshal_decode(dec, &mut self.version)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = NpmDependecyEntry::default();
        }
        Ok(())
    }
}

impl UnmarshalerFrom for NpmLock {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "ata.npmLock", |name, dec| {
            match name {
                "dependencies" => json_unmarshal_decode(dec, &mut self.dependencies)?,
                "packages" => json_unmarshal_decode(dec, &mut self.packages)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = NpmLock::default();
        }
        Ok(())
    }
}

impl TypingsInstaller {
    // Go: project/ata/ata.go:414 processCacheLocation
    pub fn process_cache_location(
        &self,
        project_id: &str,
        fs: &dyn vfs::Fs,
        logger: &dyn logging::Logger,
    ) {
        // Go does not read `projectID`.
        let _ = project_id;
        logger.log(&format!(
            "ATA:: Processing cache location {}",
            self.typings_location
        ));
        let package_json = tspath::combine_paths(&self.typings_location, &["package.json"]);
        let package_lock_json =
            tspath::combine_paths(&self.typings_location, &["package-lock.json"]);
        logger.log(&format!("ATA:: Trying to find '{package_json}'..."));
        if fs.file_exists(&package_json) && fs.file_exists(&package_lock_json) {
            let mut npm_config = NpmConfig::default();
            let npm_config_contents =
                parse_npm_config_or_lock(fs, logger, &package_json, &mut npm_config);
            let mut npm_lock = NpmLock::default();
            let npm_lock_contents =
                parse_npm_config_or_lock(fs, logger, &package_lock_json, &mut npm_lock);

            logger.log(&format!(
                "ATA:: Loaded content of {package_json}: {npm_config_contents}"
            ));
            logger.log(&format!(
                "ATA:: Loaded content of {package_lock_json}: {npm_lock_contents}"
            ));

            // !!! sheetal strada uses Node10
            // ts#64159
            let host = self.resolution_host();
            // ts#64299
            let resolver = module::new_resolver(module::ResolverOptions {
                host: Some(host),
                compiler_options: Some(Rc::new(CompilerOptions {
                    module_resolution: ModuleResolutionKind::NODE_NEXT,
                    ..CompilerOptions::default()
                })),
                ..Default::default()
            });
            if let Some(dev_dependencies) = &npm_config.dev_dependencies
                && (npm_lock.packages.is_some() || npm_lock.dependencies.is_some())
            {
                for key in dev_dependencies.keys() {
                    let mut npm_lock_value = npm_lock
                        .packages
                        .as_ref()
                        .and_then(|packages| packages.get(&format!("node_modules/{key}")));
                    if npm_lock_value.is_none() {
                        npm_lock_value = npm_lock
                            .dependencies
                            .as_ref()
                            .and_then(|dependencies| dependencies.get(key));
                    }
                    let Some(npm_lock_value) = npm_lock_value else {
                        // if package in package.json but not package-lock.json, skip adding to cache so it is reinstalled on next use
                        continue;
                    };
                    // key is @types/<package name>
                    let package_name = tspath::get_base_file_name(key);
                    if package_name.is_empty() {
                        continue;
                    }
                    let typing_file = self.typing_to_file_name(&resolver, &package_name);
                    if typing_file.is_empty() {
                        self.missing_typings_set
                            .borrow_mut()
                            .insert(package_name, true);
                        continue;
                    }
                    let existing_typing_file = self
                        .package_name_to_typing_location
                        .borrow()
                        .get(&package_name)
                        .cloned();
                    if let Some(existing_typing_file) = existing_typing_file {
                        if existing_typing_file.typings_location == typing_file {
                            continue;
                        }
                        logger.log(&format!(
                            "ATA:: New typing for package {package_name} from {typing_file} conflicts with existing typing file {}",
                            existing_typing_file.typings_location
                        ));
                    }
                    logger.log(&format!(
                        "ATA:: Adding entry into typings cache: {package_name} => {typing_file}"
                    ));
                    let version = &npm_lock_value.version;
                    if version.is_empty() {
                        continue;
                    }
                    let new_version = semver::must_parse_version(version);
                    let new_typing = Rc::new(CachedTyping {
                        typings_location: typing_file,
                        version: new_version,
                    });
                    self.package_name_to_typing_location
                        .borrow_mut()
                        .insert(package_name, new_typing);
                }
            }
        }
        logger.log(&format!(
            "ATA:: Finished processing cache location {}",
            self.typings_location
        ));
    }
}

// Go: project/ata/ata.go:473 parseNpmConfigOrLock
// PORT: Go `[T npmConfig | npmLock]` is any `UnmarshalerFrom`. Like Go, a
// read or decode error is ignored and a partly decoded `config` is kept.
pub fn parse_npm_config_or_lock<T: UnmarshalerFrom>(
    fs: &dyn vfs::Fs,
    logger: &dyn logging::Logger,
    location: &str,
    config: &mut T,
) -> String {
    // Go does not read `logger`.
    let _ = logger;
    let (contents, _) = fs.read_file(location);
    let _ = json::json_unmarshal(contents.as_bytes(), config, &[]);
    contents
}

impl TypingsInstaller {
    // Go: project/ata/ata.go:479 ensureTypingsLocationExists
    pub fn ensure_typings_location_exists(&self, fs: &dyn vfs::Fs, logger: &dyn logging::Logger) {
        let npm_config_path = tspath::combine_paths(&self.typings_location, &["package.json"]);
        logger.log(&format!("ATA:: Npm config file: {npm_config_path}"));

        if !fs.file_exists(&npm_config_path) {
            logger.log(&format!(
                "ATA:: Npm config file: '{npm_config_path}' is missing, creating new one..."
            ));
            let err = fs.write_file(&npm_config_path, "{ \"private\": true }");
            if let Err(err) = err {
                logger.log(&format!(
                    "ATA:: Npm config file write failed: {}",
                    crate::execute::incremental::emit_files::fs_error_text(&err)
                ));
            }
        }
    }

    // Go: project/ata/ata.go:492 typingToFileName (ts#64299: `*module.DefaultResolver`)
    pub fn typing_to_file_name(
        &self,
        resolver: &module::DefaultResolver,
        package_name: &str,
    ) -> String {
        let (result, _, _) = resolver.resolve_module_name(
            package_name,
            &tspath::combine_paths(&self.typings_location, &["index.d.ts"]),
            ModuleKind::NONE,
            None,
        );
        result.resolved_file_name.clone()
    }

    // Go: project/ata/ata.go:498 loadTypesRegistryFile
    pub fn load_types_registry_file(
        &self,
        fs: &dyn vfs::Fs,
        logger: &dyn logging::Logger,
    ) -> FxHashMap<String, Option<FxHashMap<String, String>>> {
        let types_registry_file = tspath::combine_paths(
            &self.typings_location,
            &["node_modules/types-registry/index.json"],
        );
        let (types_registry_file_contents, ok) = fs.read_file(&types_registry_file);
        if ok {
            // PORT: nil maps are `None` at each level that Go can test for
            // nil (see `TypingsInstaller.types_registry`). A nil
            // `entries["entries"]` reads like an empty map.
            let mut entries: FxHashMap<
                String,
                Option<FxHashMap<String, Option<FxHashMap<String, String>>>>,
            > = FxHashMap::default();
            let err =
                json::json_unmarshal(types_registry_file_contents.as_bytes(), &mut entries, &[]);
            if err.is_ok()
                && let Some(types_registry) = entries.remove("entries")
            {
                return types_registry.unwrap_or_default();
            }
            // PORT: Go `%v` of a nil error prints `<nil>`.
            let err_text = match &err {
                Ok(()) => "<nil>".to_string(),
                Err(err) => err.to_string(),
            };
            logger.log(&format!(
                "ATA:: Error when loading types registry file '{types_registry_file}': {err_text}"
            ));
        } else {
            logger.log(&format!(
                "ATA:: Error reading types registry file '{types_registry_file}'"
            ));
        }
        FxHashMap::default()
    }
}

#[cfg(test)]
mod npm_thread_tests {
    use super::*;
    use std::sync::{Condvar, Mutex};

    /// An ATA host whose npm runs on a helper thread and waits for `open`.
    struct GatedNpm {
        fs: Rc<dyn vfs::Fs>,
        cwd: String,
        calls: Arc<Mutex<Vec<String>>>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl NpmExecutor for GatedNpm {
        fn npm_install(
            &self,
            _ctx: &Context,
            _cwd: &str,
            _args: &[String],
        ) -> (Vec<u8>, Option<GoError>) {
            panic!("npm ran on the dispatch thread");
        }

        fn npm_install_func(&self) -> Option<NpmInstallFunc> {
            let (calls, gate) = (self.calls.clone(), self.gate.clone());
            Some(Arc::new(
                move |_ctx: &Context, cwd: &str, args: &[String]| {
                    calls.lock().unwrap().push(args.join(" "));
                    let (open, cond) = &*gate;
                    drop(
                        cond.wait_while(open.lock().unwrap(), |open| !*open)
                            .unwrap(),
                    );
                    let dir = std::path::Path::new(cwd).join("node_modules/types-registry");
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join("index.json"), r#"{"entries":{"left-pad":{}}}"#)
                        .unwrap();
                    (Vec::new(), None)
                },
            ))
        }
    }

    impl module::ResolutionHost for GatedNpm {
        fn fs(&self) -> &dyn vfs::Fs {
            &*self.fs
        }
        fn get_current_directory(&self) -> &str {
            &self.cwd
        }
    }

    fn gated_installer(throttle_limit: i32) -> (Rc<GatedNpm>, Rc<TypingsInstaller>, String) {
        let dir = std::env::temp_dir().join(format!(
            "goport-ata-{}-{throttle_limit}",
            std::process::id()
        ));
        let cwd = dir.to_string_lossy().replace('\\', "/");
        let host = Rc::new(GatedNpm {
            fs: vfs::osvfs::osvfs_fs(),
            cwd: cwd.clone(),
            calls: Arc::default(),
            gate: Arc::default(),
        });
        let ti = new_typings_installer(
            &TypingsInstallerOptions {
                typings_location: format!("{cwd}/cache"),
                throttle_limit,
            },
            host.clone(),
        );
        (host, ti, cwd)
    }

    /// Opens the gate of `host` and runs this thread's work until `done`
    /// is `n`.
    fn open_and_run(host: &GatedNpm, done: &Cell<usize>, n: usize) {
        let (open, cond) = &*host.gate;
        *open.lock().unwrap() = true;
        cond.notify_all();
        while done.get() < n && gostd::local::wait_pending() {
            gostd::local::run_pending();
        }
        assert_eq!(done.get(), n);
    }

    /// Go runs ATA requests on goroutines, and `initOnce.Do` blocks a second
    /// request until the first one's npm call ends. The port's requests wait
    /// for npm without blocking the thread, and npm runs once. Neither
    /// request resumes until npm ends: the first is resumed by the npm post,
    /// the second by the end of `init`.
    #[test]
    fn second_request_waits_for_init_off_thread() {
        let (host, ti, cwd) = gated_installer(5);
        let done = Rc::new(Cell::new(0));
        for _ in 0..2 {
            let (ti_then, done) = (ti.clone(), done.clone());
            ti.init(
                &gostd::context::background(),
                "p",
                host.fs.clone(),
                None,
                Box::new(move |()| {
                    assert_eq!(ti_then.init_once.get(), OnceState::Done);
                    done.set(done.get() + 1);
                }),
            );
        }
        assert_eq!((done.get(), ti.init_once.get()), (0, OnceState::Running));
        assert_eq!(ti.init_waiters.borrow().len(), 1);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !gostd::local::has_pending(),
            "a request was woken before npm ended"
        );

        open_and_run(&host, &done, 2);
        assert_eq!(
            *host.calls.lock().unwrap(),
            ["install --ignore-scripts types-registry@latest"]
        );
        assert!(ti.types_registry.borrow().contains_key("left-pad"));
        let _ = std::fs::remove_dir_all(&cwd);
    }

    /// Go `ThrottleGroup.Go` blocks in a send on the full semaphore channel,
    /// and a body that ends gives its slot to the oldest blocked body. With
    /// one slot, three npm calls run one at a time in request order, and a
    /// waiting body resumes only when it gets the slot.
    #[test]
    fn semaphore_gives_freed_slots_in_order() {
        let (host, ti, cwd) = gated_installer(1);
        let done = Rc::new(Cell::new(0));
        for i in 0..3 {
            let (ti_body, cwd, done) = (ti.clone(), cwd.clone(), done.clone());
            let args = [format!("t{i}")];
            throttled(
                &ti,
                Box::new(move |then| {
                    let ctx = gostd::context::background();
                    ti_body.npm_install(&ctx, &cwd, &args, then)
                }),
                Box::new(move |(_, err): NpmResult| {
                    assert!(err.is_none());
                    done.set(done.get() + 1);
                }),
            );
        }
        let started = std::time::Instant::now();
        while host.calls.lock().unwrap().is_empty() && started.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(*host.calls.lock().unwrap(), ["t0"]);
        assert_eq!(ti.semaphore_waiters.borrow().len(), 2);
        assert!(
            !gostd::local::has_pending(),
            "a request was woken before npm ended"
        );

        open_and_run(&host, &done, 3);
        assert_eq!(*host.calls.lock().unwrap(), ["t0", "t1", "t2"]);
        assert!(ti.semaphore_waiters.borrow().is_empty());
        assert!(
            ti.concurrency_semaphore.1.try_recv().is_err(),
            "a slot stayed taken"
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
