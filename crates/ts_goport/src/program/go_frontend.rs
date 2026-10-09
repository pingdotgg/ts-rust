//! The goport program loader: Go tsc config parsing
//! (`tsoptions.GetParsedCommandLineOfConfigFile`), the Go program loader
//! (`compiler.NewProgram`: scanner, parser, module resolution, file order)
//! and the `GoProgram` built from its node stores.
//!
//! Go: execute/tsc.go:213 (config), tsc.go:293 (host), tsc.go:301 (program).

use super::*;
use crate::ast::store::{
    file_store_file_name, file_store_parser_flags, publish_file_stores, unpublished_file_ids,
};
use crate::diagnostics::Message;
use crate::frontend::bundled;
use crate::frontend::compiler::{
    NewProgram, ProgramOptions, TraceFn, new_cached_fs_compiler_host, new_program,
    start_default_lib_prefetch, with_loader_state_forgotten,
};
use crate::frontend::parser::{ParsedSourceFile, SourceFileParseOptions};
use crate::frontend::tsoptions::{
    ParseConfigHost, ParsedCommandLine, get_parsed_command_line_of_config_file,
};
use crate::frontend::tspath;
use crate::frontend::tspath::Path as GoPath;
use crate::frontend::vfs::{Fs, osvfs_fs};
use rustc_hash::FxHashSet;
use std::rc::Rc;

/// Thread-safe copies of the Go frontend data that checker code reads.
/// Built once on the loading thread, before any checker exists.
// PORT: Go shares the frontend program between checker goroutines. The
// Rust frontend uses `Rc` and `RefCell`, so the values the checker asks for
// are copied here instead. The module resolutions are the exception: the
// frontend keeps them behind `Arc`, so they are shared.
pub(super) struct GoSharedState {
    /// Go `processedFiles.resolvedModules`: the frontend map itself, which
    /// clones of the frontend program share too.
    // PERF: no copy. A lookup borrows the specifier text
    // (`ModeAwareKey`).
    resolved_modules: Arc<FrontendResolutions>,
    /// Go `processedFiles.jsxRuntimeImportSpecifiers`, by file path.
    jsx_runtime_import_specifiers: FxHashMap<String, (String, Node)>,
    /// Go `processedFiles.importHelpersImportSpecifiers`, by file path.
    import_helpers_import_specifiers: FxHashMap<String, Node>,
    /// Go include processor diagnostics of each program file, by file index.
    include_diagnostics: FxHashMap<usize, Vec<Diagnostic>>,
    /// The frontend `IncludeProcessor::diagnostics_read`: set where Go builds
    /// the include processor diagnostics (`get_include_processor_diagnostics`).
    include_diagnostics_read: Arc<std::sync::atomic::AtomicBool>,
    /// Go `projectReferenceFileMapper.realpathDtsToSource` after the load,
    /// by path: the entries with a source (`get_parse_file_redirect`).
    realpath_dts_to_source: FxHashMap<String, Arc<SourceOutputAndProjectReference>>,
    /// Go `processedFiles.redirectTargetsMap`, by path.
    redirect_targets: FxHashMap<String, Vec<String>>,
    /// Go `GetSourceFileFromReference` of each preserved `/// <reference
    /// path>` of a program file, by file index and reference file name.
    references: FxHashMap<(usize, String), Node>,
    /// Go `processedFiles.outputFileToProjectReferenceSource`, by path.
    output_file_to_project_reference_source: FxHashMap<String, String>,
    /// Go `projectReferenceFileMapper.sourceToProjectReference`, by path.
    source_to_project_reference: FxHashMap<String, Arc<SourceOutputAndProjectReference>>,
    /// Go `projectReferenceFileMapper.outputDtsToProjectReference`, by path.
    output_dts_to_project_reference: FxHashMap<String, Arc<SourceOutputAndProjectReference>>,
    /// Go `projectReferenceFileMapper.opts.canUseProjectReferenceSource()`.
    can_use_project_reference_source: bool,
    /// Go `GetRedirectForResolution` of each program file, by file index.
    /// A file with no redirect has no entry.
    redirects_for_resolution: FxHashMap<usize, Arc<ResolvedProjectReference>>,
    /// Go `GetResolvedProjectReferences`.
    resolved_project_references: Vec<Option<Arc<ResolvedProjectReference>>>,
    /// Go `Program.knownSymlinks`: set on first use (`known_symlinks`).
    /// Shared with the version this one was updated from when the frontend
    /// shares the cache (Go `tryReuse`).
    known_symlinks: OnceLock<Arc<crate::modulespecifiers::symlinks::KnownSymlinks>>,
    /// Go `processedFiles.sourceFilesFoundSearchingNodeModules`, by path.
    /// Shared like `resolved_modules`.
    source_files_found_searching_node_modules: Arc<FxHashSet<String>>,
    /// Go `Program.hasEmitBlockingDiagnostics`, by path.
    has_emit_blocking_diagnostics: FxHashSet<String>,
    /// Go `Program.toPath` inputs.
    current_directory: String,
    use_case_sensitive_file_names: bool,
    /// #4712: Go `Program.ContentMapperExtensions` (the extensions of the
    /// command line).
    content_mapper_extensions: Vec<String>,
    /// #4712: Go `Program.contentMapperOptionDiagnostics`.
    // PORT: `NewProgram` has no field for it, so the program version keeps
    // it (see `GoSharedState::new`).
    content_mapper_option_diagnostics: Vec<Diagnostic>,
    /// Not in Go: true when the tables of this version started from those
    /// of the version it was updated from (`reused_tables`), false when
    /// they were built from its files alone (`full_tables`). Tests read it
    /// (`ls_program::version_tables_reused`).
    pub(super) from_old_tables: bool,
}

type FrontendSourceOutput = crate::frontend::tsoptions::SourceOutputAndProjectReference;

/// `GoSharedState::resolved_modules`.
type FrontendResolutions =
    FxHashMap<GoPath, crate::frontend::module::ModeAwareCache<Arc<ResolvedModule>>>;

/// Thread-safe copies of the frontend project references. Go shares one
/// `*ParsedCommandLine` per referenced project, so each is copied once.
/// PORT: Go reads `CommonSourceDirectory` of a reference only when it needs
/// it (output names with outDir or declarationDir, the checker for
/// `rewriteRelativeImportExtensions`), and that call records TS6059 errors
/// on the reference. The copy needs the value now, so it reads it without
/// the check (`common_source_directory_unchecked`). The checker's call on
/// the copy marks the reference (`common_source_directory_read`), and the
/// next reader of the reference's errors records them, as Go does from the
/// checker thread.
#[derive(Default)]
struct ProjectReferenceCopies {
    resolved: FxHashMap<*const ParsedCommandLine, Arc<ResolvedProjectReference>>,
}

impl ProjectReferenceCopies {
    fn resolved(&mut self, parsed: &Rc<ParsedCommandLine>) -> Arc<ResolvedProjectReference> {
        self.resolved
            .entry(Rc::as_ptr(parsed))
            .or_insert_with(|| {
                Arc::new(ResolvedProjectReference::new(
                    (**parsed.compiler_options()).clone(),
                    parsed.common_source_directory_unchecked(),
                    parsed.common_source_directory_read.clone(),
                ))
            })
            .clone()
    }

    fn entry(&mut self, entry: &FrontendSourceOutput) -> Arc<SourceOutputAndProjectReference> {
        let parsed = entry
            .resolved
            .upgrade()
            .expect("the referenced project command line is alive");
        Arc::new(SourceOutputAndProjectReference {
            source: entry.source.clone(),
            output_dts: entry.output_dts.clone(),
            resolved: self.resolved(&parsed),
        })
    }
}

thread_local! {
    /// Go `SourceFile.jsdocCache` entries added by lazy JSDoc parses on this
    /// thread. PORT: the parsed file is shared and read only, so the lazy
    /// entries live here. The slices are leaked to give `NodeSlice` a static
    /// borrow. The parsed nodes are synthetic nodes of this thread, so each
    /// thread keeps its own entries (see `WorkerSeed`). The entries of a
    /// dead file version go, and so do its JSDoc nodes (a file scope,
    /// `resolve_lazy_js_doc`).
    static LAZY_JSDOC: RefCell<PerFileMap<&'static [Node]>> = const { RefCell::new(PerFileMap::new()) };
    /// The file system of a checker worker thread (Go `host.FS()` without
    /// the cache; the program's `HostFsCache` caches its answers).
    static WORKER_FS: Rc<dyn Fs> = bundled::wrap_fs(osvfs_fs());
}

/// The lazy JSDoc entries of this thread, to seed a checker worker.
pub(super) fn lazy_jsdoc_seed() -> PerFileMap<&'static [Node]> {
    LAZY_JSDOC.with(|cache| {
        let mut cache = cache.borrow_mut();
        // The copy has no entries of dead file versions.
        cache.write();
        cache.clone()
    })
}

/// The number of lazy JSDoc entries on this thread.
pub(super) fn lazy_jsdoc_count() -> usize {
    LAZY_JSDOC.with(|cache| cache.borrow().len())
}

/// Installs the entries of `lazy_jsdoc_seed` on a new checker worker.
pub(super) fn install_lazy_jsdoc_seed(seed: PerFileMap<&'static [Node]>) {
    LAZY_JSDOC.with(|cache| *cache.borrow_mut() = seed);
}

/// Go `tsc.System` as `ParseConfigHost` (FS and current directory).
struct System {
    fs: Rc<dyn Fs>,
    current_directory: String,
}

impl ParseConfigHost for System {
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }
    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }
}

/// `try_load_with` for the Go frontend.
pub(super) fn try_load_with(
    config_path: &str,
    edit_options: impl FnOnce(&mut CompilerOptions),
    times: &mut CompileTimes,
) -> Result<&'static GoProgram, String> {
    let opts = load_config(config_path, edit_options, times)?;
    // Go: tsc.go:298 startTracingIfNeeded. PORT: the warning goes to stdout
    // (Go `sys.Writer()`) as Go bytes, like `trace_from_sys`. A program
    // version (`try_load_version`) does not trace, as Go tsc starts tracing
    // only here.
    if let Some(warning) = crate::tracing::start_tracing_if_needed(&opts.config, false) {
        let _ = crate::execute::tsc::write_go_output(
            &mut crate::execute::tsc::stdio::CliStdout,
            warning.as_bytes(),
        );
    }
    // Go: tsc.go:305 times `NewProgram`. PORT: the port's `NewProgram` is
    // `install_new_program`, which also builds the Go files, as the build
    // task times `new_program` and `new_program_version`.
    let parse_start = std::time::Instant::now();
    let program = install_new_program(opts);
    times.parse_time = parse_start.elapsed();
    program
}

