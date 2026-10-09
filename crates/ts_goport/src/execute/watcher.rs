//! Go: execute/watcher.go (package `execute`): `tsc --watch` without
//! `--build`.
//!
//! PORT: testing. Go `testing tsc.CommandLineTesting` is `None` outside
//! tests (see tsc/compile.rs). The test watch backend comes from
//! `set_test_watch_backend`, not from `testing`.
//!
//! PORT: Go runs `DoCycle` from `WatchManager.RunLoop` on the goroutine
//! that called `start`. The port does the same on the calling thread. The
//! watch manager is `Rc<RefCell<WatchManager>>`: the loop keeps a shared
//! borrow while `do_cycle` borrows the watcher mutably, and `do_cycle` only
//! takes shared borrows of the manager.
//!
//! PORT: Go makes a new program on every build. Each build here makes a
//! program version (`execute_tsc::new_frontend_program`) and runs with it
//! current (`core::enter_program`). Files that the source file cache keeps
//! are shared with the last version. The last version is released when the
//! next build has read it (Go drops the old program there). The rest of the
//! old program is freed after the next build has reported its status.

use crate::contentmapper::{self, Mapper, SourceFiles};
use crate::execute::build::host::TscExtendedConfigCache;
use crate::execute::execute_tsc::{get_trace_from_sys, new_frontend_program, os_write_file};
use crate::execute::incremental;
use crate::execute::tsc::compile::{
    CommandLineTesting, CompileAndEmitResult, CompileTimes, System, SystemParseConfigHost,
    new_content_mapper_host, write_str,
};
use crate::execute::tsc::diagnostics::{
    DiagnosticReporter, DiagnosticsReporter, create_watch_status_reporter,
};
use crate::execute::tsc::emit::{EmitInput, emit_files_and_report_errors};
use crate::execute::watchmanager::{
    WatchBackend, WatchManager, can_watch_directory, new_dir_watch_set, new_watch_manager,
};
use crate::frontend::prelude::*;
use crate::frontend::vfs::trackingvfs;
use crate::fswatch;
use crate::gostd::{Context, GoError};
use std::sync::Arc;
use std::time::SystemTime;

// Go: execute/watcher.go:26 cachedSourceFile
// PORT: Go `time.Time` is `Option<SystemTime>` (`None` = zero), as in
// `vfs::FileInfo`.
pub struct CachedSourceFile {
    pub file: Rc<ParsedSourceFile>,
    pub mod_time: Option<SystemTime>,
}

// Go: execute/watcher.go:31 watchCompilerHost
// PORT: the embedded Go `compiler.CompilerHost` is the `compiler_host`
// field; the trait impl below forwards to it. Go `*collections.SyncMap`
// shared with the watcher is `Rc<RefCell<FxHashMap>>` (one thread).
pub struct WatchCompilerHost {
    pub compiler_host: Rc<dyn CompilerHost>,
    pub cache: Rc<RefCell<FxHashMap<Path, Rc<CachedSourceFile>>>>,
    /// PORT: not in Go (`reuse_parse`). In the build after a config change,
    /// the source file cache from before the change. `do_build` empties it
    /// when the program is loaded.
    pub config_parses: RefCell<FxHashMap<Path, Rc<CachedSourceFile>>>,
}

/// PORT: not in Go. Records a new parse that a watch host keeps for later
/// builds (`WatchCompilerHost::get_source_file`, the build host's
/// `watch_source_file`). A kept parse can be left out of the program that
/// loaded it and be a program file of a later one: Go keeps one copy of a
/// package that two `node_modules` directories hold with the same name and
/// version (filesparser.go:448, the other path maps to the kept file at
/// :473), and its watcher keeps the parse of the other copy, because that
/// path is in `FilesByPath` (watcher.go:516). When the versions differ
/// later, the next program takes that parse. So the publish of this
/// program gives the store its complete Go file
/// (`program::note_parsed_source_file`), where it gave it the name only
/// and the next build stopped with exit 70 ("published as a store that is
/// not a source file"). A new parse of a path that this thread published
/// before gets its `FileVersion` here, as `program::mark_freeable_parses`
/// gives one to the files of the program, so it is freed with its last
/// holder, also when no program takes it. Without one, each edit of the
/// left-out copy published a static store (9 MiB for each edit of a
/// 335 KB `.d.ts` copy).
pub(crate) fn note_kept_parse(file: &Rc<ParsedSourceFile>) {
    if crate::ast::is_published(file.store) {
        return;
    }
    if file.version.get().is_none() && crate::ast::freeable_path(&file.path().0) {
        let _ = file.version.set(crate::ast::FileVersion::new(file.store));
    }
    crate::program::note_parsed_source_file(file);
}

impl WatchCompilerHost {
    /// PORT: not in Go. The parse that the source file cache had before a
    /// config change, when a parse now gives the same file: the same text
    /// (a bundled lib never changes), and the same parse options or options
    /// that the parse did not read (`parse_with_options`, which then gives
    /// a copy with the new options). Go parses every file again after a
    /// config change (`doBuild` empties `sourceFileCache`, watcher.go:409),
    /// and so did the port, on one thread in watchfree1: a query-core build
    /// after a tsconfig edit took 1.8 times as long as Go, and an effect
    /// build after a `moduleDetection` edit 2.7 times. The parse and the
    /// bind of a file read only its text and its parse options, so the
    /// output does not change. The Go language server parse cache keys a
    /// parse by the same fields (project/parsecache.go:16 `ParseCacheKey`).
    /// The text is read as Go reads it for its parse. A file with
    /// diagnostics in the old snapshot is not in `config_parses` (see
    /// `Watcher::do_build`). The parse workers parse the files that this
    /// does not give (`prefetch_parses`).
    fn reuse_parse(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let old = self.config_parses.borrow_mut().remove(&opts.path)?;
        let file = crate::frontend::parser::parse_with_options(&old.file, opts)?;
        if !crate::frontend::bundled::is_bundled(&opts.file_name) {
            let (text, ok) = self.compiler_host.fs().read_file(&opts.file_name);
            if !ok || text != old.file.text() {
                return None;
            }
        }
        Some(file)
    }
}

