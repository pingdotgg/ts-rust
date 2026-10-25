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
//! The port's request is a future (the Go functions that reach npm are
//! `async fn`, and each Go blocking point is an `.await`) that `run_task`
//! runs on the dispatch thread. A waiting request costs nothing until the
//! event it waits for wakes it, as a blocked goroutine does:
//! - npm: an executor that gives `npm_install_func` (the LSP server) runs
//!   npm on a helper thread, which posts the result to the dispatch thread
//!   once (`gostd::local::post_later`). Without it (the test mock) npm runs
//!   in the first poll and the whole request ends in that poll.
//! - `initOnce.Do`: the request that runs `init` wakes the others when it
//!   is done (`init_waiters`).
//! - the `concurrencySemaphore` send: a body that ends gives its slot to the
//!   oldest waiting body, as a Go channel receive does (`semaphore_waiters`).

use crate::project::ata::prelude::*;

use crate::frontend::core_ext::TypeAcquisition;
use crate::frontend::json::{self, JsonDecoder, JsonError, UnmarshalerFrom, json_unmarshal_decode};
use crate::frontend::json_ext::{LspAny, unmarshal_struct_fields};
use crate::frontend::{module, semver, tspath, vfs};
use crate::gostd::{self, Context, GoError};
use crate::project::logging::{self, Logger as _};
use std::cell::Cell;
use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::task::{Poll, Waker};

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
    pub init_waiters: RefCell<Vec<Waker>>,

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
    pub fn is_known_types_package_name(
        &self,
        project_id: &dyn ProjectID,
        name: &str,
        fs: &dyn vfs::Fs,
        logger: &dyn logging::Logger,
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
        block_on(self.init(
            &gostd::context::background(),
            &project_id.string(),
            fs,
            logger,
        ));
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
    // result on error, so the result is a `Result`. PORT: async (see the
    // file comment).
    pub async fn install_typings_exported(
        &self,
        ctx: &Context,
        request: &TypingsInstallRequest,
    ) -> Result<TypingsInstallResult, GoError> {
        let mut result = self.discover_and_install_typings(ctx, request).await;
        if let Ok(result) = &mut result {
            result.typings_files.sort();
            result.files_to_watch.sort();
            request.logger.log(&format!(
                "ATA:: Got install request for: {}",
                request.project_id.string()
            ));
        }
        result
    }

    // Go: project/ata/ata.go:128 discoverAndInstallTypings
    pub async fn discover_and_install_typings(
        &self,
        ctx: &Context,
        request: &TypingsInstallRequest,
    ) -> Result<TypingsInstallResult, GoError> {
        self.init(
            ctx,
            &request.project_id.string(),
            &*request.fs,
            &request.logger,
        )
        .await;

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
                let typings_files = self
                    .install_typings(
                        ctx,
                        request_id,
                        &cached_typing_paths,
                        &filtered_typings,
                        &request.logger,
                    )
                    .await?;
                return Ok(TypingsInstallResult {
                    typings_files,
                    files_to_watch,
                });
            }
            request.logger.log(
                "ATA:: All typings are known to be missing or invalid - no need to install more typings",
            );
        } else {
            request
                .logger
                .log("ATA:: No new typings were requested as a result of typings discovery");
        }

        Ok(TypingsInstallResult {
            typings_files: cached_typing_paths,
            files_to_watch,
        })
        // !!! sheetal events to send
        // this.event(response, "setTypings");
    }

    // Go: project/ata/ata.go:168 installTypings
    // ts#64319: no project ID or typings info parameters.
    pub async fn install_typings(
        &self,
        ctx: &Context,
        request_id: i32,
        currently_cached_typings: &[String],
        filtered_typings: &[String],
        logger: &dyn logging::Logger,
    ) -> Result<Vec<String>, GoError> {
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

        let (package_names, ok) = self
            .install_worker(ctx, request_id, &scoped_typings, logger)
            .await;
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
    pub async fn install_worker(
        &self,
        ctx: &Context,
        request_id: i32,
        package_names: &[String],
        logger: &dyn logging::Logger,
    ) -> (Vec<String>, bool) {
        // PORT: Go `%v` of a slice; log text is not compared.
        logger.log(&format!(
            "ATA:: #{request_id} with cwd: {} arguments: {package_names:?}",
            self.typings_location
        ));
        let err = install_npm_packages_async(
            ctx,
            package_names,
            &self.concurrency_semaphore,
            &self.semaphore_waiters,
            |package_names| async move {
                let mut npm_args: Vec<String> = Vec::new();
                npm_args.extend(["install".to_string(), "--ignore-scripts".to_string()]);
                npm_args.extend(package_names.iter().cloned());
                npm_args.extend([
                    "--save-dev".to_string(),
                    format!("--user-agent=\"typesInstaller/{}\"", crate::core::version()),
                ]);
                let (output, err) = self
                    .npm_install(ctx, &self.typings_location, &npm_args)
                    .await;
                if let Some(err) = err {
                    // PORT: Go `%s` of a `[]byte`.
                    logger.log(&format!(
                        "ATA:: Output is: {}",
                        String::from_utf8_lossy(&output)
                    ));
                    return Err(err);
                }
                Ok(())
            },
        )
        .await;
        logger.log(&format!("TI:: npm install #{request_id} completed"));
        (package_names.to_vec(), err.is_ok())
    }

    /// PORT: Go `ti.host.NpmInstall(cwd, args)`. With the host's
    /// `npm_install_func`, npm runs on a helper thread (Go: the goroutine
    /// blocks in `exec.Cmd.Output`). When npm ends, the thread posts the
    /// result to this thread once, and the post wakes the request. Without
    /// it, npm runs in the first poll.
    async fn npm_install(&self, ctx: &Context, cwd: &str, args: &[String]) -> NpmResult {
        let Some(npm_install) = self.host.npm_install_func() else {
            return self.host.npm_install(ctx, cwd, args);
        };
        // The result, and the waker of the request while it waits.
        let state: Rc<RefCell<(Option<NpmResult>, Option<Waker>)>> = Rc::default();
        let (tx, rx) = std::sync::mpsc::channel::<NpmResult>();
        let post = {
            let state = state.clone();
            gostd::local::post_later(Box::new(move || {
                // No result: the npm function panicked on the helper thread.
                let result = rx.try_recv().unwrap_or_else(|_| {
                    (
                        Vec::new(),
                        Some(gostd::errors::new("npm install: the helper thread failed")),
                    )
                });
                let waker = {
                    let mut state = state.borrow_mut();
                    state.0 = Some(result);
                    state.1.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            }))
        };
        let (ctx, cwd, args) = (ctx.clone(), cwd.to_string(), args.to_vec());
        crate::core::GoThread::new()
            .name("ata-npm".to_string())
            .spawn(move || {
                let _ = tx.send(npm_install(&ctx, &cwd, &args));
                post.post();
            });
        poll_fn(|cx| {
            let mut state = state.borrow_mut();
            match state.0.take() {
                Some(result) => Poll::Ready(result),
                None => {
                    state.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

/// PORT: the result of `NpmExecutor::npm_install`.
type NpmResult = (Vec<u8>, Option<GoError>);

// Go: project/ata/ata.go:291 installNpmPackages
// PORT: the Go test entry; it runs `install_npm_packages_async` to its end
// with `install_packages` as each body.
pub fn install_npm_packages(
    ctx: &Context,
    package_names: &[String],
    concurrency_semaphore: &(SyncSender<()>, Receiver<()>),
    install_packages: &dyn Fn(&[String]) -> Result<(), GoError>,
) -> Result<(), GoError> {
    block_on(install_npm_packages_async(
        ctx,
        package_names,
        concurrency_semaphore,
        &SemaphoreWaiters::default(),
        |packages| std::future::ready(install_packages(packages)),
    ))
}

// Go: project/ata/ata.go:291 installNpmPackages
// PORT: each `tg.Go` body is a future. The bodies run at the same time, each
// while it holds a semaphore slot, and the result is the first error, as
// `tg.Wait()` gives.
pub async fn install_npm_packages_async<'a, Fut>(
    ctx: &Context,
    package_names: &'a [String],
    concurrency_semaphore: &(SyncSender<()>, Receiver<()>),
    semaphore_waiters: &SemaphoreWaiters,
    install_packages: impl Fn(&'a [String]) -> Fut,
) -> Result<(), GoError>
where
    Fut: Future<Output = Result<(), GoError>> + 'a,
{
    // Go: tg := core.NewThrottleGroup(ctx, concurrencySemaphore)
    let _ = ctx;
    let mut tg: Vec<Pin<Box<dyn Future<Output = Result<(), GoError>> + '_>>> = Vec::new();

    let mut current_command_start: usize = 0;
    let mut current_command_end: usize = 0;
    let mut current_command_size: usize = 100;

    for package_name in package_names {
        current_command_size = current_command_size + package_name.len() + 1;
        if current_command_size < 8000 {
            current_command_end += 1;
        } else {
            let packages = &package_names[current_command_start..current_command_end];
            tg.push(Box::pin(throttled(
                concurrency_semaphore,
                semaphore_waiters,
                install_packages(packages),
            )));
            current_command_start = current_command_end;
            current_command_size = 100 + package_name.len() + 1;
            current_command_end += 1;
        }
    }

    // Handle the final batch
    if current_command_start < package_names.len() {
        let packages = &package_names[current_command_start..current_command_end];
        tg.push(Box::pin(throttled(
            concurrency_semaphore,
            semaphore_waiters,
            install_packages(packages),
        )));
    }

    // Go: tg.Wait()
    let mut first_err: Option<GoError> = None;
    poll_fn(|cx| {
        tg.retain_mut(|body| match body.as_mut().poll(cx) {
            Poll::Ready(result) => {
                if let Err(err) = result {
                    first_err.get_or_insert(err);
                }
                false
            }
            Poll::Pending => true,
        });
        if tg.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// PORT: the goroutines that block in a send on a full Go
/// `concurrencySemaphore` channel, oldest first.
pub type SemaphoreWaiters = RefCell<VecDeque<Rc<SlotWaiter>>>;

/// PORT: one body that waits for a semaphore slot. `granted` is set when a
/// body that ends gives it its slot.
pub struct SlotWaiter {
    granted: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

/// PORT: Go `ThrottleGroup.Go` takes a semaphore slot before the body
/// starts and frees it when the body ends. A full semaphore (npm calls of
/// other ATA requests) waits until a body ends; as with a Go channel, the
/// freed slot goes to the oldest waiting body.
async fn throttled<T>(
    semaphore: &(SyncSender<()>, Receiver<()>),
    waiters: &SemaphoreWaiters,
    body: impl Future<Output = T>,
) -> T {
    let mut waiter: Option<Rc<SlotWaiter>> = None;
    poll_fn(|cx| {
        if let Some(waiter) = &waiter {
            if waiter.granted.get() {
                return Poll::Ready(());
            }
            *waiter.waker.borrow_mut() = Some(cx.waker().clone());
            return Poll::Pending;
        }
        match semaphore.0.try_send(()) {
            Ok(()) => return Poll::Ready(()),
            Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => panic!("ata: semaphore closed"),
        }
        let new_waiter = Rc::new(SlotWaiter {
            granted: Cell::new(false),
            waker: RefCell::new(Some(cx.waker().clone())),
        });
        waiters.borrow_mut().push_back(new_waiter.clone());
        waiter = Some(new_waiter);
        Poll::Pending
    })
    .await;
    // Go: defer func() { <-tg.semaphore }()
    struct Release<'a>(&'a (SyncSender<()>, Receiver<()>), &'a SemaphoreWaiters);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            let next = self.1.borrow_mut().pop_front();
            match next {
                // The slot stays taken and passes to the waiter.
                Some(next) => {
                    next.granted.set(true);
                    let waker = next.waker.take();
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
                None => {
                    let _ = self.0.1.try_recv();
                }
            }
        }
    }
    let _release = Release(semaphore, waiters);
    body.await
}

thread_local! {
    /// PORT: the ATA requests of this thread that wait, by id. It is not
    /// dropped when the thread ends (Go: a blocked goroutine ends with the
    /// process), so a request's drop never wakes another one then.
    static TASKS: std::mem::ManuallyDrop<RefCell<FxHashMap<u64, Pin<Box<dyn Future<Output = ()>>>>>> =
        std::mem::ManuallyDrop::new(RefCell::new(FxHashMap::default()));
    static NEXT_TASK: Cell<u64> = const { Cell::new(0) };
}

/// PORT: the waker of an ATA request. A wake queues the next poll of the
/// request on the dispatch thread (`gostd::local::go`), once until that poll
/// runs. Every wake comes on the dispatch thread: an npm result arrives
/// through `gostd::local::post_later`, and `init` and the semaphore wake
/// from other requests.
struct TaskWaker {
    id: u64,
    thread: std::thread::ThreadId,
    queued: AtomicBool,
}

impl std::task::Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        assert!(
            std::thread::current().id() == self.thread,
            "ata: a request woke on another thread"
        );
        if self.queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let waker = self.clone();
        gostd::local::go(Box::new(move || {
            waker.queued.store(false, Ordering::SeqCst);
            crate::core::go_wait_group_task(|| poll_task(&waker));
        }));
    }
}

/// PORT: runs `task`, the rest of an ATA goroutine, on the dispatch thread.
/// It polls `task` now; while `task` waits (for npm on a helper thread, a
/// semaphore slot or another request's `init`), it is not polled until
/// that event wakes it. A `background::TaskHold` moved into `task` keeps the
/// background task running until `task` ends. A later poll runs under
/// `core::go_wait_group_task`, as the queue runs the task.
pub fn run_task(task: Pin<Box<dyn Future<Output = ()>>>) {
    let id = NEXT_TASK.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    TASKS.with(|tasks| tasks.borrow_mut().insert(id, task));
    poll_task(&Arc::new(TaskWaker {
        id,
        thread: std::thread::current().id(),
        queued: AtomicBool::new(false),
    }));
}

/// Polls the request of `waker` once, if it still waits.
fn poll_task(waker: &Arc<TaskWaker>) {
    let Some(mut task) = TASKS.with(|tasks| tasks.borrow_mut().remove(&waker.id)) else {
        return;
    };
    let std_waker = Waker::from(waker.clone());
    let mut cx = std::task::Context::from_waker(&std_waker);
    if task.as_mut().poll(&mut cx).is_pending() {
        TASKS.with(|tasks| tasks.borrow_mut().insert(waker.id, task));
    }
}

/// PORT: runs `fut` to its end on this thread, for a Go caller that blocks
/// (`IsKnownTypesPackageName`, the test entry `install_npm_packages`).
/// While `fut` waits, it runs this thread's ready work
/// (`gostd::local::run_pending`), as `background::Queue::wait` does: npm
/// results and the requests that `fut` waits for run there.
pub fn block_on<T>(fut: impl Future<Output = T>) -> T {
    let mut cx = std::task::Context::from_waker(Waker::noop());
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(value) = fut.as_mut().poll(&mut cx) {
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
    // PORT: `initOnce.Do` is the `OnceState` cell. A caller that comes while
    // another request's body waits for npm waits in `init_waiters` until it
    // is `Done`, as Go's `Do` blocks. The body sets `Done` and wakes the
    // waiters, oldest first, when it ends, also on a panic (Go marks the
    // Once done even if the body panics).
    pub async fn init(
        &self,
        ctx: &Context,
        project_id: &str,
        fs: &dyn vfs::Fs,
        logger: &dyn logging::Logger,
    ) {
        match self.init_once.get() {
            OnceState::Done => return,
            OnceState::Running => {
                poll_fn(|cx| {
                    if self.init_once.get() == OnceState::Done {
                        return Poll::Ready(());
                    }
                    let mut waiters = self.init_waiters.borrow_mut();
                    if !waiters.iter().any(|w| w.will_wake(cx.waker())) {
                        waiters.push(cx.waker().clone());
                    }
                    Poll::Pending
                })
                .await;
                return;
            }
            OnceState::NotStarted => {}
        }
        self.init_once.set(OnceState::Running);
        struct SetDone<'a>(&'a TypingsInstaller);
        impl Drop for SetDone<'_> {
            fn drop(&mut self) {
                self.0.init_once.set(OnceState::Done);
                let waiters = self.0.init_waiters.take();
                for waker in waiters {
                    waker.wake();
                }
            }
        }
        let _done = SetDone(self);

        logger.log(&format!(
            "ATA:: Global cache location '{}'",
            self.typings_location
        )); //, safe file path '" + safeListPath + "', types map path '" + typesMapLocation + "`")
        self.process_cache_location(project_id, fs, logger);

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

        self.ensure_typings_location_exists(fs, logger);
        logger.log("ATA:: Updating types-registry@latest npm package...");
        let (_, err) = self
            .npm_install(
                ctx,
                &self.typings_location,
                &[
                    "install".to_string(),
                    "--ignore-scripts".to_string(),
                    "types-registry@latest".to_string(),
                ],
            )
            .await;
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

    /// A task that counts its polls in `polls`.
    fn counted(
        polls: Rc<Cell<u32>>,
        fut: impl Future<Output = ()> + 'static,
    ) -> Pin<Box<dyn Future<Output = ()>>> {
        let mut fut = Box::pin(fut);
        Box::pin(poll_fn(move |cx| {
            polls.set(polls.get() + 1);
            fut.as_mut().poll(cx)
        }))
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
    /// request is polled again until npm ends: the first is woken by the npm
    /// post, the second by the end of `init`.
    #[test]
    fn second_request_waits_for_init_off_thread() {
        let (host, ti, cwd) = gated_installer(5);
        let done = Rc::new(Cell::new(0));
        let polls: Vec<Rc<Cell<u32>>> = (0..2).map(|_| Rc::default()).collect();
        for polls in &polls {
            let (ti, fs, done) = (ti.clone(), host.fs.clone(), done.clone());
            run_task(counted(polls.clone(), async move {
                ti.init(
                    &gostd::context::background(),
                    "p",
                    &*fs,
                    &None::<Rc<dyn logging::Logger>>,
                )
                .await;
                assert_eq!(ti.init_once.get(), OnceState::Done);
                done.set(done.get() + 1);
            }));
        }
        assert_eq!((done.get(), ti.init_once.get()), (0, OnceState::Running));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !gostd::local::has_pending(),
            "a request was woken before npm ended"
        );

        open_and_run(&host, &done, 2);
        assert_eq!(polls.iter().map(|p| p.get()).collect::<Vec<_>>(), [2, 2]);
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
    /// waiting body is polled only when it gets the slot and when its npm
    /// call ends.
    #[test]
    fn semaphore_gives_freed_slots_in_order() {
        let (host, ti, cwd) = gated_installer(1);
        let done = Rc::new(Cell::new(0));
        let polls: Vec<Rc<Cell<u32>>> = (0..3).map(|_| Rc::default()).collect();
        for (i, polls) in polls.iter().enumerate() {
            let (ti, cwd, done) = (ti.clone(), cwd.clone(), done.clone());
            run_task(counted(polls.clone(), async move {
                let args = [format!("t{i}")];
                let ctx = gostd::context::background();
                let npm = ti.npm_install(&ctx, &cwd, &args);
                let (_, err) =
                    throttled(&ti.concurrency_semaphore, &ti.semaphore_waiters, npm).await;
                assert!(err.is_none());
                done.set(done.get() + 1);
            }));
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
        assert_eq!(polls.iter().map(|p| p.get()).collect::<Vec<_>>(), [2, 3, 3]);
        assert!(ti.semaphore_waiters.borrow().is_empty());
        assert!(
            ti.concurrency_semaphore.1.try_recv().is_err(),
            "a slot stayed taken"
        );
        let _ = std::fs::remove_dir_all(&cwd);
    }
}