/// `try_load_version` (program.rs).
pub(super) fn try_load_version(
    config_path: &str,
    edit_options: impl FnOnce(&mut CompilerOptions),
) -> Result<&'static GoProgram, String> {
    // A new version parses with no current program, like the first load.
    let _scope = crate::core::enter_program(None);
    let cwd = current_directory()?;
    let opts = load_config(config_path, edit_options, &mut CompileTimes::default())?;
    let np = Rc::new(new_program(opts));
    Ok(build_program(&np, Entry::Version, cwd, None))
}

/// `update_program_version` (program.rs).
pub(super) fn update_program_version(
    old: &'static GoProgram,
    changed_file: &str,
) -> (&'static GoProgram, bool) {
    let old_np = FRONTENDS
        .with(|frontends| frontends.borrow().get(&old.id).cloned())
        .expect("the old program version has no frontend on this thread");
    let cwd = state_of(old).cwd.to_string();
    // A new version parses with no current program, like the first load.
    let _scope = crate::core::enter_program(None);
    // PORT: Go watch gives `UpdateProgram` a host whose cache no longer has
    // the changed file. A new cached host over the OS file system reads it
    // again.
    let host_cwd = old_np.get_current_directory();
    // #4712: no content mapper project (see `load_config`).
    let host = new_cached_fs_compiler_host(
        &host_cwd,
        bundled::wrap_fs(osvfs_fs()),
        &bundled::lib_path(),
        None,
        Some(trace_from_sys()),
        None,
    );
    let changed_path = crate::frontend::tspath::to_path(
        changed_file,
        &host_cwd,
        old_np.use_case_sensitive_file_names(),
    );
    let (np, _, reused) = old_np.update_program(&changed_path, host, None);
    mark_freeable_parses(&np);
    let np = Rc::new(np);
    (build_program(&np, Entry::Version, cwd, Some(old)), reused)
}

/// Gives each new parse of `np` (its store is not published) of a path
/// that this thread published before a `FileVersion`, as the language
/// server parse cache does (`ast::freeable_path`). `tsc --watch` calls it
/// for each build (`program::mark_freeable_parses`), and
/// `update_program_version` (`goport_multiprog`) too, where the rule is off
/// unless `GOPORT_FREE_FILE_VERSIONS=1`, so a measurement can turn it on
/// (lsshells M3b). A parse that is not a program file gets none: it is
/// published static.
pub(super) fn mark_freeable_parses(np: &NewProgram) {
    if !crate::ast::free_file_versions() {
        return;
    }
    for file in np.get_source_files() {
        if file.version.get().is_none()
            && !crate::ast::is_published(file.store)
            && crate::ast::freeable_path(&file.path().0)
        {
            let _ = file.version.set(crate::ast::FileVersion::new(file.store));
        }
    }
}

// Go: compiler/program.go:2330 GetSymlinkCache (`get_go_symlink_cache`)
// PORT: Go builds the cache on first use, on any goroutine. The port
// builds it from the frontend program, which only the loading thread has,
// and keeps a thread-safe copy in the program's shared state. The loading
// thread builds it on first use, or when it seeds a thread
// (`build_lazy_shared_state`), so a seeded thread finds it built.
pub(super) fn known_symlinks() -> Arc<crate::modulespecifiers::symlinks::KnownSymlinks> {
    if let Some(built) = with_go(GoSharedState::built_known_symlinks) {
        return built;
    }
    let id = prog().id;
    let frontend = FRONTENDS
        .with(|frontends| frontends.borrow().get(&id).cloned())
        .expect("the symlink cache is built on the loading thread (`build_lazy_shared_state`)");
    // PORT: the parallel bind seeds threads while the project host still
    // tracks its file system calls, before Go makes any use of the cache. A
    // Go first use after the host is frozen (project/compilerhost.go:57)
    // makes no tracked calls. So when the frontend has not built its value,
    // the copy is built with the tracking paused, and the frontend's own
    // value is left for its first use (the auto-import registry, which can
    // come before the freeze, as in Go). Else a node_modules/<dependency>
    // Created event marks the project dirty where Go only clones the
    // snapshot (editfuzz3 H3).
    let copy = match frontend.symlink_cache_if_built() {
        Some(built) => Arc::new((*built).clone()),
        None => {
            let mut built = None;
            frontend
                .host()
                .without_fs_tracking(&mut || built = Some(frontend.build_symlink_cache()));
            Arc::new(built.expect("without_fs_tracking runs its callback"))
        }
    };
    with_go(|go| Arc::clone(go.known_symlinks.get_or_init(|| copy)))
}

/// Builds the shared state values that the current program makes on first
/// use (`known_symlinks`), when this thread loaded it: a thread seeded from
/// this one (`WorkerSeed`) cannot build them. Does nothing on another
/// thread, which was seeded after they were built, or for an alias
/// resolver program.
pub(super) fn build_lazy_shared_state() {
    let id = prog().id;
    if FRONTENDS.with(|frontends| frontends.borrow().contains_key(&id)) {
        let _ = known_symlinks();
    }
}

/// `new_program_version` (program.rs).
pub(super) fn new_program_version(
    np: &Rc<NewProgram>,
    previous: Option<&'static GoProgram>,
) -> &'static GoProgram {
    // The files are built and published with no current program, like the
    // first load. `build_program` makes the new version current for its
    // state.
    let _scope = crate::core::enter_program(None);
    build_program(np, Entry::Version, np.get_current_directory(), previous)
}

thread_local! {
    /// Source files that were parsed on this thread outside a program load
    /// (the language server parse cache) and are not published yet, by
    /// store id. A program that does not include such a file still
    /// publishes it, with its parser fields, so a later program can share it.
    static PARSED_UNPUBLISHED: RefCell<FxHashMap<usize, Rc<ParsedSourceFile>>> =
        RefCell::new(FxHashMap::default());

    /// The parses of `PARSED_UNPUBLISHED` that a publish kept for good
    /// (`go_files_of_unpublished_stores`), by store id. A store here is
    /// published, so `unpublished_parsed_source_file` no longer finds it.
    // PORT: bump B 2c config hand-off (api ext battery, `getConfigSourceFile`
    // event 3): the project system parses the root config, and a program
    // version publishes its store before the api request encodes it.
    static PUBLISHED_OUTSIDE: RefCell<FxHashMap<usize, &'static Rc<ParsedSourceFile>>> =
        RefCell::new(FxHashMap::default());
}

/// `note_parsed_source_file` (program.rs).
pub(super) fn note_parsed_source_file(file: &Rc<ParsedSourceFile>) {
    if !crate::ast::is_published(file.store) {
        PARSED_UNPUBLISHED.with(|parsed| parsed.borrow_mut().insert(file.store, file.clone()));
    }
}

/// The parse that `note_parsed_source_file` recorded for store `store` on
/// this thread, while the store is not published.
pub(super) fn unpublished_parsed_source_file(store: usize) -> Option<Rc<ParsedSourceFile>> {
    PARSED_UNPUBLISHED.with(|parsed| parsed.borrow().get(&store).cloned())
}

/// The parse of store `store` that a publish kept for good: a file parsed
/// on this thread outside a program load (`note_parsed_source_file`) that
/// a publish gave no program. None for any other store, and for a freeable
/// file version (`go_files_of_unpublished_stores` keeps no parse of it).
pub(super) fn published_outside_parsed_source_file(store: usize) -> Option<Rc<ParsedSourceFile>> {
    PUBLISHED_OUTSIDE.with(|kept| kept.borrow().get(&store).map(|&file| Rc::clone(file)))
}

/// `publish_parsed_files` (program.rs).
pub(super) fn publish_parsed_files(cwd: &str) {
    let parsed = FxHashMap::default();
    let files =
        go_files_of_unpublished_stores(&parsed, cwd, osvfs_fs().use_case_sensitive_file_names());
    publish_file_stores(files);
}

// Go: ast/ast.go:2747 (*SourceFile).resolveJSDoc (slow path; the caller
// has checked the parser cache) and parser/jsdoc.go:19 parseJSDocForNode
/// The lazy JSDoc of `node` in published file `file`, whose info is
/// `info`. The result is cached in `LAZY_JSDOC`.
// PORT: as in Go, the parser inputs come from the file itself: Go
// `ParseOptions()` (the name, path and module indicator options of
// `info`), `ScriptKind` and `Text()` (the store text). So every file that
// a node handle can read parses its JSDoc, with or without a program: a
// file of the current program, of an alias resolver program, or of a
// released program version that only the parse cache keeps (editfuzz2
// P2-1, which reached `unported!` when no live program had the file).
pub(super) fn resolve_lazy_js_doc(
    file: Node,
    info: &SourceFileInfo,
    node: Node,
) -> &'static [Node] {
    if let Some(jsdocs) = cached_lazy_js_doc(node) {
        return jsdocs;
    }
    let parse_options = SourceFileParseOptions {
        file_name: info.file_name.clone(),
        path: GoPath(info.path.clone()),
        external_module_indicator_options: info.external_module_indicator_options,
    };
    let text = crate::ast::file_store_text(file.file_index());
    // The cache outlives program versions and keeps the nodes as long as
    // `node` lives: they belong to its file version, or to the thread.
    let _file = crate::ast::enter_file_synthetic_owner(node);
    let jsdocs: &'static [Node] = Box::leak(
        crate::frontend::parser::parse_js_doc_for_node(
            &parse_options,
            &text,
            info.script_kind,
            node,
        )
        .into_boxed_slice(),
    );
    LAZY_JSDOC.with(|cache| cache.borrow_mut().write().insert(node, jsdocs));
    jsdocs
}

/// The entry that `resolve_lazy_js_doc` cached for `node` on this thread,
/// or `None`. It never parses (Go `EagerJSDoc` reads `jsdocCache`, which
/// holds such entries; `Node::eager_js_doc`).
pub(crate) fn cached_lazy_js_doc(node: Node) -> Option<&'static [Node]> {
    LAZY_JSDOC.with(|cache| cache.borrow().get(&node).copied())
}

// Go: compiler/program.go:145 FileExists (the Go frontend program)
// PORT: the loading thread asks the program host (with its cache). Another
// thread (a checker worker) cannot read that cache. It asks the program's
// thread-safe cache (Go `cachedvfs.FS`, compiler/host.go:52), which asks
// this thread's copy of the file system on a miss. Module specifier
// generation probes the same paths for many types (`tryGetAnyFileFromPath`):
// realworld3 lexical made 8.6M uncached stat calls, Go 8,768.
pub(super) fn file_exists(path: &str) -> bool {
    let id = prog().id;
    if let Some(go) = FRONTENDS.with(|frontends| frontends.borrow().get(&id).cloned()) {
        return go.file_exists(path);
    }
    super::with_tables(|tables| {
        tables
            .host_fs_cache
            .file_exists(path, || WORKER_FS.with(|fs| fs.file_exists(path)))
    })
}