impl CompilerHost for WatchCompilerHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.compiler_host.fs()
    }

    fn default_library_path(&self) -> String {
        self.compiler_host.default_library_path()
    }

    fn get_current_directory(&self) -> String {
        self.compiler_host.get_current_directory()
    }

    fn trace(&self, msg: &'static Message, args: Vec<String>) {
        self.compiler_host.trace(msg, args);
    }

    // Go: execute/watcher.go:34 (*watchCompilerHost).GetSourceFile
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let info = self.compiler_host.fs().stat(&opts.file_name);

        let cached = self.cache.borrow().get(&opts.path).cloned();
        if let Some(cached) = cached {
            if let Some(info) = &info {
                if info.mod_time() == cached.mod_time {
                    return Some(cached.file.clone());
                }
            }
        }

        let file = self
            .reuse_parse(opts)
            .or_else(|| self.compiler_host.get_source_file(opts));
        if let Some(file) = &file {
            if let Some(info) = &info {
                note_kept_parse(file);
                self.cache.borrow_mut().insert(
                    opts.path.clone(),
                    Rc::new(CachedSourceFile {
                        file: file.clone(),
                        mod_time: info.mod_time(),
                    }),
                );
            }
        } else {
            self.cache.borrow_mut().remove(&opts.path);
        }
        file
    }

    // PORT: Go embeds the inner host, so these two go to it (tsgo#4712).
    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        self.compiler_host
            .get_content_mapped_source_files(parse_options, mapper)
    }

    fn content_mapper_project(&self) -> Option<Rc<dyn contentmapper::Project>> {
        self.compiler_host.content_mapper_project()
    }

    // PORT: not in Go (see `CompilerHost::prefetch_content_mapped`). The
    // inner host takes the workers' transforms.
    fn prefetch_content_mapped(&self) -> bool {
        self.compiler_host.prefetch_content_mapped()
    }

    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        self.compiler_host
            .get_resolved_project_reference(file_name, path)
    }

    // PORT: not in Go (see `CompilerHost::prefetch_parses`). A rebuild
    // gets the files that did not change from `cache`, so parse workers
    // would parse them again for nothing (about 38 MiB for each query-core
    // rebuild). A build with an empty cache parses ahead: the first build,
    // and a build after an overflow or a config change. Go parses every
    // file of those builds on goroutines. After a config change the workers
    // skip the parses that `reuse_parse` probably gives
    // (`cached_source_file_refs`), so they parse the files with old
    // diagnostics and the files whose parse read changed options. A worker
    // parse of a published path is a freeable file version
    // (`freeable_worker_parses`).
    fn prefetch_parses(&self) -> bool {
        self.cache.borrow().is_empty() && self.compiler_host.prefetch_parses()
    }

    // PORT: not in Go (see `CompilerHost::cached_source_file_refs`). The
    // parses from before a config change, each for the module indicator
    // options that its parse read, if any (`FileRefs::of_kept_parse`).
    fn cached_source_file_refs(&self) -> FxHashMap<String, Arc<FileRefs>> {
        self.config_parses
            .borrow()
            .values()
            .map(|cached| {
                let refs = FileRefs::of_kept_parse(&cached.file);
                (cached.file.file_name().to_string(), Arc::new(refs))
            })
            .collect()
    }

    // PORT: not in Go (see `CompilerHost::freeable_worker_parses`).
    fn freeable_worker_parses(&self) -> bool {
        true
    }
}

// Go: execute/watcher.go:59 Watcher
// PORT: Go `*tsoptions.ParsedCommandLine` is `Rc<ParsedCommandLine>`. Go
// `*collections.OrderedMap[string, any]` is
// `Option<IndexMap<String, CompilerOptionsValue>>` (as in build/host.rs).
// Go `*incremental.Program` and `*tsc.ExtendedConfigCache` are `Option`s
// (nil before `start`). Go `*collections.Set` of seen files is a plain
// set (empty before the first build, which Go's nil set reads as). Go
// `time.Time` is `Option<SystemTime>`.
pub struct Watcher {
    sys: Rc<dyn System>,
    config_file_name: String,
    config: Rc<ParsedCommandLine>,
    compiler_options_from_command_line: Rc<CompilerOptions>,
    command_line_raw: Option<IndexMap<String, CompilerOptionsValue>>,
    report_diagnostic: DiagnosticReporter,
    report_error_summary: DiagnosticsReporter,
    report_watch_status: DiagnosticReporter,
    testing: Option<Rc<dyn CommandLineTesting>>,

    // contentMapperHost transforms content-mapped files; it is created once per watch session (when
    // enabled) and reused across cycles. It closes itself when the session context is cancelled (see
    // contentmapper.New).
    // PORT: Go nil interfaces are `None`.
    content_mapper_host: Option<Rc<dyn contentmapper::Host>>,
    content_mapper_project: Option<Rc<dyn contentmapper::Project>>,

    program: Option<incremental::program::Program>,
    /// PORT: not in Go. The program that the last build replaced, without
    /// its program version (released at once, see `retire_program`). Go
    /// drops it and its GC frees it later; here its snapshot is freed after
    /// the build reports its status (`do_build`), not before the check.
    retired_program: Option<incremental::program::Program>,
    extended_config_cache: Option<Rc<TscExtendedConfigCache>>,
    config_modified: bool,
    config_has_errors: bool,
    config_file_paths: Vec<String>,

    source_file_cache: Rc<RefCell<FxHashMap<Path, Rc<CachedSourceFile>>>>,

    wm: Rc<RefCell<WatchManager>>,
    seen_files: FxHashSet<Path>, // all build dependencies (for event filtering)
    config_mtimes: FxHashMap<String, Option<SystemTime>>,
    watch_set_dirty: bool,
    // forceFullRebuild records a reason that requires a full NewProgram rebuild
    // (e.g. an event overflow, a mid-cycle watch failure, a newly appeared
    // project file, or a changed non-source dependency). Unlike watchSetDirty,
    // which is only raised to recheck wildcard roots and may be cleared once the
    // file set is confirmed unchanged, this flag is preserved until a full
    // rebuild actually runs so the single-file fast path cannot silently reuse a
    // stale program.
    force_full_rebuild: bool,
    program_ready: bool,

    // Test-only observability of which build path was taken.
    fast_path_builds: i32,
    full_builds: i32,
}