/// The Go files of this thread's unpublished stores, in store id order
/// (`publish_file_stores`). `parsed` holds the program files by store id.
/// A store that is not a program file is a file that the language server
/// parsed outside a program load (`PARSED_UNPUBLISHED`), or a config file.
/// Each parse that gets its Go file here is kept for good, unless it is a
/// freeable file version (see the PERF note below).
fn go_files_of_unpublished_stores(
    parsed: &FxHashMap<usize, &Rc<ParsedSourceFile>>,
    cwd: &str,
    use_case_sensitive_file_names: bool,
) -> Vec<GoFile> {
    let outside = PARSED_UNPUBLISHED.with(|outside| std::mem::take(&mut *outside.borrow_mut()));
    // `files[i]` is the GoFile of store `unpublished_file_ids().start + i`.
    let stores = unpublished_file_ids();
    let mut files = Vec::with_capacity(stores.len());
    for store in stores {
        // PERF: the `SourceFileInfo` of a static file borrows lists of the
        // parse. A static published file is never freed, so its parse is
        // kept for good, whether a program or the parse cache made it. The
        // frontend program is freed with its last holder; this keeps only
        // the parses. A file that an earlier publish gave its Go file keeps
        // the parse of that publish. A freeable file version (lsshells M3a)
        // keeps no parse: the parse holds the version, so a kept parse would
        // keep it alive. Its `SourceFileInfo` owns copies of the lists
        // instead, which are freed with the version (M3b). A freeable parse
        // can also be outside a program load (`is_outside`): the parse
        // cache notes its parses in `PARSED_UNPUBLISHED`. It also gets
        // `KeptLists::copied` and no `PUBLISHED_OUTSIDE` entry, so after
        // its publish `parsed_source_file` returns None for it (bump B kept
        // every outside parse). That is correct: keeping it would leak the
        // version. Only a static outside parse goes to `PUBLISHED_OUTSIDE`.
        let is_outside = !parsed.contains_key(&store);
        let file = parsed.get(&store).copied().or_else(|| outside.get(&store));
        let info = match file {
            Some(file) => {
                crate::ast::note_published_path(&file.path().0);
                let lists = if file.version.get().is_some() {
                    KeptLists::copied(file)
                } else {
                    let kept: &'static Rc<ParsedSourceFile> = Box::leak(Box::new(Rc::clone(file)));
                    if is_outside {
                        PUBLISHED_OUTSIDE
                            .with(|published| published.borrow_mut().insert(store, kept));
                    }
                    KeptLists::borrowed(kept)
                };
                program_file_info(store, file, lists)
            }
            None => other_store_info(store, cwd, use_case_sensitive_file_names),
        };
        // PORT: a store that is not a parsed source file (a config file) is
        // never bound or checked, so its root is not read.
        let root = file.map_or(Node::NIL, |file| file.root);
        files.push(GoFile {
            root,
            parser_flags: file_store_parser_flags(store),
            info,
            node_bind: OnceLock::new(),
            file_bind: OnceLock::new(),
            flow_nodes: OnceLock::new(),
        });
    }
    files
}

/// The process current directory, normalized. The error is Go's
/// `os.Getwd` text (`*os.SyscallError` "getwd: <errno text>").
fn current_directory() -> Result<String, String> {
    let cwd = crate::frontend::vfs::os_current_dir()
        .map_err(|e| format!("getwd: {}", crate::fswatch::syscall::io_error_text(&e)))?;
    Ok(tspath::normalize_path(&cwd))
}

/// Go tsc config parsing and compiler host (tsc.go:213 and :293): the
/// program options of a load. It records the config time in `times`.
fn load_config(
    config_path: &str,
    edit_options: impl FnOnce(&mut CompilerOptions),
    times: &mut CompileTimes,
) -> Result<ProgramOptions, String> {
    let cwd = current_directory()?;
    // Go: sys.FS() is bundled.WrapFS(osvfs.FS()).
    let fs = bundled::wrap_fs(osvfs_fs());
    let mut config_abs = tspath::resolve_path(&cwd, &[config_path]);
    // Go tsc `-p <dir>` reads `<dir>/tsconfig.json`.
    if fs.directory_exists(&config_abs) {
        config_abs = tspath::combine_paths(&config_abs, &["tsconfig.json"]);
    }

    // Go: tsc.go:214 GetParsedCommandLineOfConfigFile with the command line
    // options. PORT: the command line raw map only marks explicit nulls,
    // and the command line here has none, so it is nil.
    let mut command_line_options = CompilerOptions::default();
    edit_options(&mut command_line_options);
    let sys = System {
        fs: fs.clone(),
        current_directory: cwd.clone(),
    };
    let config_start = std::time::Instant::now();
    let (config, errors) = get_parsed_command_line_of_config_file(
        &config_abs,
        Some(&command_line_options),
        None,
        &sys,
        None,
    );
    times.config_time = config_start.elapsed();
    if !errors.is_empty() {
        // Go reports these unrecoverable errors and exits.
        let messages: Vec<String> = errors
            .iter()
            .map(|d| {
                format!(
                    "error TS{}: {}",
                    d.code,
                    d.localize(&crate::locale::DEFAULT)
                )
            })
            .collect();
        return Err(messages.join("\n"));
    }
    let config = config.ok_or_else(|| format!("cannot parse {config_abs}"))?;
    // PERF: the default lib parses start now, before the host and the
    // loader exist (`start_default_lib_prefetch`).
    start_default_lib_prefetch(
        &config,
        &cwd,
        fs.use_case_sensitive_file_names(),
        &bundled::lib_path(),
    );

    // Go: tsc.go:302 NewCachedFSCompilerHost, tsc.go:310 NewProgram.
    // #4712: the 6th argument is Go `contentMapperProject` (tsc.go:298
    // `getContentMapperProject(tsc.NewContentMapperHost(ctx, sys, options),
    // config)`).
    // PORT: Go makes a content mapper host only with `runExternalCode`, and
    // the host spawns the mapper processes through `sys.Spawn`. The port has
    // no OS spawner here, so the host has no content mapper project: Go's
    // value without `runExternalCode`. A content-mapped file then fails its
    // transform with `ErrProjectUnavailable`, as in Go without that option.
    let host = new_cached_fs_compiler_host(
        &cwd,
        fs,
        &bundled::lib_path(),
        None,
        Some(trace_from_sys()),
        None,
    );
    Ok(ProgramOptions {
        host,
        config: Rc::new(config),
        use_source_of_project_reference: false,
        single_threaded: Tristate::Unknown,
        typings_location: String::new(),
        project_name: String::new(),
        create_module_resolver: None,
        skip_module_resolution: false,
    })
}

/// Go `compiler.NewProgram` for a config that is already parsed. It builds
/// the Go files and installs the program for the process. Call it once.
pub(super) fn install_new_program(opts: ProgramOptions) -> Result<&'static GoProgram, String> {
    let cwd = current_directory()?;
    // PERF: the program of a one-program process is never freed, so its
    // loader state is not freed either (`with_loader_state_forgotten`). The
    // forgotten clone keeps the program when `FRONTENDS` drops at process
    // exit, so the exit does not free it either.
    let new_program = Rc::new(with_loader_state_forgotten(|| new_program(opts)));
    std::mem::forget(Rc::clone(&new_program));
    Ok(build_program(&new_program, Entry::Only, cwd, None))
}

/// How `build_program` makes a program known.
enum Entry {
    /// The program of a one-program process (`core::set_prog`).
    Only,
    /// A program version of a multi-program process
    /// (`core::register_program_version`). The caller parses and builds it
    /// inside `core::enter_program(None)`.
    Version,
}

/// Builds the Go files of the stores that `np` parsed, publishes them and
/// makes the `GoProgram` of `np` with its state. Program files that an
/// earlier version published keep their `GoFile`. `cwd` is the current
/// directory of the process. `previous` is the version that `np` was
/// updated from, if it is still loaded; the new state shares its copies of
/// unchanged frontend data (`GoSharedState::new`). When `np` replaced files
/// of `previous` in place (Go `ReuseProgram`), the new tables start from
/// its tables (`reused_tables`). When `np` keeps the frontend resolver of
/// `previous`, the new tables share its package.json cache
/// (`VersionTables::package_json_cache`).
fn build_program(
    np: &Rc<NewProgram>,
    entry: Entry,
    cwd: String,
    previous: Option<&'static GoProgram>,
) -> &'static GoProgram {
    let use_case_sensitive_file_names = osvfs_fs().use_case_sensitive_file_names();
    let options = crate::program::intern_compiler_options(np.options());
    let files = np.source_files();
    let previous = previous.and_then(previous_version);
    let replaced = previous
        .as_ref()
        .and_then(|old| replaced_files(np, &old.np, &old.tables));
    let check = check_version_tables() && replaced.is_some();

    // PERF: the publish keeps the parse of each new file, so each
    // `SourceFileInfo` can borrow its fields instead of copying them
    // (`go_files_of_unpublished_stores`). Only a program file that no
    // publish has seen is new. The old version published every file it
    // shares, so with replaced files only those can be new.
    fn new_files<'a>(
        files: impl Iterator<Item = &'a Rc<ParsedSourceFile>>,
    ) -> FxHashMap<usize, &'a Rc<ParsedSourceFile>> {
        let unpublished = unpublished_file_ids();
        files
            .filter(|file| unpublished.contains(&file.store))
            .map(|file| (file.store, file))
            .collect()
    }
    let parsed = match &replaced {
        Some(replaced) => new_files(replaced.iter().map(|r| r.file)),
        None => new_files(files.iter()),
    };
    if check {
        let all = new_files(files.iter());
        assert!(
            all.keys().all(|store| parsed.contains_key(store)),
            "a program file that no publish has seen was not replaced"
        );
    }
    let go_files = go_files_of_unpublished_stores(&parsed, &cwd, use_case_sensitive_file_names);

    let id = next_program_id();
    FRONTENDS.with(|frontends| {
        assert!(
            frontends.borrow_mut().insert(id, Rc::clone(np)).is_none(),
            "program {id} already loaded"
        );
    });
    // The stores become read-only here, before the program is installed.
    publish_file_stores(go_files);
    // A program file must be published as a source file. A parse that an
    // earlier publish did not know (see `note_parsed_source_file`) became a
    // store with no root there, and its bind would fail far from the cause.
    // The build of the old version checked the files that a new version
    // shares with it.
    let assert_published = |file: &Rc<ParsedSourceFile>| {
        assert!(
            crate::ast::go_file(file.store).root.is_some(),
            "program file {} was published as a store that is not a source file",
            file.file_name()
        );
    };
    match &replaced {
        Some(replaced) if !check => replaced.iter().for_each(|r| assert_published(r.file)),
        _ => files.iter().for_each(assert_published),
    }
    let program: &'static GoProgram = Box::leak(Box::new(GoProgram {
        id,
        options,
        state: OnceLock::new(),
    }));
    let one_program = match entry {
        Entry::Only => {
            set_prog(program);
            true
        }
        Entry::Version => {
            register_program_version(program);
            false
        }
    };
    // The tables are built with the program current, like the lazy Go
    // reads they replace.
    let _scope = (!one_program).then(|| crate::core::enter_program(Some(program)));
    let previous_shared = previous
        .as_ref()
        .and_then(|old| Some((&*old.np, old.tables.go.as_ref()?)));
    let (mut tables, common_source_directory) = match (&previous, &replaced) {
        (Some(old), Some(replaced)) => {
            let tables = reused_tables(np, old, replaced);
            // Go computes it from the file names and the options, which a
            // program that replaces files in place keeps.
            let common_source_directory = old.common_source_directory;
            if check {
                let full = full_tables(np, previous_shared);
                assert_same_tables(&tables, &full);
                assert_eq!(
                    common_source_directory,
                    common_source_directory_of(np),
                    "common source directory"
                );
            }
            (tables, common_source_directory)
        }
        _ => {
            let tables = full_tables(np, previous_shared);
            let common_source_directory = intern_program_str(&common_source_directory_of(np));
            (tables, common_source_directory)
        }
    };
    // Go: compiler/program.go:359 ReuseProgram keeps `processedFiles`, so
    // the new program keeps the resolver and the package.json lookups of
    // module specifier generation in its cache (program.go:147-169).
    if let Some(old) = &previous
        && let (Some(resolver), Some(old_resolver)) = (&np.resolver, &old.np.resolver)
        && Rc::ptr_eq(resolver, old_resolver)
    {
        tables.package_json_cache = Arc::clone(&old.tables.package_json_cache);
    }
    if check_version_tables() {
        tables
            .go
            .as_ref()
            .expect("a Go frontend program has shared state")
            .assert_include_diagnostics(np);
    }
    drop(replaced);
    drop(previous);
    let program_state = ProgramState {
        cwd: intern_program_str(&cwd),
        use_case_sensitive_file_names,
        common_source_directory: Some(common_source_directory),
        alias_resolver: false,
        tables: TablesSlot::new(tables, one_program),
    };
    assert!(program.state.set(program_state).is_ok());
    program
}

/// True in debug builds, or when `GOPORT_CHECK_VERSION_TABLES=1`:
/// `build_program` builds the tables of a version that starts from the
/// tables of an older one (`reused_tables`) again from its files alone
/// (`full_tables`), and panics when they differ. Read once.
fn check_version_tables() -> bool {
    static CHECK: OnceLock<bool> = OnceLock::new();
    *CHECK.get_or_init(|| {
        cfg!(debug_assertions)
            || std::env::var("GOPORT_CHECK_VERSION_TABLES").is_ok_and(|v| v == "1")
    })
}

/// The version that a new program version was updated from, while it is
/// loaded on this thread (`previous_version`).
struct PreviousVersion {
    np: Rc<NewProgram>,
    tables: Arc<VersionTables>,
    common_source_directory: &'static str,
}

/// `old` as a `PreviousVersion`, or None when it is not a Go frontend
/// program version that this thread loaded, or when it is released.
fn previous_version(old: &'static GoProgram) -> Option<PreviousVersion> {
    let np = FRONTENDS.with(|frontends| frontends.borrow().get(&old.id).cloned())?;
    let state = old.state.get()?;
    let TablesSlot::Version(slot) = &state.tables else {
        return None;
    };
    let tables = slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    tables.go.as_ref()?;
    Some(PreviousVersion {
        np,
        tables,
        common_source_directory: state.common_source_directory?,
    })
}

/// A program file that a new program version put in the slot of a file of
/// the version it was updated from (Go `ReuseProgram`).
struct Replaced<'a> {
    slot: usize,
    old: &'a Rc<ParsedSourceFile>,
    file: &'a Rc<ParsedSourceFile>,
}

/// The files of `np` that are not those of `old_np`, when `np` replaced
/// them in place (Go `ReuseProgram`): the same number of files at the same
/// paths in the same order, and the same resolutions. `old` holds the tables
/// of `old_np`. None when `np` is not such a program, or when a replaced
/// file is in a package redirect group.
fn replaced_files<'a>(
    np: &'a NewProgram,
    old_np: &'a NewProgram,
    old: &VersionTables,
) -> Option<Vec<Replaced<'a>>> {
    let (files, old_files) = (np.source_files(), old_np.source_files());
    let reused = Arc::ptr_eq(
        &np.processed_files.resolved_modules,
        &old_np.processed_files.resolved_modules,
    ) && files.len() == old_files.len()
        && old.source_file_order.len() == files.len()
        && np.files_by_path().len() == old_np.files_by_path().len();
    if !reused {
        return None;
    }
    let mut replaced = Vec::new();
    for (slot, (file, old_file)) in files.iter().zip(old_files).enumerate() {
        if Rc::ptr_eq(file, old_file) {
            continue;
        }
        if file.path() != old_file.path() {
            return None;
        }
        // Go: program.go:368-375 ReuseProgram rebuilds when the changed file
        // is in a package redirect group, but it does not check the
        // content-mapper supplemental files (:391, :424-429). When one of those
        // is a redirect target, Go's `filesByPath` keeps the old file at the
        // redirect paths of its group, and a shared path-to-slot map would
        // give the new file there. So such a version takes the full build,
        // which keeps Go's map.
        // The `redirect_files_by_path` half (Go :371) cannot hold here, and
        // stays as in Go. A redirect path is the path of a package copy that
        // the loader leaves out of the program files (Go
        // filesparser.go:467-477, the `continue`), and `old_file` is a
        // program file. A changed copy at a redirect path takes the full
        // build in `reuse_program` (frontend/compiler/program_new.rs, Go
        // program.go:371), so no reused version gets here with it. Test:
        // `redirect_paths_are_not_program_files`.
        let in_redirect_group = old_np
            .redirect_files_by_path
            .as_ref()
            .is_some_and(|redirects| redirects.contains_key(old_file.path()))
            || old_np
                .redirect_targets_map
                .as_ref()
                .is_some_and(|targets| targets.contains_key(old_file.path()));
        if in_redirect_group {
            return None;
        }
        replaced.push(Replaced {
            slot,
            old: old_file,
            file,
        });
    }
    Some(replaced)
}

/// The tables of `np` built from its files alone. `previous` is the
/// frontend program and shared state of the version that `np` was updated
/// from (`GoSharedState::new`).
fn full_tables(np: &NewProgram, previous: Option<(&NewProgram, &GoSharedState)>) -> VersionTables {
    let files = np.source_files();
    let source_file_order: Vec<usize> = files.iter().map(|file| file.store).collect();
    let mut slots = SlotsBuilder::new(&source_file_order);
    // Every program file path is in `files_by_path` too.
    let mut file_by_path = FxHashMap::with_capacity_and_hasher(
        np.files_by_path().len().max(files.len()),
        Default::default(),
    );
    for (slot, file) in files.iter().enumerate() {
        file_by_path.insert(file.path().0.clone(), slot);
    }
    // Go: filesparser.go:480 `filesByPath[task.path] = packageIdFile`. A
    // package dedup redirect path maps to the first file with the same
    // package id, so `GetSourceFileByPath` finds that file.
    // PERF: copies the path only when it adds an entry (`entry` needs an
    // owned key even when the path is already there).
    for (path, file) in np.files_by_path() {
        if !file_by_path.contains_key(&path.0) {
            file_by_path.insert(path.0.clone(), slots.slot_of(file.store));
        }
    }
    let other_files = slots.other_files;
    let file_meta = files
        .iter()
        .map(|file| file_program_meta(np, file))
        .collect();
    let go = GoSharedState::new(np, previous, None, &file_by_path);
    VersionTables {
        go: Some(go),
        file_versions: files
            .iter()
            .filter_map(|file| file.version.get().cloned())
            .collect(),
        ..VersionTables::from_parts(
            source_file_order,
            other_files,
            Arc::new(file_by_path),
            Arc::new(file_meta),
        )
    }
}

/// The tables of `np`, which replaced the files `replaced` of version `old`
/// in place (Go `ReuseProgram`): the old tables with the entries of the
/// replaced files changed. The files keep their paths and slots, so the new
/// tables share the paths and, while they are equal, the program-set
/// fields. They equal `full_tables` of `np` (`check_version_tables`).
// PERF: Go `ReuseProgram` copies the old program's maps. This copies only
// lists of ids and `Arc`s, not paths (editfast1).
fn reused_tables(np: &NewProgram, old: &PreviousVersion, replaced: &[Replaced]) -> VersionTables {
    let (old_np, old) = (&*old.np, &*old.tables);
    let old_shared = old.go.as_ref().expect("checked by previous_version");
    let mut source_file_order = old.source_file_order.clone();
    let mut file_meta = Arc::clone(&old.file_meta);
    for r in replaced {
        source_file_order[r.slot] = r.file.store;
        let meta = file_program_meta(np, r.file);
        if file_meta[r.slot] != meta {
            Arc::make_mut(&mut file_meta)[r.slot] = meta;
        }
    }
    let dropped: Vec<&Arc<crate::ast::FileVersion>> = replaced
        .iter()
        .filter_map(|r| r.old.version.get())
        .collect();
    let file_versions = old
        .file_versions
        .iter()
        .filter(|version| !dropped.iter().any(|dropped| Arc::ptr_eq(version, dropped)))
        .cloned()
        .chain(
            replaced
                .iter()
                .filter_map(|r| r.file.version.get().cloned()),
        )
        .collect();
    let go = GoSharedState::new(
        np,
        Some((old_np, old_shared)),
        Some(replaced),
        &old.file_by_path,
    );
    VersionTables {
        go: Some(go),
        file_versions,
        ..VersionTables::from_parts(
            source_file_order,
            old.other_files.clone(),
            Arc::clone(&old.file_by_path),
            file_meta,
        )
    }
}

/// Go `SourceFile.Metadata` and `IsDefaultLibrary` of program file `file`.
fn file_program_meta(np: &NewProgram, file: &ParsedSourceFile) -> FileProgramMeta {
    let path = file.path();
    FileProgramMeta {
        meta_data: np.get_source_file_meta_data(path),
        is_default_library: np.is_source_file_default_library(path),
    }
}