// Go: execute/watcher.go:103 `var _ tsc.Watcher = (*Watcher)(nil)`
impl crate::execute::tsc::Watcher for Watcher {
    fn do_cycle(&mut self) {
        Watcher::do_cycle(self);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

thread_local! {
    /// The watch backend of `set_test_watch_backend`.
    static TEST_WATCH_BACKEND: RefCell<Option<Rc<dyn WatchBackend>>> = const { RefCell::new(None) };
}

/// Go `tsc.CommandLineTesting` with `WatchBackend()`
/// (watchmanager.CommandLineTestingWithWatchBackend): a watcher that this
/// thread makes later uses `backend` in place of the OS file watcher.
// PORT: a test harness sets the backend here, not through `testing`. With
// `testing` set too, the watcher is in Go's test mode: `start` returns
// after the first build and the test calls `DoCycle`. Without it (the
// `goport_watch` bin), the watcher still runs its own loop.
pub fn set_test_watch_backend(backend: Rc<dyn WatchBackend>) {
    TEST_WATCH_BACKEND.with(|slot| *slot.borrow_mut() = Some(backend));
}

/// The backend of `set_test_watch_backend` (also for `tsc -b --watch`).
pub(crate) fn test_watch_backend() -> Option<Rc<dyn WatchBackend>> {
    TEST_WATCH_BACKEND.with(|slot| slot.borrow().clone())
}

// Go: execute/watcher.go:105 createWatcher
pub fn create_watcher(
    sys: Rc<dyn System>,
    config_parse_result: Rc<ParsedCommandLine>,
    compiler_options_from_command_line: Rc<CompilerOptions>,
    command_line_raw: Option<IndexMap<String, CompilerOptionsValue>>,
    report_diagnostic: DiagnosticReporter,
    report_error_summary: DiagnosticsReporter,
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> Watcher {
    // PORT: Go passes the method value `sys.FS().DirectoryExists`.
    let fs = sys.fs();
    let use_case_sensitive_file_names = fs.use_case_sensitive_file_names();
    let mut wm = new_watch_manager(
        sys.writer(),
        Box::new(move |path: &str| fs.directory_exists(path)),
        use_case_sensitive_file_names,
    );
    // Go: if t, ok := testing.(CommandLineTestingWithWatchBackend); ok { wm.SetBackend(t.WatchBackend()) }
    if let Some(backend) = test_watch_backend() {
        wm.set_backend(backend);
    }
    let mut w = Watcher {
        sys: sys.clone(),
        config_file_name: String::new(),
        config: config_parse_result.clone(),
        compiler_options_from_command_line,
        command_line_raw,
        report_diagnostic,
        report_error_summary,
        report_watch_status: create_watch_status_reporter(
            sys,
            &config_parse_result.locale(),
            config_parse_result.compiler_options().clone(),
            testing.clone(),
        ),
        testing,
        content_mapper_host: None,
        content_mapper_project: None,
        program: None,
        retired_program: None,
        extended_config_cache: None,
        config_modified: false,
        config_has_errors: false,
        config_file_paths: Vec::new(),
        source_file_cache: Rc::new(RefCell::new(FxHashMap::default())),
        wm: Rc::new(RefCell::new(wm)),
        seen_files: FxHashSet::default(),
        config_mtimes: FxHashMap::default(),
        watch_set_dirty: false,
        force_full_rebuild: false,
        program_ready: false,
        fast_path_builds: 0,
        full_builds: 0,
    };
    if let Some(config_file) = &config_parse_result.config_file {
        w.config_file_name = source_file_file_name(config_file.source_file).to_string();
    }
    w
}

impl Watcher {
    // Go: execute/watcher.go:136 (*Watcher).start
    pub fn start(&mut self, ctx: &Context) {
        // PORT: not in Go. From the second build on, a new parse of a file
        // that an earlier build published is a freeable file version
        // (`ast::set_watch_process`, `program::mark_freeable_parses`), so
        // it is freed with its last holder, as Go's GC frees an old
        // `*ast.SourceFile`. The first version of each file stays static.
        crate::ast::set_watch_process();
        self.content_mapper_host =
            new_content_mapper_host(ctx, &self.sys, self.config.compiler_options());
        let config = self.config.clone();
        self.replace_content_mapper_project(&config);
        self.wm.borrow().lock();
        let extended_config_cache = Rc::new(TscExtendedConfigCache::default());
        self.extended_config_cache = Some(extended_config_cache.clone());
        let host = new_compiler_host(
            &self.sys.get_current_directory(),
            self.sys.fs(),
            &self.sys.default_library_path(),
            Some(extended_config_cache as Rc<dyn ExtendedConfigCache>),
            Some(get_trace_from_sys(
                &*self.sys,
                self.config.locale(),
                self.testing.clone(),
            )),
            self.content_mapper_project.clone(),
        );
        self.program = incremental::program::read_build_info_program(
            &self.config,
            &*incremental::incremental::new_build_info_reader(host.clone()),
            &*host,
        );

        if !self.config_file_name.is_empty() {
            let mut config_file_paths = vec![self.config_file_name.clone()];
            config_file_paths.extend(self.config.extended_source_files().iter().cloned());
            self.config_file_paths = config_file_paths;
        }

        let (value, _) = self.sys.get_environment_variable("TS_WATCH_DEBUG");
        if !value.is_empty() {
            self.wm.borrow_mut().debug_log = Some(self.sys.writer());
        }

        if self.testing.is_none() {
            self.wm.borrow_mut().ensure_default_backend();
        }

        (self.report_watch_status)(&new_compiler_diagnostic(
            diag::Starting_compilation_in_watch_mode,
            args![],
        ));
        self.watch_set_dirty = true;
        if self.do_build().is_err() {
            self.wm.borrow().force_overflow();
        }
        self.wm.borrow().unlock();

        if self.testing.is_none() {
            // The content mapper host closes itself when ctx is cancelled (see contentmapper.New).
            // PORT: Go passes the method value `w.DoCycle`.
            let wm = Rc::clone(&self.wm);
            wm.borrow().run_loop(ctx, &mut || self.do_cycle());
        }

        // Go: if w.contentMapperHost != nil && w.testing == nil { defer w.contentMapperHost.Close() }
        // PORT: `start` has no early return, so the deferred close runs here.
        if self.testing.is_none() {
            if let Some(host) = &self.content_mapper_host {
                let _ = host.close();
            }
        }
    }

    // Go: execute/watcher.go:172 (*Watcher).replaceContentMapperProject
    fn replace_content_mapper_project(&mut self, config: &ParsedCommandLine) {
        let Some(host) = &self.content_mapper_host else {
            return;
        };
        let project = host.project(contentmapper::ProjectSpec {
            config_file_name: config.config_name().to_string(),
            mappers: config.content_mappers().to_vec(),
            compiler_options: Some(config.compiler_options().clone()),
        });
        if let Some(old) = &self.content_mapper_project {
            let _ = old.close();
        }
        self.content_mapper_project = project;
    }

    // Go: execute/watcher.go:187 (*Watcher).contentMapperWatchedFiles
    fn content_mapper_watched_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for mapper in self.config.content_mappers() {
            if !mapper.package_directory.is_empty() && mapper.contribution_id.is_empty() {
                files.push(combine_paths(&mapper.package_directory, &["package.json"]));
            }
        }
        if let Some(project) = &self.content_mapper_project {
            match project.watched_files() {
                Ok(dynamic_files) => files.extend(dynamic_files),
                Err(err) => {
                    (self.report_diagnostic)(&content_mapper_project_diagnostic(&err));
                    return files;
                }
            }
        }
        files.sort();
        files.dedup();
        files
    }

    /// The paths of `content_mapper_watched_files`, as a set (Go
    /// `collections.NewSetFromItems(core.Map(..., tspath.ToPath))`).
    fn content_mapper_watched_paths(&self, cwd: &str, case_sensitive: bool) -> FxHashSet<Path> {
        self.content_mapper_watched_files()
            .iter()
            .map(|file_name| to_path(file_name, cwd, case_sensitive))
            .collect()
    }

    // Go: execute/watcher.go:207 (*Watcher).computeDesiredWatches
    // PORT: Go ranges over `WildcardDirectories()` (a map, random order).
    pub fn compute_desired_watches(&self, seen_file_paths: &[String]) -> FxHashMap<String, bool> {
        let cwd = self.sys.get_current_directory();

        let mut desired_dirs: FxHashMap<String, bool> = FxHashMap::default(); // dir → recursive

        // Wildcard directories from tsconfig (recursive or non-recursive)
        if self.config.config_file.is_some() {
            for (dir, recursive) in self.config.wildcard_directories() {
                let real_dir = self.sys.fs().realpath(dir);
                desired_dirs.insert(real_dir, *recursive);
            }
        }

        // For no-config CLI mode, ensure CWD is watched
        if self.config.config_file.is_none() && desired_dirs.is_empty() {
            let dir = self.sys.fs().realpath(&cwd);
            desired_dirs.insert(dir, false);
        }

        // Config file parent directories as non-recursive watches
        for cfg_path in &self.config_file_paths {
            let real_path = self.sys.fs().realpath(cfg_path);
            let dir = get_directory_path(&real_path);
            if !desired_dirs.contains_key(&dir) {
                desired_dirs.insert(dir, false);
            }
        }

        // For no-config CLI mode, also watch the CLI-specified files' directories
        if self.config.config_file.is_none() {
            for file_name in self.config.file_names() {
                let abs_path = get_normalized_absolute_path(file_name, &cwd);
                let real_path = self.sys.fs().realpath(&abs_path);
                let dir = get_directory_path(&real_path);
                if !desired_dirs.contains_key(&dir) {
                    desired_dirs.insert(dir, false);
                }
            }
        }

        // Add parent directories for seen files not covered by existing dir watches.
        // Resolve ancestor fallbacks first so coverage checks use final dirs.
        let resolved_dirs = self.wm.borrow().resolve_desired_dirs(&desired_dirs);

        let mut coverage = new_dir_watch_set(self.compare_paths_options());
        for (dir, recursive) in &resolved_dirs {
            coverage.set(dir, *recursive);
        }
        // ts#64366: program files and root files are watched at any depth.
        let program = self.get_program();
        let program_files = program.files_by_path();
        let case_sensitive = self.sys.fs().use_case_sensitive_file_names();
        let root_files: FxHashSet<Path> = self
            .config
            .file_names()
            .iter()
            .map(|file_name| to_path(file_name, &cwd, case_sensitive))
            .collect();
        for file_path in seen_file_paths {
            let dir = get_directory_path(file_path);
            if coverage.covered(&dir) {
                continue;
            }
            // Seen files mix program files with lookup locations. Only lookups keep the depth check, so an imported
            // file outside the tsconfig directory (say /shared next to /app) is still watched. A root file is not in
            // the program while it is missing, but its directory stays watched so that recreating it rebuilds.
            let p = to_path(file_path, &cwd, case_sensitive);
            let is_program_file = program_files.contains_key(&p);
            if is_program_file || root_files.contains(&p) || can_watch_directory(&dir) {
                coverage.set(&dir, false);
            }
        }

        // Re-resolve in case newly added dirs don't exist
        self.wm.borrow().resolve_desired_dirs(&coverage.dirs())
    }

    // Go: execute/watcher.go:201 (*Watcher).reconcileWatches
    pub fn reconcile_watches(&self, seen_file_paths: &[String]) -> Result<(), GoError> {
        let desired_dirs = self.compute_desired_watches(seen_file_paths);
        self.wm.borrow().reconcile_watches(&desired_dirs)
    }

    // Go: execute/watcher.go:206 (*Watcher).comparePathsOptions
    pub fn compare_paths_options(&self) -> ComparePathsOptions {
        ComparePathsOptions {
            use_case_sensitive_file_names: self.sys.fs().use_case_sensitive_file_names(),
            current_directory: self.sys.get_current_directory(),
        }
    }

    // Go: execute/watcher.go:278 (*Watcher).DoCycle
    // PORT: Go unlocks with `defer`; the port unlocks before each return.
    pub fn do_cycle(&mut self) {
        self.wm.borrow().lock();

        let (changed_paths, overflow) = self.wm.borrow().drain_events();
        let has_events = !changed_paths.is_empty() || overflow;

        if self.recheck_ts_config(self.content_mapper_manifest_changed(&changed_paths)) {
            self.wm.borrow().unlock();
            return;
        }

        if has_events && !overflow && !self.config_modified {
            // Filter fswatch events against known dependencies
            if self.is_relevant_change(&changed_paths) {
                self.evict_changed_source_files(&changed_paths);
                let case_sensitive = self.sys.fs().use_case_sensitive_file_names();
                let cwd = self.sys.get_current_directory();
                let program = self.get_program();
                let program_files = program.files_by_path();
                let content_mapper_watched_files =
                    self.content_mapper_watched_paths(&cwd, case_sensitive);
                let mut content_mapper_config_changed = false;
                // PORT: Go ranges over a map (random order). Only flags are set.
                for event_path in changed_paths.keys() {
                    if self.sys.fs().directory_exists(event_path) {
                        // A watched directory changed: the wildcard file set may have
                        // changed, so reload file names on the next build.
                        self.watch_set_dirty = true;
                        continue;
                    }
                    let p = to_path(event_path, &cwd, case_sensitive);
                    if content_mapper_watched_files.contains(&p) {
                        content_mapper_config_changed = true;
                        self.force_full_rebuild = true;
                    }
                    if self.config.config_file.is_some()
                        && self.config.possibly_matches_file_name(event_path)
                    {
                        if !self.seen_files.contains(&p) {
                            // A file that matches the project but was not previously
                            // seen appeared: a structural change that requires a full
                            // rebuild, not the single-file fast path.
                            self.watch_set_dirty = true;
                            self.force_full_rebuild = true;
                            continue;
                        }
                    }
                    if program_files
                        .get(&p)
                        .is_some_and(|source_file| !source_file.content_mapper().is_empty())
                    {
                        // Canonical mapped files must be transformed again, and supplemental paths are failed
                        // physical lookups reserved for virtual files. Neither can use single-file AST reuse.
                        self.force_full_rebuild = true;
                    } else if !program_files.contains_key(&p) && self.seen_files.contains(&p) {
                        // A non-source build dependency changed. Such dependencies
                        // (e.g. package.json or a previously-missing module path) are
                        // tracked in seenFiles but are not program source files, so a
                        // missing sourceFileCache entry would not account for them.
                        // Module resolution may now differ, so the single-file fast
                        // path is unsafe; force a full rebuild.
                        self.force_full_rebuild = true;
                    }
                }
                if content_mapper_config_changed {
                    if let Some(project) = &self.content_mapper_project {
                        if project.refresh().is_err() {
                            (self.report_diagnostic)(&new_compiler_diagnostic(
                                diag::The_content_mapper_process_could_not_be_started_or_initialized,
                                args![],
                            ));
                            self.wm.borrow().unlock();
                            return;
                        }
                    }
                }
            } else {
                if let Some(debug_log) = &self.wm.borrow().debug_log {
                    write_str(
                        debug_log,
                        &format!(
                            "[watch] DoCycle: {} event(s) not relevant to compilation, skipping rebuild\n",
                            changed_paths.len()
                        ),
                    );
                }
                self.on_program();
                self.wm.borrow().unlock();
                return;
            }
        } else if overflow {
            // Overflow: evict the entire source file cache and force a full rebuild.
            // The fast path must not run here: after clearing the cache a one-file
            // program would present exactly one cache miss and be misread as a
            // single-file content edit, silently reusing a stale (e.g. unresolved
            // import) program instead of rediscovering the file graph.
            self.source_file_cache = Rc::new(RefCell::new(FxHashMap::default()));
            self.watch_set_dirty = true;
            self.force_full_rebuild = true;
        } else if !has_events && !self.config_modified {
            // No events and no config change
            if let Some(debug_log) = &self.wm.borrow().debug_log {
                write_str(debug_log, "[watch] DoCycle: no events, skipping\n");
            }
            self.on_program();
            self.wm.borrow().unlock();
            return;
        }

        (self.report_watch_status)(&new_compiler_diagnostic(
            diag::File_change_detected_Starting_incremental_compilation,
            args![],
        ));
        if self.do_build().is_err() {
            // Mid-cycle watch failure; force a full rebuild on the next event
            self.wm.borrow().force_overflow();
        }
        self.wm.borrow().unlock();
    }

    // Go: execute/watcher.go:378 (*Watcher).isRelevantChange
    // PORT: Go map iteration order is random; `changed_paths` is an
    // `FxHashMap`. The result does not depend on the order.
    pub fn is_relevant_change(
        &self,
        changed_paths: &FxHashMap<String, fswatch::EventKind>,
    ) -> bool {
        let case_sensitive = self.sys.fs().use_case_sensitive_file_names();
        let cwd = self.sys.get_current_directory();
        let content_mapper_watched_files = self.content_mapper_watched_paths(&cwd, case_sensitive);
        for event_path in changed_paths.keys() {
            let p = to_path(event_path, &cwd, case_sensitive);
            if content_mapper_watched_files.contains(&p) {
                return true;
            }
            if self.seen_files.contains(&p) {
                return true;
            }
            if self.config.config_file.is_some()
                && self.config.possibly_matches_file_name(event_path)
            {
                return true;
            }
            if self.config.config_file.is_some() && self.config.possibly_matches_directory_name(&p)
            {
                return true;
            }
            if self.sys.fs().directory_exists(event_path)
                && self.wm.borrow().is_path_under_watch(event_path)
            {
                return true;
            }
        }
        false
    }

    // Go: execute/watcher.go:408 (*Watcher).doBuild
    pub fn do_build(&mut self) -> Result<(), GoError> {
        // PORT: the full build below can still use the parses of the old
        // cache (`WatchCompilerHost::reuse_parse`); Go drops them here.
        let mut config_parses = FxHashMap::default();
        if self.config_modified {
            let old = std::mem::replace(
                &mut self.source_file_cache,
                Rc::new(RefCell::new(FxHashMap::default())),
            );
            config_parses = old.take();
            // A file that has diagnostics in the old snapshot is parsed
            // again, as in Go: the new snapshot can copy them, and Go's
            // error summary groups errors by file object
            // (diagnosticwriter.go:479 getErrorSummary). A copy keeps the
            // old object, so Go shows a file whose copied and new errors
            // meet in one build twice.
            if let Some(program) = &self.program {
                let snapshot = program.snapshot.borrow();
                let has_diagnostics = |path: &Path| {
                    [
                        &snapshot.semantic_diagnostics_per_file,
                        &snapshot.emit_diagnostics_per_file,
                    ]
                    .iter()
                    .any(|map| {
                        map.get(path).is_some_and(|entry| {
                            entry.diagnostics.as_ref().is_some_and(|d| !d.is_empty())
                                || !entry.build_info_diagnostics.is_empty()
                        })
                    })
                };
                config_parses.retain(|path, _| !has_diagnostics(path));
            }
            self.watch_set_dirty = true;
        }

        let mut reloaded_file_names = false;
        if self.watch_set_dirty {
            if self.config.config_file.is_some() && !self.config.wildcard_directories().is_empty() {
                let new_config = Rc::new(
                    self.config
                        .reload_file_names_of_parsed_command_line(&*self.sys.fs()),
                );
                reloaded_file_names = true;
                if self.config.file_names() != new_config.file_names() {
                    self.config = new_config;
                } else {
                    self.watch_set_dirty = false;
                    self.config = new_config;
                }
            } else if !self.config_modified {
                self.watch_set_dirty = false;
            }
        }

        if self.program.is_some()
            && self.program_ready
            && !self.config_modified
            && !self.watch_set_dirty
            && !self.force_full_rebuild
        {
            let cached = cachedvfs_from(self.sys.fs());
            let inner_host = new_compiler_host(
                &self.sys.get_current_directory(),
                cached.clone() as Rc<dyn Fs>,
                &self.sys.default_library_path(),
                self.extended_config_cache
                    .clone()
                    .map(|cache| cache as Rc<dyn ExtendedConfigCache>),
                Some(get_trace_from_sys(
                    &*self.sys,
                    self.config.locale(),
                    self.testing.clone(),
                )),
                self.content_mapper_project.clone(),
            );
            let host: Rc<dyn CompilerHost> = Rc::new(WatchCompilerHost {
                compiler_host: inner_host,
                cache: self.source_file_cache.clone(),
                config_parses: RefCell::default(),
            });

            if self.try_update_program(&host) {
                self.fast_path_builds += 1;
                // PORT: the reused program's version is current for the build,
                // as in the full build below.
                let _program =
                    crate::core::enter_program(self.program.as_ref().and_then(|p| p.program));
                let result = self.compile_and_emit();
                cached.disable_and_clear_cache();

                self.config_mtimes = FxHashMap::with_capacity_and_hasher(
                    self.config_file_paths.len(),
                    Default::default(),
                );
                for cfg_path in &self.config_file_paths {
                    if let Some(s) = self.sys.fs().stat(cfg_path) {
                        self.config_mtimes.insert(cfg_path.clone(), s.mod_time());
                    }
                }
                self.config_modified = false;

                let error_count = result.diagnostics.len();
                if error_count == 1 {
                    (self.report_watch_status)(&new_compiler_diagnostic(
                        diag::Found_1_error_Watching_for_file_changes,
                        args![],
                    ));
                } else {
                    (self.report_watch_status)(&new_compiler_diagnostic(
                        diag::Found_0_errors_Watching_for_file_changes,
                        args![error_count],
                    ));
                }
                self.on_program();
                // PORT: the replaced program is freed after the status
                // report (see `retired_program`).
                self.free_retired_program();
                return Ok(());
            }
            cached.disable_and_clear_cache();
        }

        let cached = cachedvfs_from(self.sys.fs());
        let tfs = Rc::new(trackingvfs::FS {
            inner: cached.clone(),
            seen_files: RefCell::new(IndexSet::default()),
        });
        let inner_host = new_compiler_host(
            &self.sys.get_current_directory(),
            tfs.clone() as Rc<dyn Fs>,
            &self.sys.default_library_path(),
            self.extended_config_cache
                .clone()
                .map(|cache| cache as Rc<dyn ExtendedConfigCache>),
            Some(get_trace_from_sys(
                &*self.sys,
                self.config.locale(),
                self.testing.clone(),
            )),
            self.content_mapper_project.clone(),
        );
        let watch_host = Rc::new(WatchCompilerHost {
            compiler_host: inner_host,
            cache: self.source_file_cache.clone(),
            config_parses: RefCell::new(config_parses),
        });
        let host: Rc<dyn CompilerHost> = watch_host.clone();

        if self.config.config_file.is_some() {
            for dir in self.config.wildcard_directories().keys() {
                tfs.seen_files.borrow_mut().insert(dir.clone());
            }
            if !reloaded_file_names
                && !self.watch_set_dirty
                && !self.config.wildcard_directories().is_empty()
            {
                self.config = Rc::new(
                    self.config
                        .reload_file_names_of_parsed_command_line(&*self.sys.fs()),
                );
            }
        }
        for path in &self.config_file_paths {
            tfs.seen_files.borrow_mut().insert(path.clone());
        }
        for path in self.content_mapper_watched_files() {
            tfs.seen_files.borrow_mut().insert(path);
        }

        // Go: compiler.NewProgram(compiler.ProgramOptions{Config, Host})
        // PORT: a new program version, current for the rest of the build.
        // Its new parses of files that an earlier build published are
        // freeable file versions (see `start`).
        let np = new_frontend_program(host.clone(), self.config.clone());
        // The old parses that the program did not take go now, as Go's GC
        // frees them after the new program is made.
        drop(watch_host.config_parses.take());
        crate::program::mark_freeable_parses(&np);
        let version = crate::program::new_program_version(&np, None);
        drop(np);
        let _program = crate::core::enter_program(Some(version));
        let mut program = incremental::program::new_program(
            self.program.as_ref(),
            incremental::incremental::create_host(host),
            Some(self.sys_now()),
            self.testing.is_some(),
        );
        // PORT: Go passes a nil incremental host. The Rust `new_program`
        // takes a host, so the field is cleared after.
        program.host = None;
        // PORT: Go drops the old program here, and its GC frees it later
        // (watcher.go:482). The old checker pool stops without a wait
        // (`release_program_in_background`): the old checkers are freed on
        // the pool threads while this build goes on, and the old tables
        // with the last of them. The rest of the old program
        // (`retire_program`) and the old frontend program are kept until
        // after the status report below, so their free is not in the
        // rebuild time. Its `GoProgram` and the static file versions (the
        // first version of each file) stay leaked; a freeable file version
        // is freed with its last holder (see `start`).
        let released = self
            .program
            .as_ref()
            .and_then(|old| old.program)
            .map(|_| self.get_program());
        self.retire_program(program);
        self.program_ready = true;
        self.full_builds += 1;

        let result = self.compile_and_emit();
        cached.disable_and_clear_cache();

        // PORT: Go's module specifier lookups read package.json files through
        // the program's resolver, whose file system is `tfs`
        // (module/resolver.go:1755 getPackageJsonInfo), so the lookups of
        // this build are seen files. The port's lookups run on the checker
        // threads on the OS file system, and the program's `HostFsCache`
        // keeps their entries (modulespecifiers/host.rs). It is new in each
        // full build, as Go's resolver is. Each entry adds what Go's lookup
        // adds: the package directory (`DirectoryExists`) and, when it
        // exists, its package.json (`FileExists`, `ReadFile`).
        crate::program::with_host_fs_cache(|cache| {
            let mut seen = tfs.seen_files.borrow_mut();
            cache.package_json_entries(|_, package_directory, directory_exists, _| {
                seen.insert(package_directory.to_string());
                if directory_exists {
                    seen.insert(combine_paths(package_directory, &["package.json"]));
                }
                true
            });
        });

        let case_sensitive = self.sys.fs().use_case_sensitive_file_names();
        let cwd = self.sys.get_current_directory();
        let seen_slice: Vec<String> = tfs.seen_files.borrow().iter().cloned().collect();
        self.seen_files = FxHashSet::with_capacity_and_hasher(seen_slice.len(), Default::default());
        for p in &seen_slice {
            self.seen_files.insert(to_path(p, &cwd, case_sensitive));
        }

        self.config_mtimes =
            FxHashMap::with_capacity_and_hasher(self.config_file_paths.len(), Default::default());
        for cfg_path in &self.config_file_paths {
            if let Some(s) = self.sys.fs().stat(cfg_path) {
                self.config_mtimes.insert(cfg_path.clone(), s.mod_time());
            }
        }

        if let Err(err) = self.reconcile_watches(&seen_slice) {
            write_str(&self.sys.writer(), &format!("{}\n", err.error()));
            return Err(err);
        }
        self.watch_set_dirty = false;
        self.config_modified = false;
        self.force_full_rebuild = false;

        // PORT: Go `w.program.GetProgram().FilesByPath()`. `FilesByPath` is on
        // the frontend program of the current program version.
        let program = crate::program::go_frontend_program()
            .expect("the watch build made a Go frontend program");
        let program_files = program.files_by_path();
        self.source_file_cache
            .borrow_mut()
            .retain(|path, _| program_files.contains_key(path));

        let error_count = result.diagnostics.len();
        if error_count == 1 {
            (self.report_watch_status)(&new_compiler_diagnostic(
                diag::Found_1_error_Watching_for_file_changes,
                args![],
            ));
        } else {
            (self.report_watch_status)(&new_compiler_diagnostic(
                diag::Found_0_errors_Watching_for_file_changes,
                args![error_count],
            ));
        }
        drop(released);

        self.on_program();
        self.free_retired_program();
        Ok(())
    }

    /// Makes `program` the watch program. The old checker pool stops now,
    /// without a wait (`release_program_in_background`): the old checkers
    /// are freed on the pool threads while the build goes on, and the old
    /// tables with the last of them. The rest of the old program (its
    /// snapshot) is kept in `retired_program` until the build reports its
    /// status (`free_retired_program`).
    // PORT: Go replaces the program and its GC frees the old one later
    // (watcher.go:482, :567). Stopping the old pool after the status report
    // too saved about 0.7 ms more on a Hono body edit, but the old and the
    // new checkers then live together for the whole rebuild: the watch
    // peak RSS on Hono went from 308 to 397 MiB.
    fn retire_program(&mut self, program: incremental::program::Program) {
        // A build that failed after its program was made did not free the
        // program before it.
        self.free_retired_program();
        if let Some(mut old) = self.program.replace(program) {
            if let Some(version) = old.program.take() {
                crate::program::release_program_in_background(version);
            }
            self.retired_program = Some(old);
        }
    }

    /// Frees the snapshot of the program that the last build replaced,
    /// after the build reported its status, so the free is not in the
    /// rebuild time.
    fn free_retired_program(&mut self) {
        self.retired_program = None;
    }

    // Go: execute/watcher.go:536 (*Watcher).tryUpdateProgram
    // PORT: Go `w.program.GetProgram()` is `get_program`. The parses and the
    // reuse run with no current program, as `program::update_program_version`
    // does. A reused program gets a new program version that shares the
    // unchanged frontend data of the old one; the old version is released as
    // in `do_build`.
    fn try_update_program(&mut self, host: &Rc<dyn CompilerHost>) -> bool {
        let old_version = self
            .program
            .as_ref()
            .and_then(|program| program.program)
            .expect("the watch program has a program");
        let old_program = self.get_program();

        let mut changed_path: Option<&Path> = None;
        let mut changed_count = 0;
        // PORT: Go ranges over a map (random order); at most one path is kept.
        for (path, file) in old_program.files_by_path() {
            if !file.content_mapper().is_empty() {
                continue;
            }
            if !self.source_file_cache.borrow().contains_key(path) {
                changed_path = Some(path);
                changed_count += 1;
                if changed_count > 1 {
                    return false;
                }
            }
        }
        let Some(changed_path) = changed_path else {
            return false;
        };

        if let Some(old_file) = old_program.files_by_path().get(changed_path) {
            if let Some(new_file) = host.get_source_file(old_file.parse_options()) {
                if !equal_jsx_implicit_import(old_program.options(), old_file, &new_file) {
                    return false;
                }
            }
        }

        // PORT: `reuse_program` is Go `Program.ReuseProgram` (tsgo#4399, the
        // program part). The Rust frontend program has no checker pool, so
        // Go's `createCheckerPool` argument (nil here) is dropped, as in
        // `update_program`. Go also passes a nil `createModuleResolver`
        // (ts#64299), so the program keeps its own resolver.
        let (new_program, _, reused) = old_program.reuse_program(changed_path, host.clone(), None);
        if reused {
            let np = Rc::new(new_program.expect("ReuseProgram returns the reused program"));
            // The new parse of the changed file is a freeable file version
            // (see `start`).
            crate::program::mark_freeable_parses(&np);
            let version = crate::program::new_program_version(&np, Some(old_version));
            let _program = crate::core::enter_program(Some(version));
            let mut program = incremental::program::new_program(
                self.program.as_ref(),
                incremental::incremental::create_host(host.clone()),
                Some(self.sys_now()),
                self.testing.is_some(),
            );
            // PORT: Go passes a nil incremental host (see `do_build`).
            program.host = None;
            // PORT: Go replaces the program and its GC frees the old one
            // later (watcher.go:567). The old checker pool stops without a
            // wait, as in `do_build`, and the old snapshot is freed after
            // the status report (`retire_program`).
            self.retire_program(program);
        }
        reused
    }

    // Go: execute/watcher.go:575 (*Watcher).FastPathBuilds
    /// FastPathBuilds reports how many builds reused an existing program via the
    /// UpdateProgram single-file fast path. It is intended for tests that need to
    /// verify which build path was taken.
    pub fn fast_path_builds(&self) -> i32 {
        self.fast_path_builds
    }

    // Go: execute/watcher.go:579 (*Watcher).FullBuilds
    /// FullBuilds reports how many builds constructed a full program via NewProgram.
    /// It is intended for tests that need to verify which build path was taken.
    pub fn full_builds(&self) -> i32 {
        self.full_builds
    }

    /// Go `w.program.GetProgram()`: the frontend program of the watch
    /// program's version.
    // PORT: Go `GetProgram` returns the `*compiler.Program`. Its frontend
    // part (`FilesByPath`, `ReuseProgram`) is the `NewProgram` of the version.
    fn get_program(&self) -> Rc<NewProgram> {
        let version = self.program.as_ref().and_then(|program| program.program);
        let _program = crate::core::enter_program(version);
        crate::program::go_frontend_program().expect("the watch build made a Go frontend program")
    }

    /// Go `w.sys.Now`, the method value.
    fn sys_now(&self) -> incremental::program::NestedEmitNow {
        let sys = self.sys.clone();
        Rc::new(move || sys.now())
    }

    /// Go `if w.testing != nil { w.testing.OnProgram(w.program) }`.
    // PORT: the test reads the program's files, so its version is current
    // for the call.
    fn on_program(&self) {
        let (Some(testing), Some(program)) = (&self.testing, &self.program) else {
            return;
        };
        let _program = crate::core::enter_program(program.program);
        testing.on_program(program);
    }

    // Go: execute/watcher.go:593 (*Watcher).evictChangedSourceFiles
    pub fn evict_changed_source_files(
        &self,
        changed_paths: &FxHashMap<String, fswatch::EventKind>,
    ) {
        let case_sensitive = self.sys.fs().use_case_sensitive_file_names();
        let cwd = self.sys.get_current_directory();
        for event_path in changed_paths.keys() {
            let p = to_path(event_path, &cwd, case_sensitive);
            if self.source_file_cache.borrow().contains_key(&p) {
                if let Some(debug_log) = &self.wm.borrow().debug_log {
                    write_str(
                        debug_log,
                        &format!("[watch] evicting cached source file: {}\n", p.as_str()),
                    );
                }
                self.source_file_cache.borrow_mut().remove(&p);
            }
        }
    }

    // Go: execute/watcher.go:370 (*Watcher).compileAndEmit
    // PORT: `EmitInput.Program` is the current program (see tsc/emit.rs);
    // `do_build` makes the build's version current. Go leaves `WriteFile`
    // nil; see `os_write_file`.
    pub fn compile_and_emit(&self) -> CompileAndEmitResult {
        let program = self.program.as_ref().expect("the watch program is set");
        emit_files_and_report_errors(&EmitInput {
            sys: &*self.sys,
            program_like: program,
            config: Some(&self.config),
            report_diagnostic: self.report_diagnostic.clone(),
            report_error_summary: self.report_error_summary.clone(),
            writer: self.sys.writer(),
            write_file: Some(os_write_file()),
            compile_times: Rc::new(RefCell::new(CompileTimes::default())),
            testing: self.testing.clone(),
            testing_m_times_cache: None,
        })
    }

    // Go: execute/watcher.go:621 (*Watcher).contentMapperManifestChanged
    // PORT: Go looks up the map by key; the order does not matter.
    // ts#64544: the event paths and the manifest compare by path key, so a
    // manifest event with other casing counts on a case-insensitive file
    // system.
    fn content_mapper_manifest_changed(
        &self,
        changed_paths: &FxHashMap<String, fswatch::EventKind>,
    ) -> bool {
        let compare_paths_options = self.compare_paths_options();
        let to_key = |path: &str| {
            to_path(
                path,
                &compare_paths_options.current_directory,
                compare_paths_options.use_case_sensitive_file_names,
            )
        };
        let mut changed_path_keys: Option<FxHashSet<Path>> = None;
        for mapper in self.config.content_mappers() {
            if mapper.package_directory.is_empty() || !mapper.contribution_id.is_empty() {
                continue;
            }
            let changed_path_keys = changed_path_keys
                .get_or_insert_with(|| changed_paths.keys().map(|path| to_key(path)).collect());
            // ts#63936: `package_directory` is already a real path.
            let manifest_path = combine_paths(&mapper.package_directory, &["package.json"]);
            if changed_path_keys.contains(&to_key(&manifest_path)) {
                return true;
            }
        }
        false
    }

    // Go: execute/watcher.go:633 (*Watcher).recheckTsConfig
    pub fn recheck_ts_config(&mut self, force: bool) -> bool {
        if self.config_file_name.is_empty() {
            return false;
        }

        if !force && !self.config_has_errors && !self.config_file_paths.is_empty() {
            let mut changed = false;
            for path in &self.config_file_paths {
                let old_mtime = self.config_mtimes.get(path);
                let s = self.sys.fs().stat(path);
                match old_mtime {
                    None => {
                        if s.is_some() {
                            changed = true;
                            break;
                        }
                    }
                    Some(old_mtime) => {
                        if s.is_none_or(|s| s.mod_time() != *old_mtime) {
                            changed = true;
                            break;
                        }
                    }
                }
            }
            if !changed {
                return false;
            }
        }

        let Some(config_parse_result) = self.parse_config_file() else {
            return true;
        };
        if self.config_has_errors {
            self.config_modified = true;
        }
        self.config_has_errors = false;
        let mut config_file_paths = vec![self.config_file_name.clone()];
        config_file_paths.extend(config_parse_result.extended_source_files().iter().cloned());
        self.config_file_paths = config_file_paths;
        // PORT: ts#64457 makes Go call `ParsedOptions.Equals`
        // (tsoptions/parsedoptions.go:21) in place of `reflect.DeepEqual`.
        // Both are `PartialEq` here (see `ParsedOptions`).
        if self.config.parsed_config != config_parse_result.parsed_config {
            self.config_modified = true;
        }
        self.replace_content_mapper_project(&config_parse_result);
        self.config = config_parse_result;
        false
    }

    // Go: execute/watcher.go:425 (*Watcher).parseConfigFile
    pub fn parse_config_file(&mut self) -> Option<Rc<ParsedCommandLine>> {
        let extended_config_cache = Rc::new(TscExtendedConfigCache::default());
        let (config_parse_result, errors) = get_parsed_command_line_of_config_file(
            &self.config_file_name,
            Some(&*self.compiler_options_from_command_line),
            self.command_line_raw.as_ref(),
            &SystemParseConfigHost(&*self.sys),
            Some(&*extended_config_cache),
        );
        if !errors.is_empty() {
            for e in &errors {
                (self.report_diagnostic)(e);
            }
            self.config_has_errors = true;
            let error_count = errors.len();
            if error_count == 1 {
                (self.report_watch_status)(&new_compiler_diagnostic(
                    diag::Found_1_error_Watching_for_file_changes,
                    args![],
                ));
            } else {
                (self.report_watch_status)(&new_compiler_diagnostic(
                    diag::Found_0_errors_Watching_for_file_changes,
                    args![error_count],
                ));
            }
            return None;
        }
        self.extended_config_cache = Some(extended_config_cache);
        config_parse_result.map(Rc::new)
    }
}

// Go: execute/watcher.go:581 equalJSXImplicitImport
// PORT: the files are frontend `ParsedSourceFile`s (the new one is not
// published), so the base comes from the file loader's
// `get_jsx_implicit_import_base_of_file`, Go `ast.GetJSXImplicitImportBase`
// on the parser's pragmas.
fn equal_jsx_implicit_import(
    options: &CompilerOptions,
    old_file: &ParsedSourceFile,
    new_file: &ParsedSourceFile,
) -> bool {
    let is_jsx = |file: &ParsedSourceFile| {
        file.script_kind == ScriptKind::JSX || file.script_kind == ScriptKind::TSX
    };
    if !is_jsx(old_file) && !is_jsx(new_file) {
        return true;
    }
    let old_import = crate::ast::get_jsx_runtime_import(
        &crate::frontend::compiler::file_loader::get_jsx_implicit_import_base_of_file(
            options, old_file,
        ),
        options,
    );
    let new_import = crate::ast::get_jsx_runtime_import(
        &crate::frontend::compiler::file_loader::get_jsx_implicit_import_base_of_file(
            options, new_file,
        ),
        options,
    );
    old_import == new_import
}