/// Panics when `tables`, which `reused_tables` made, differ from `full`,
/// which `full_tables` made for the same program.
fn assert_same_tables(tables: &VersionTables, full: &VersionTables) {
    assert_eq!(
        tables.source_file_order, full.source_file_order,
        "source file order"
    );
    assert_eq!(tables.file_by_path.len(), full.file_by_path.len(), "paths");
    for (path, &slot) in full.file_by_path.iter() {
        assert_eq!(
            tables.file_at_path(path),
            Some(full.file_at_slot(slot)),
            "file at {path}"
        );
        assert_eq!(
            tables.file_meta_by_path(path),
            full.file_meta_by_path(path),
            "program fields of {path}"
        );
    }
    let versions = |tables: &VersionTables| {
        let mut versions: Vec<*const crate::ast::FileVersion> =
            tables.file_versions.iter().map(Arc::as_ptr).collect();
        versions.sort_unstable();
        versions
    };
    assert_eq!(versions(tables), versions(full), "file versions");
    let (Some(go), Some(full_go)) = (&tables.go, &full.go) else {
        panic!("a Go frontend program has shared state");
    };
    go.assert_same(full_go);
}

// Go: compiler/program.go:1562 CommonSourceDirectory.
// PORT: Go computes it on first use, and `checkSourceFilesBelongToPath`
// then adds include diagnostics. Go uses it first either in
// `verifyCompilerOptions`, which the frontend program has already run, or
// during emit, after the program diagnostics are reported. So the value is
// computed when the program is built, without the check, which adds
// nothing.
fn common_source_directory_of(p: &NewProgram) -> String {
    let files = &p.processed_files;
    crate::frontend::outputpaths::get_common_source_directory(
        p.options(),
        || {
            files
                .files
                .iter()
                .filter(|file| {
                    p.source_file_may_be_emitted(file, false) && !file.is_declaration_file
                })
                .map(|file| file.file_name().to_string())
                .collect()
        },
        &p.get_current_directory(),
        p.use_case_sensitive_file_names(),
        None,
    )
}

// Go: compiler/program.go:852 collectContentMapperOptionDiagnostics (#4712)
// PORT: returns the list; Go sets `p.contentMapperOptionDiagnostics`.
fn collect_content_mapper_option_diagnostics(p: &NewProgram) -> Vec<Diagnostic> {
    // Go: compiler/program.go:158 ContentMapperProject
    let Some(project) = p.host().content_mapper_project() else {
        return Vec::new();
    };
    let option_diagnostics = project.diagnostics();
    option_diagnostics
        .iter()
        .map(|diagnostic| {
            let (file, loc) =
                crate::frontend::tsoptions::get_content_mapper_option_diagnostic_location(
                    Some(p.command_line().as_ref()),
                    &diagnostic.mapper,
                    &diagnostic.path,
                );
            crate::ast::new_external_diagnostic(
                file,
                loc,
                &diagnostic.source,
                crate::diagnostics::Category::Error,
                diagnostic.code,
                &diagnostic.message_text,
            )
        })
        .collect()
}

/// Go `getTraceFromSys` (tsc.go:280) with no testing hooks:
/// `tsc.GetTraceWithWriterFromSys` (tsc/emit.go:20) writes each localized
/// trace message and a newline to `sys.Writer()`, which is stdout.
fn trace_from_sys() -> TraceFn {
    Rc::new(|msg: &'static Message, args: Vec<String>| {
        let text = format_message(msg, &args);
        // PORT: `text` is in the port form (see
        // `scanner_util::GO_STRING_MARKER`); stdout gets its Go bytes.
        let _ = crate::execute::tsc::write_go_output(
            &mut crate::execute::tsc::stdio::CliStdout,
            format!("{text}\n").as_bytes(),
        );
    })
}

/// The parse lists that a `SourceFileInfo` keeps.
struct KeptLists {
    diagnostics: KeptData<[Diagnostic]>,
    js_diagnostics: KeptData<[Diagnostic]>,
    jsdoc_diagnostics: KeptData<[Diagnostic]>,
    reparsed_clones: KeptData<[Node]>,
    jsdoc_cache: KeptData<FxHashMap<Node, Vec<Node>>>,
}

impl KeptLists {
    /// The lists of `file`, a parse that the publish keeps for good.
    fn borrowed(file: &'static ParsedSourceFile) -> Self {
        Self {
            diagnostics: KeptData::Borrowed(&file.diagnostics),
            js_diagnostics: KeptData::Borrowed(&file.js_diagnostics),
            jsdoc_diagnostics: KeptData::Borrowed(&file.jsdoc_diagnostics),
            reparsed_clones: KeptData::Borrowed(&file.reparsed_clones),
            jsdoc_cache: KeptData::Borrowed(&file.jsdoc_cache),
        }
    }

    /// Owned copies of the lists of `file`, a freeable file version whose
    /// parse is not kept (lsshells M3b: they are freed with the version).
    /// The lists of a TypeScript file are almost always empty, and an empty
    /// list is not copied.
    fn copied(file: &ParsedSourceFile) -> Self {
        fn copy<T: Clone>(list: &[T]) -> KeptData<[T]> {
            if list.is_empty() {
                KeptData::Borrowed(&[])
            } else {
                KeptData::Owned(list.into())
            }
        }
        Self {
            diagnostics: copy(&file.diagnostics),
            js_diagnostics: copy(&file.js_diagnostics),
            jsdoc_diagnostics: copy(&file.jsdoc_diagnostics),
            reparsed_clones: copy(&file.reparsed_clones),
            jsdoc_cache: if file.jsdoc_cache.is_empty() {
                KeptData::Borrowed(&super::EMPTY_JSDOC_CACHE)
            } else {
                KeptData::Owned(Box::new(file.jsdoc_cache.clone()))
            },
        }
    }
}

/// `SourceFileInfo` of a program file, from the Go parser fields. The Go
/// program fields are in `VersionTables::file_meta`.
// PERF: the diagnostics, reparsed clones and JSDoc cache are `lists`: the
// lists of the parse, which the publish keeps, or owned copies for a
// freeable file version. The other lists stay copies: `source_file_parser_fields`
// (ast/synthetic.rs) copies them out as owned lists, and the frontend
// program still reads the parsed file.
fn program_file_info(store: usize, file: &ParsedSourceFile, lists: KeptLists) -> SourceFileInfo {
    let info = SourceFileInfo {
        file_name: file.file_name().to_string(),
        path: file.path().0.clone(),
        is_declaration_file: file.is_declaration_file,
        language_variant: file.language_variant,
        script_kind: file.script_kind,
        pragmas: file.pragmas.clone(),
        check_js_directive: file.check_js_directive,
        referenced_files: file.referenced_files.clone(),
        type_reference_directives: file.type_reference_directives.clone(),
        lib_reference_directives: file.lib_reference_directives.clone(),
        comment_directives: file.comment_directives.clone(),
        diagnostics: lists.diagnostics,
        js_diagnostics: lists.js_diagnostics,
        jsdoc_diagnostics: lists.jsdoc_diagnostics,
        has_lazy_js_doc: file.has_lazy_js_doc,
        external_module_indicator_options: file.parse_options.external_module_indicator_options,
        late: OnceLock::new(),
    };
    let late = LateSourceFileInfo {
        file_index: store,
        external_module_indicator: file.external_module_indicator,
        reparsed_clones: lists.reparsed_clones,
        imports: file.imports.clone(),
        module_augmentations: file.module_augmentations.clone(),
        ambient_module_names: file.ambient_module_names.clone(),
        uses_uri_style_node_core_modules: file.uses_uri_style_node_core_modules,
        jsdoc_cache: lists.jsdoc_cache,
        post_bind: OnceLock::new(),
    };
    assert!(info.late.set(late).is_ok());
    info
}

/// `SourceFileInfo` of a store that is not a program file (a tsconfig or
/// an extended config). Only the name and the text are read, for
/// diagnostic locations.
fn other_store_info(
    store: usize,
    cwd: &str,
    use_case_sensitive_file_names: bool,
) -> SourceFileInfo {
    let file_name = file_store_file_name(store).to_string();
    let info = SourceFileInfo {
        path: tspath::to_path(&file_name, cwd, use_case_sensitive_file_names).into_string(),
        file_name,
        is_declaration_file: false,
        language_variant: LanguageVariant::STANDARD,
        script_kind: ScriptKind::JSON,
        pragmas: Vec::new(),
        check_js_directive: None,
        referenced_files: Vec::new(),
        type_reference_directives: Vec::new(),
        lib_reference_directives: Vec::new(),
        comment_directives: Vec::new(),
        diagnostics: KeptData::Borrowed(&[]),
        js_diagnostics: KeptData::Borrowed(&[]),
        jsdoc_diagnostics: KeptData::Borrowed(&[]),
        has_lazy_js_doc: false,
        external_module_indicator_options: Default::default(),
        late: OnceLock::new(),
    };
    let late = LateSourceFileInfo {
        file_index: store,
        external_module_indicator: Node::NIL,
        reparsed_clones: KeptData::Borrowed(&[]),
        imports: Vec::new(),
        module_augmentations: Vec::new(),
        ambient_module_names: Vec::new(),
        uses_uri_style_node_core_modules: Tristate::Unknown,
        jsdoc_cache: KeptData::Borrowed(&EMPTY_JSDOC_CACHE),
        post_bind: OnceLock::new(),
    };
    assert!(info.late.set(late).is_ok());
    info
}

impl GoSharedState {
    /// Copies what the checker reads from the frontend program. `previous`
    /// is the frontend program and shared state of the version that `p` was
    /// updated from. Its copies are shared where `p` shares the frontend
    /// data they were copied from, so a new version copies only what
    /// changed, and the versions that are alive keep one copy. `replaced`
    /// is set when `p` replaced these files of `previous` in place
    /// (`replaced_files`): then the copies by file id start from those of
    /// `previous`. `file_by_path` holds the slots of the program files
    /// (`VersionTables::file_by_path`).
    fn new(
        p: &NewProgram,
        previous: Option<(&NewProgram, &GoSharedState)>,
        replaced: Option<&[Replaced]>,
        file_by_path: &FxHashMap<String, usize>,
    ) -> Self {
        let program_files = p.source_files();
        // The old version when `p` replaced files of it in place.
        let reused = previous.filter(|_| replaced.is_some());
        let replaced = replaced.unwrap_or_default();
        let files = &p.processed_files;
        // Go `UpdateProgram` shares the resolutions and the project
        // references with the old program and keeps every file path, so
        // equal pointers mean equal copies.
        let same_resolutions = previous.filter(|(old, _)| {
            Arc::ptr_eq(
                &files.resolved_modules,
                &old.processed_files.resolved_modules,
            ) && match (
                &files.project_reference_file_mapper,
                &old.processed_files.project_reference_file_mapper,
            ) {
                (Some(mapper), Some(old_mapper)) => Rc::ptr_eq(mapper, old_mapper),
                (mapper, old_mapper) => mapper.is_none() && old_mapper.is_none(),
            }
        });
        let resolved_modules = Arc::clone(&files.resolved_modules);
        // #4712: Go `NewProgram` collects the content mapper option
        // diagnostics (`collectContentMapperOptionDiagnostics`), and
        // `ReuseProgram` clones the list of the program it reuses. A reused
        // program shares the processed files of that program.
        let content_mapper_option_diagnostics = match previous {
            Some((old, old_shared))
                if Arc::ptr_eq(
                    &files.resolved_modules,
                    &old.processed_files.resolved_modules,
                ) =>
            {
                old_shared.content_mapper_option_diagnostics.clone()
            }
            _ => collect_content_mapper_option_diagnostics(p),
        };
        let jsx_runtime_import_specifiers = files
            .jsx_runtime_import_specifiers
            .iter()
            .flat_map(|map| map.iter())
            .map(|(path, s)| (path.0.clone(), (s.module_reference.clone(), s.specifier)))
            .collect();
        let import_helpers_import_specifiers = files
            .import_helpers_import_specifiers
            .iter()
            .flat_map(|map| map.iter())
            .map(|(path, &specifier)| (path.0.clone(), specifier))
            .collect();
        let include_diagnostics = include_diagnostics_of(p, file_by_path);
        let redirect_targets = files
            .redirect_targets_map
            .iter()
            .flat_map(|map| map.iter())
            .map(|(path, targets)| (path.0.clone(), targets.clone()))
            .collect();
        // Go: the declaration transformer asks only for preserved references
        // (transformers/declarations/transform.go:469). A reference can
        // name a replaced file, so the map is built again unless no file
        // has one.
        let has_preserved =
            |file: &ParsedSourceFile| file.referenced_files.iter().any(|r| r.preserve);
        let references = match reused {
            Some((_, old))
                if old.references.is_empty() && !replaced.iter().any(|r| has_preserved(r.file)) =>
            {
                FxHashMap::default()
            }
            _ => program_files
                .iter()
                .flat_map(|file| {
                    file.referenced_files
                        .iter()
                        .filter(|r| r.preserve)
                        .map(move |r| {
                            let target = p
                                .get_source_file_from_reference(file, r)
                                .map_or(Node::NIL, |target| target.root);
                            ((file.store, r.file_name.clone()), target)
                        })
                })
                .collect(),
        };
        let output_file_to_project_reference_source = files
            .output_file_to_project_reference_source
            .iter()
            .flat_map(|map| map.iter())
            .map(|(path, source)| (path.0.clone(), source.clone()))
            .collect();
        let mut project_references = ProjectReferenceCopies::default();
        let mapper = p.mapper();
        let mut copy_map = |map: &FxHashMap<GoPath, Rc<FrontendSourceOutput>>| {
            map.iter()
                .map(|(path, entry)| (path.0.clone(), project_references.entry(entry)))
                .collect::<FxHashMap<_, _>>()
        };
        let source_to_project_reference = copy_map(&mapper.source_to_project_reference);
        let output_dts_to_project_reference = copy_map(&mapper.output_dts_to_project_reference);
        // The load released the mapper's loader (Go fileloader.go:223), so
        // no entry is added after it (`getSourceToDtsIfSymlink`, :163), and
        // an entry with no source finds nothing.
        let realpath_dts_to_source: FxHashMap<_, _> = mapper
            .realpath_dts_to_source
            .borrow()
            .iter()
            .filter_map(|(path, entry)| {
                Some((path.0.clone(), project_references.entry(entry.as_ref()?)))
            })
            .collect();
        let can_use_project_reference_source = mapper.use_source_of_project_reference;
        drop(mapper);
        // Go asks for the redirect of checker files only, which are program
        // files. A redirect comes from the path and the project references,
        // so with the same references a file that `p` shares keeps its
        // redirect. The map is built again unless no file has one. With no
        // project reference entries, Go finds no redirect for any file
        // (projectreferencefilemapper.go:90 getRedirectForResolution).
        let no_redirects = source_to_project_reference.is_empty()
            && output_dts_to_project_reference.is_empty()
            && realpath_dts_to_source.is_empty();
        let redirects_for_resolution = match (reused, same_resolutions) {
            _ if no_redirects => FxHashMap::default(),
            (Some((_, old)), Some(_))
                if old.redirects_for_resolution.is_empty()
                    && replaced
                        .iter()
                        .all(|r| p.get_redirect_for_resolution(&**r.file).is_none()) =>
            {
                FxHashMap::default()
            }
            _ => program_files
                .iter()
                .filter_map(|file| {
                    let redirect = p.get_redirect_for_resolution(&**file)?;
                    Some((file.store, project_references.resolved(&redirect)))
                })
                .collect(),
        };
        let resolved_project_references = p
            .get_resolved_project_references()
            .iter()
            .map(|r| r.as_ref().map(|r| project_references.resolved(r)))
            .collect();
        // Go builds the symlink cache on first use (`known_symlinks`), and
        // `ReuseProgram` takes the old program's cache when it is built
        // (`tryReuse`, program.go:415). The copy stands for that cache: it
        // is shared when `p` replaced files of `previous` in place, or when
        // the frontend shares its built cache.
        // PORT: the copy can be built while the frontend's own value is not
        // (`known_symlinks` builds it with the fs tracking paused, H3).
        let known_symlinks = OnceLock::new();
        if let Some((old_p, old)) = previous
            && (reused.is_some()
                || matches!(
                    (p.known_symlinks.get(), old_p.known_symlinks.get()),
                    (Some(symlinks), Some(old_symlinks)) if Rc::ptr_eq(symlinks, old_symlinks)
                ))
            && let Some(copy) = old.known_symlinks.get()
        {
            let _ = known_symlinks.set(Arc::clone(copy));
        }
        let source_files_found_searching_node_modules = match previous {
            Some((old_p, old))
                if Rc::ptr_eq(
                    &files.source_files_found_searching_node_modules,
                    &old_p
                        .processed_files
                        .source_files_found_searching_node_modules,
                ) =>
            {
                Arc::clone(&old.source_files_found_searching_node_modules)
            }
            _ => Arc::new(
                files
                    .source_files_found_searching_node_modules
                    .iter()
                    .map(|path| path.0.clone())
                    .collect(),
            ),
        };
        let has_emit_blocking_diagnostics = p
            .has_emit_blocking_diagnostics
            .iter()
            .map(|path| path.0.clone())
            .collect();
        Self {
            has_emit_blocking_diagnostics,
            current_directory: p.get_current_directory(),
            use_case_sensitive_file_names: p.use_case_sensitive_file_names(),
            // #4712
            content_mapper_extensions: p.command_line().content_mapper_extensions(),
            content_mapper_option_diagnostics,
            resolved_modules,
            jsx_runtime_import_specifiers,
            import_helpers_import_specifiers,
            include_diagnostics,
            include_diagnostics_read: Arc::clone(&p.include_processor.diagnostics_read),
            realpath_dts_to_source,
            redirect_targets,
            references,
            output_file_to_project_reference_source,
            source_to_project_reference,
            output_dts_to_project_reference,
            can_use_project_reference_source,
            redirects_for_resolution,
            resolved_project_references,
            known_symlinks,
            source_files_found_searching_node_modules,
            from_old_tables: reused.is_some(),
        }
    }

    // Go: compiler/program.go:549 ContentMapperExtensions (#4712)
    pub(super) fn content_mapper_extensions(&self) -> &[String] {
        &self.content_mapper_extensions
    }

    /// Go `Program.contentMapperOptionDiagnostics` (#4712).
    pub(super) fn content_mapper_option_diagnostics(&self) -> &[Diagnostic] {
        &self.content_mapper_option_diagnostics
    }

    // Go: compiler/program.go:206 GetSourceOfProjectReferenceIfOutputIncluded
    // (the map lookup; the caller falls back to the file name)
    pub(super) fn get_source_of_project_reference_if_output_included(
        &self,
        path: &str,
    ) -> Option<&str> {
        self.output_file_to_project_reference_source
            .get(path)
            .map(String::as_str)
    }

    // Go: compiler/projectreferencefilemapper.go:78 getProjectReferenceFromSource
    pub(super) fn get_project_reference_from_source(
        &self,
        path: &str,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.source_to_project_reference.get(path).cloned()
    }

    // Go: compiler/projectreferencefilemapper.go:82 getProjectReferenceFromOutputDts
    pub(super) fn get_project_reference_from_output_dts(
        &self,
        path: &str,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.output_dts_to_project_reference.get(path).cloned()
    }

    // Go: compiler/projectreferencefilemapper.go:86 isSourceFromProjectReference
    pub(super) fn is_source_from_project_reference(&self, path: &str) -> bool {
        self.can_use_project_reference_source && self.source_to_project_reference.contains_key(path)
    }

    // Go: compiler/program.go:231 GetRedirectForResolution (for program files)
    pub(super) fn get_redirect_for_resolution(
        &self,
        file: Node,
    ) -> Option<&Arc<ResolvedProjectReference>> {
        self.redirects_for_resolution.get(&file.file_index())
    }

    // Go: compiler/program.go:245 GetResolvedProjectReferences
    pub(super) fn get_resolved_project_references(
        &self,
    ) -> Vec<Option<Arc<ResolvedProjectReference>>> {
        self.resolved_project_references.clone()
    }

    /// The copy of Go `Program.GetSymlinkCache` when it is built
    /// (`known_symlinks`).
    fn built_known_symlinks(
        &self,
    ) -> Option<Arc<crate::modulespecifiers::symlinks::KnownSymlinks>> {
        self.known_symlinks.get().cloned()
    }

    // Go: compiler/program.go:236 GetParseFileRedirect
    // Go: compiler/projectreferencefilemapper.go:45 getParseFileRedirect
    // PORT: after the load, so `getSourceToDtsIfSymlink` only reads
    // `realpathDtsToSource`. None is the Go "".
    // PERF: with no project reference entries there is no redirect, and
    // the path is not made.
    pub(super) fn get_parse_file_redirect(&self, file_name: &str) -> Option<&str> {
        let to_path = || {
            crate::frontend::tspath::to_path(
                file_name,
                &self.current_directory,
                self.use_case_sensitive_file_names,
            )
        };
        let redirect = if self.can_use_project_reference_source {
            // Map to source file from project reference
            if self.output_dts_to_project_reference.is_empty()
                && self.realpath_dts_to_source.is_empty()
            {
                return None;
            }
            let path = to_path();
            let source = self
                .output_dts_to_project_reference
                .get(&path.0)
                .or_else(|| self.realpath_dts_to_source.get(&path.0))?;
            source.source.as_str()
        } else {
            // Map to dts file from project reference
            if self.source_to_project_reference.is_empty() {
                return None;
            }
            self.source_to_project_reference
                .get(&to_path().0)?
                .output_dts
                .as_str()
        };
        (!redirect.is_empty()).then_some(redirect)
    }

    // Go: compiler/program.go:198 GetRedirectTargets
    pub(super) fn get_redirect_targets(&self, path: &str) -> Vec<String> {
        self.redirect_targets.get(path).cloned().unwrap_or_default()
    }

    // Go: compiler/program.go:275 GetSourceFileFromReference (for the
    // preserved references of a program file)
    pub(super) fn get_source_file_from_reference(&self, origin: Node, r: &FileReference) -> Node {
        self.references
            .get(&(origin.file_index(), r.file_name.clone()))
            .copied()
            .expect("not a preserved reference of a program file")
    }

    // Go: compiler/program.go:644 GetResolvedModule
    // PERF: returns a borrow, and looks the name up as `&str`. A name has at
    // most one entry per mode, so the mode scan finds the Go map entry.
    pub(super) fn get_resolved_module(
        &self,
        file: Node,
        module_reference: &str,
        mode: ResolutionMode,
    ) -> Option<&Arc<ResolvedModule>> {
        let info = source_file_info(file);
        let path = info.path.as_str();
        self.resolved_modules
            .get(path)?
            .get(&(module_reference, mode) as &dyn crate::frontend::module::ModeAwareKey)
    }

    // Go: compiler/program.go:664 GetResolvedModules (ranged over: every
    // resolution of every file)
    // PERF: borrows the version's own map, as Go returns `p.resolvedModules`
    // with no copy. The order is the map order; Go ranges over maps in a
    // random order.
    pub(super) fn resolved_modules(&self) -> impl Iterator<Item = &ResolvedModule> + '_ {
        self.resolved_modules
            .values()
            .flat_map(|cache| cache.values())
            .map(|resolved| &**resolved)
    }

    // Go: compiler/program.go:2228 GetJSXRuntimeImportSpecifier
    pub(super) fn get_jsx_runtime_import_specifier(&self, path: &str) -> (String, Node) {
        self.jsx_runtime_import_specifiers
            .get(path)
            .cloned()
            .unwrap_or((String::new(), Node::NIL))
    }

    // Go: compiler/program.go:1414 IsEmitBlocked
    pub(super) fn is_emit_blocked(&self, emit_file_name: &str) -> bool {
        let path = crate::frontend::tspath::to_path(
            emit_file_name,
            &self.current_directory,
            self.use_case_sensitive_file_names,
        );
        self.has_emit_blocking_diagnostics.contains(&path.0)
    }

    // Go: compiler/program.go:2224 IsSourceFileFromExternalLibrary
    pub(super) fn is_source_file_from_external_library(&self, path: &str) -> bool {
        self.source_files_found_searching_node_modules
            .contains(path)
    }

    // Go: compiler/program.go:2235 GetImportHelpersImportSpecifier
    pub(super) fn get_import_helpers_import_specifier(&self, path: &str) -> Node {
        self.import_helpers_import_specifiers
            .get(path)
            .copied()
            .unwrap_or(Node::NIL)
    }

    // Go: compiler/program.go:864 GetIncludeProcessorDiagnostics (the
    // include processor part)
    pub(super) fn get_include_processor_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        // Go builds the collection here (`includeProcessor.getDiagnostics`).
        self.include_diagnostics_read
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.include_diagnostics
            .get(&file.file_index())
            .cloned()
            .unwrap_or_default()
    }

    /// Panics when the copies by file id differ from those of `full`, which
    /// `GoSharedState::new` made for the same program with no replaced
    /// files (`assert_same_tables`). The other copies are made the same way
    /// for both.
    fn assert_same(&self, full: &GoSharedState) {
        assert_same_include_diagnostics(&self.include_diagnostics, &full.include_diagnostics);
        assert_eq!(self.references, full.references, "preserved references");
        let mut redirects: Vec<_> = self.redirects_for_resolution.keys().collect();
        let mut full_redirects: Vec<_> = full.redirects_for_resolution.keys().collect();
        redirects.sort_unstable();
        full_redirects.sort_unstable();
        assert_eq!(redirects, full_redirects, "files with a redirect");
        for (store, redirect) in &self.redirects_for_resolution {
            let full = &full.redirects_for_resolution[store];
            assert!(
                redirect.compiler_options.deep_equal(&full.compiler_options)
                    && redirect.common_source_directory == full.common_source_directory,
                "redirect of file {store}"
            );
        }
    }

    /// Panics when the include processor diagnostics differ from those that
    /// Go `GetIncludeProcessorDiagnostics` reads for each program file of
    /// `p` (`include_diagnostics_of`).
    fn assert_include_diagnostics(&self, p: &NewProgram) {
        let collection = p.include_processor.get_diagnostics(p);
        let each_file: FxHashMap<usize, Vec<Diagnostic>> = p
            .source_files()
            .iter()
            .filter_map(|file| {
                let diagnostics = collection.borrow_mut().get_diagnostics_for_file(file.root);
                (!diagnostics.is_empty()).then_some((file.store, diagnostics))
            })
            .collect();
        assert_same_include_diagnostics(&self.include_diagnostics, &each_file);
    }
}

/// Go `GetIncludeProcessorDiagnostics` (the include processor part) of each
/// program file of `p` that has any, by file id. `file_by_path` holds the
/// slots of the program files (`VersionTables::file_by_path`).
// PERF: reads the files that the collection has diagnostics for, not every
// program file (editfast1). The collection keys its lists by file name, and
// one program has one file name per path.
fn include_diagnostics_of(
    p: &NewProgram,
    file_by_path: &FxHashMap<String, usize>,
) -> FxHashMap<usize, Vec<Diagnostic>> {
    let files = p.source_files();
    let mut collection = p.include_processor.get_diagnostics(p).borrow_mut();
    let names: Vec<&'static str> = collection.file_names().collect();
    names
        .into_iter()
        .filter_map(|name| {
            let file = files.get(*file_by_path.get(&p.to_path(name).0)?)?;
            (file.file_name() == name)
                .then(|| (file.store, collection.get_diagnostics_for_file_name(name)))
        })
        .collect()
}

/// Panics when two maps of include processor diagnostics by file id differ.
// PORT: `Diagnostic` has no `==`; the debug text holds every field.
fn assert_same_include_diagnostics(
    diagnostics: &FxHashMap<usize, Vec<Diagnostic>>,
    expected: &FxHashMap<usize, Vec<Diagnostic>>,
) {
    let text = |map: &FxHashMap<usize, Vec<Diagnostic>>| {
        let mut entries: Vec<_> = map
            .iter()
            .map(|(store, list)| (*store, format!("{list:?}")))
            .collect();
        entries.sort_unstable();
        entries
    };
    assert_eq!(
        text(diagnostics),
        text(expected),
        "include processor diagnostics"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of `np` with the file at `slot` parsed again, as Go
    /// `ReuseProgram` replaces a changed file in place: the same files at
    /// the same paths and the same resolutions.
    fn replaced_in_place(np: &NewProgram, slot: usize) -> NewProgram {
        let mut processed_files = np.processed_files.clone();
        let old = &processed_files.files[slot];
        let file = Rc::new(crate::frontend::parser::parse_source_file(
            old.parse_options(),
            crate::ast::file_version::FileText::new(format!("{}\n", old.text()), false),
            old.script_kind,
        ));
        processed_files.files[slot] = file.clone();
        processed_files
            .files_by_path
            .insert(file.path().clone(), file);
        NewProgram {
            opts: np.opts.clone(),
            hosts: np.hosts.clone(),
            module_resolution_error: np.module_resolution_error.clone(),
            compare_paths_options: np.compare_paths_options.clone(),
            processed_files,
            uses_uri_style_node_core_modules: np.uses_uri_style_node_core_modules,
            common_source_directory: std::cell::OnceCell::new(),
            program_diagnostics: Vec::new(),
            has_emit_blocking_diagnostics: FxHashSet::default(),
            source_files_to_emit: std::cell::OnceCell::new(),
            unresolved_imports: Default::default(),
            known_symlinks: Default::default(),
            package_names: Default::default(),
            has_ts_file: std::cell::OnceCell::new(),
        }
    }

    /// A program whose `index.ts` imports two packages, each with its own
    /// copy of pkg@1.0.0: a package redirect group. The second copy's path
    /// is a redirect path, and the first copy is its target. `name` names
    /// the fixture dir. The caller enters the program scope.
    fn redirect_group_program(name: &str) -> NewProgram {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = r#"{"name":"pkg","version":"1.0.0","types":"index.d.ts"}"#;
        let files = [
            ("tsconfig.json", r#"{"compilerOptions":{"types":[]}}"#),
            (
                "index.ts",
                "import { a } from \"a\";\nimport { b } from \"b\";\nexport const x = a + b;\n",
            ),
            (
                "node_modules/a/package.json",
                r#"{"name":"a","types":"index.d.ts"}"#,
            ),
            (
                "node_modules/a/index.d.ts",
                "import { p } from \"pkg\";\nexport declare const a: typeof p;\n",
            ),
            ("node_modules/a/node_modules/pkg/package.json", pkg),
            (
                "node_modules/a/node_modules/pkg/index.d.ts",
                "export declare const p: number;\n",
            ),
            (
                "node_modules/b/package.json",
                r#"{"name":"b","types":"index.d.ts"}"#,
            ),
            (
                "node_modules/b/index.d.ts",
                "import { p } from \"pkg\";\nexport declare const b: typeof p;\n",
            ),
            ("node_modules/b/node_modules/pkg/package.json", pkg),
            (
                "node_modules/b/node_modules/pkg/index.d.ts",
                "export declare const p: number;\n",
            ),
        ];
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let config = format!("{}/tsconfig.json", dir.to_string_lossy().replace('\\', "/"));
        let opts = load_config(&config, |_| {}, &mut CompileTimes::default()).unwrap();
        let np = new_program(opts);
        let _ = std::fs::remove_dir_all(&dir);
        np
    }

    // followups4 (R153 reviewer): editfast1 gives a full build when a
    // replaced file is in a package redirect group (`replaced_files`). Go's
    // ReuseProgram checks only the changed file, not the content-mapper
    // supplemental files, so a reused program can still replace a file of
    // a redirect group. Here the same pkg@1.0.0 is in two node_modules
    // dirs, so one copy redirects to the other.
    #[test]
    fn replaced_file_in_redirect_group_takes_the_full_build() {
        let _scope = crate::core::enter_program(None);
        let np = redirect_group_program("goport-redirect");

        let files = np.source_files();
        let old = VersionTables::new((0..files.len()).collect(), std::iter::empty());
        let slot_of = |name: &str| {
            files
                .iter()
                .position(|file| file.file_name().ends_with(name))
                .unwrap_or_else(|| panic!("no {name}"))
        };
        let in_group = |path: &GoPath| {
            np.redirect_files_by_path
                .as_ref()
                .is_some_and(|redirects| redirects.contains_key(path))
                || np
                    .redirect_targets_map
                    .as_ref()
                    .is_some_and(|targets| targets.contains_key(path))
        };
        let pkg = slot_of("/node_modules/a/node_modules/pkg/index.d.ts");
        assert!(
            in_group(files[pkg].path()),
            "pkg is not in a redirect group"
        );
        let index = slot_of("/index.ts");
        assert!(!in_group(files[index].path()));

        // A replaced file outside the group keeps the old tables.
        let edited = replaced_in_place(&np, index);
        let replaced = replaced_files(&edited, &np, &old).expect("a reused program");
        assert_eq!(replaced.iter().map(|r| r.slot).collect::<Vec<_>>(), [index]);

        // A replaced file of the group takes the full build.
        let edited = replaced_in_place(&np, pkg);
        assert!(replaced_files(&edited, &np, &old).is_none());
    }

    /// A project in a new dir under the system temp dir: `index.ts`
    /// imports `pkg`, and `node_modules/pkg` is a symlink to `real/pkg`, so
    /// the resolution goes through the symlink. Returns the dir with `/`
    /// separators. `name` names the dir.
    #[cfg(unix)]
    fn symlinked_package_project(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"types":[]},"files":["index.ts"]}"#,
            ),
            (
                "index.ts",
                "import { p } from \"pkg\";\nexport const x = p;\n",
            ),
            (
                "real/pkg/package.json",
                r#"{"name":"pkg","types":"index.d.ts"}"#,
            ),
            ("real/pkg/index.d.ts", "export declare const p: number;\n"),
        ];
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::os::unix::fs::symlink("../real/pkg", dir.join("node_modules/pkg")).unwrap();
        dir.to_string_lossy().replace('\\', "/")
    }

    /// The frontend program of `version` on this thread.
    fn frontend_of(version: &GoProgram) -> Rc<NewProgram> {
        FRONTENDS
            .with(|frontends| frontends.borrow().get(&version.id).cloned())
            .expect("a version loaded on this thread")
    }

    // progtables1: Go builds the symlink cache on first use
    // (program.go:2300 GetSymlinkCache, a `lazyValue`). A load does not
    // build it. A seeded thread cannot build it, so the loading thread
    // builds it when it seeds one, and both read the same copy.
    #[cfg(unix)]
    #[test]
    fn symlink_cache_is_built_on_first_use() {
        let dir = symlinked_package_project("goport-lazy-symlinks");
        let program = crate::program::try_load_version(&format!("{dir}/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let frontend = frontend_of(program);
        assert!(
            frontend.known_symlinks.get().is_none(),
            "the load built the symlink cache"
        );
        let (seeded, built) = {
            let _scope = crate::core::enter_program(Some(program));
            let seeded = crate::program::spawn_seeded_thread(|| {
                crate::program::get_go_symlink_cache().expect("a Go frontend program")
            })
            .join()
            .expect("the seeded thread reads the cache");
            let built = crate::program::get_go_symlink_cache().expect("a Go frontend program");
            (seeded, built)
        };
        assert!(Arc::ptr_eq(&seeded, &built), "two copies of the cache");
        let directories = |symlinks: &crate::modulespecifiers::symlinks::KnownSymlinks| {
            symlinks.directories().keys().cloned().collect::<Vec<_>>()
        };
        assert_eq!(
            directories(&built),
            directories(&frontend.get_symlink_cache())
        );
        assert!(
            directories(&built)
                .iter()
                .any(|path| path.0.ends_with("/node_modules/pkg/")),
            "node_modules/pkg is not a known symlink"
        );
        drop(frontend);
        crate::program::release_program(program);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // progtables1: Go `ReuseProgram` takes the old program's symlink cache
    // when it is built (`tryReuse`, program.go:415), and keeps it unbuilt
    // when it is not.
    #[cfg(unix)]
    #[test]
    fn reused_program_shares_a_built_symlink_cache() {
        let dir = symlinked_package_project("goport-reused-symlinks");
        let index = format!("{dir}/index.ts");
        let v1 = crate::program::try_load_version(&format!("{dir}/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let body_edit = |old, digit: &str| {
            std::fs::write(
                &index,
                format!("import {{ p }} from \"pkg\";\nexport const x = p + {digit};\n"),
            )
            .unwrap();
            let (version, reused) = crate::program::update_program_version(old, &index);
            assert!(reused, "a body edit replaces the file in place");
            version
        };
        let v2 = body_edit(v1, "1");
        assert!(frontend_of(v2).known_symlinks.get().is_none());
        let built = {
            let _scope = crate::core::enter_program(Some(v2));
            crate::program::get_go_symlink_cache().expect("a Go frontend program")
        };
        let v3 = body_edit(v2, "2");
        let shared = {
            let _scope = crate::core::enter_program(Some(v3));
            crate::program::get_go_symlink_cache().expect("a Go frontend program")
        };
        assert!(
            Arc::ptr_eq(&built, &shared),
            "the reused program built its own copy"
        );
        for version in [v1, v2, v3] {
            crate::program::release_program(version);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // progtables1: Go reads a parse file redirect per call (program.go:211
    // GetParseFileRedirect). The copy answers per call from the project
    // reference maps, as the frontend program does. Here b references a,
    // so a's source file redirects to its output d.ts (tsc does not use
    // the source of a project reference).
    #[test]
    fn parse_file_redirects_match_the_frontend() {
        let dir =
            std::env::temp_dir().join(format!("goport-parse-redirects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            (
                "a/tsconfig.json",
                r#"{"compilerOptions":{"composite":true,"outDir":"out","types":[]}}"#,
            ),
            ("a/index.ts", "export const a = 1;\n"),
            ("a/out/index.d.ts", "export declare const a = 1;\n"),
            (
                "b/tsconfig.json",
                r#"{"compilerOptions":{"types":[]},"references":[{"path":"../a"}],"files":["index.ts"]}"#,
            ),
            (
                "b/index.ts",
                "import { a } from \"../a/index\";\nexport const b = a;\n",
            ),
        ];
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let dir = dir.to_string_lossy().replace('\\', "/");
        let program = crate::program::try_load_version(&format!("{dir}/b/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let frontend = frontend_of(program);
        let mut names: Vec<String> = frontend
            .resolved_modules
            .values()
            .flat_map(|cache| cache.values())
            .map(|resolved| resolved.resolved_file_name.clone())
            .filter(|name| !name.is_empty())
            .collect();
        names.extend([format!("{dir}/a/index.ts"), format!("{dir}/b/index.ts")]);
        let mut redirected = 0;
        {
            let _scope = crate::core::enter_program(Some(program));
            for name in &names {
                let want = frontend.get_parse_file_redirect(name);
                let got = with_go(|go| go.get_parse_file_redirect(name).map(str::to_string));
                assert_eq!(got.unwrap_or_default(), want, "redirect of {name}");
                redirected += usize::from(!want.is_empty());
            }
        }
        assert!(redirected > 0, "no name has a redirect");
        drop(frontend);
        crate::program::release_program(program);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Why the `redirect_files_by_path` half of the bailout in
    // `replaced_files` cannot hold: no program file has a redirect path,
    // so a replaced program file is never at one.
    #[test]
    fn redirect_paths_are_not_program_files() {
        let _scope = crate::core::enter_program(None);
        let np = redirect_group_program("goport-redirect-paths");
        let redirects = np
            .redirect_files_by_path
            .as_ref()
            .expect("a redirect path for the second copy of pkg");
        assert!(
            redirects.keys().any(|path| path
                .0
                .ends_with("/node_modules/b/node_modules/pkg/index.d.ts")),
            "the second copy of pkg is not a redirect path"
        );
        let files = np.source_files();
        assert!(
            files.iter().any(|file| file
                .path()
                .0
                .ends_with("/node_modules/a/node_modules/pkg/index.d.ts")),
            "the first copy of pkg is not a program file"
        );
        for file in files {
            assert!(
                !redirects.contains_key(file.path()),
                "program file {} has a redirect path",
                file.path().0
            );
        }
        // Each redirect path still finds its target (Go `filesByPath`).
        for (path, redirect) in redirects.iter() {
            let target = np
                .files_by_path()
                .get(path)
                .expect("a file at the redirect path");
            assert_eq!(target.path(), &redirect.target);
        }
    }

    /// Writes `files` (path, text) to a new dir under the system temp dir
    /// and returns the dir with `/` separators. `name` names the dir.
    fn write_project(name: &str, files: &[(&str, &str)]) -> String {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir.to_string_lossy().replace('\\', "/")
    }

    // specstat1 (realworld3 gap 1): a thread without the frontend program
    // (a checker worker) asks the program's `HostFsCache`, as Go checkers
    // ask the program host's `cachedvfs.FS` (compiler/host.go:52). So the
    // first answer for a path stays for the program version. Before, each
    // call asked the disk: lexical made 8.6M stat calls, Go 8,768.
    #[test]
    fn worker_file_exists_keeps_the_first_answer() {
        let dir = write_project(
            "goport-worker-file-exists",
            &[
                (
                    "tsconfig.json",
                    r#"{"compilerOptions":{"types":[]},"files":["index.ts"]}"#,
                ),
                ("index.ts", "export const x = 1;\n"),
            ],
        );
        let program = crate::program::try_load_version(&format!("{dir}/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let probe = |path: String| {
            std::thread::spawn(move || {
                crate::core::set_thread_program(Some(program));
                crate::program::file_exists(&path)
            })
            .join()
            .unwrap()
        };
        let later = format!("{dir}/later.ts");
        assert!(!probe(later.clone()));
        std::fs::write(&later, "").unwrap();
        let after_write = probe(later.clone());
        let index = probe(format!("{dir}/index.ts"));
        crate::program::release_program(program);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !after_write,
            "a worker asked the disk again: the program cache keeps the first answer"
        );
        assert!(index);
    }
}
