//! Go: compiler/fileloader.go (the file loader that finds, parses and
//! resolves every program file).
//!
//! PORT: Go `opts.Tracing` is the process tracing session
//! (`crate::tracing::get`).

use super::files_parser::{TakenParse, count_prep, note_prep_taken};
use crate::contentmapper::{
    ConcurrentTransform, DiagnosticDirectiveError, DiagnosticDirectiveErrorKind, InitializeError,
    InitializeErrorKind, InvalidVirtualExtensionError, Mapper, ProjectError, ProjectErrorKind,
    SupplementalFileCollisionError, TransformError, TransformErrorKind,
};
use crate::frontend::prelude::*;
use crate::gostd::{GoError, errors};
use crate::spanmap::{MappingError, MappingErrorKind};
use std::cell::Cell;
use std::sync::Arc;

// Go: fileloader.go:35 maxContentMapperFailures (tsgo#4712)
// maxContentMapperFailures is the number of transform failures a single content mapper may accumulate
// before it is disabled for the rest of the program.
const MAX_CONTENT_MAPPER_FAILURES: i32 = 5;

// Go: fileloader.go:27 libResolution
pub struct LibResolution {
    pub library_name: String,
    pub resolution: Arc<ResolvedModule>,
    pub trace: Vec<DiagAndArgs>,
}

// Go: fileloader.go:37 LibFile
#[derive(Clone, Debug, Default)]
pub struct LibFile {
    pub name: String,
    pub path: String,
    pub replaced: bool,
}

// Go: fileloader.go:44 sourceFileFromReferenceDiagnostic
pub struct SourceFileFromReferenceDiagnostic {
    pub message: &'static Message,
    pub args: Vec<String>,
}

// Go: fileloader.go:49 fileLoader
// PORT: the Go loader is shared by the parse work group. The port is single
// threaded (contract 10), so the atomics and sync maps are `Cell` and
// `RefCell`, and `factoryMu` is not needed. `resolver` is `None` only until
// `process_all_program_files` sets it, as in Go.
// Go: fileloader.go:49 fileLoader (ts#64519: `opts ProgramConfig`, `host`,
// `tracing`). PORT: `tracing` is the process session.
pub struct FileLoader {
    pub opts: ProgramConfig,
    pub host: Rc<dyn CompilerHost>,
    /// The program has a `create_module_resolver` factory.
    // PORT: not in Go, which only calls the factory. Parse workers resolve
    // ahead only for the default resolver (PERF, `process_all_program_files`).
    pub custom_module_resolver: bool,
    pub resolver: Option<Rc<dyn Resolver>>,
    pub default_library_path: String,
    pub compare_paths_options: ComparePathsOptions,
    pub supported_extensions: Vec<Vec<String>>,
    pub supported_extensions_with_json_if_resolve_json_module: Vec<Vec<String>>,
    // tsgo#4712
    pub content_mapper_extensions: Vec<String>,

    pub files_parser: RefCell<FilesParser>,
    pub root_tasks: Vec<ParseTaskRef>,

    pub total_file_count: Cell<i32>,
    pub lib_file_count: Cell<i32>,

    pub factory: NodeFactory,

    pub project_reference_file_mapper: Rc<RefCell<ProjectReferenceFileMapper>>,
    pub dts_directories: FxHashSet<Path>,

    pub path_for_lib_file_cache: RefCell<FxHashMap<String, Rc<LibFile>>>,
    pub path_for_lib_file_resolutions: RefCell<FxHashMap<Path, Rc<LibResolution>>>,

    /// The resolution cache that `resolver` shares with the parse workers
    /// (`SharedResolutionCache`), when the program shares one.
    pub shared_resolution: Option<Arc<SharedResolutionCache>>,
    /// A language server load that resolves ahead: the module names of each
    /// parse are kept for the next loads (`import_names`).
    // PORT: not in Go (perf).
    pub keeps_import_names: bool,
    /// The lookup logs of the worker answers that the load took from a
    /// prep (`FilePrep`, `WorkerMeta`), by the address of each log: each
    /// is noted once (`note_worker_logs`). The shared cache holds every
    /// such log during the load, so no address is used twice.
    // PORT: not in Go (loadpar1).
    pub noted_worker_logs: RefCell<FxHashSet<usize>>,

    // contentMapperMu guards the content-mapper bookkeeping below, which is written concurrently as
    // content-mapped files are parsed across worker goroutines.
    // tsgo#4712. PORT: the loader is single threaded (see above), so there
    // is no mutex. Go keys the maps by `*contentmapper.Mapper`; the key here
    // is `Rc::as_ptr` of the mapper, which the program config keeps alive.
    pub content_mapper_failures: RefCell<FxHashMap<*const Mapper, i32>>,
    pub content_mapper_init_failed: RefCell<FxHashSet<*const Mapper>>,
    pub content_mapper_diagnostics: RefCell<Vec<Diagnostic>>,
    /// What the parse workers send the transforms of each mapper's files
    /// with, from the loader's first transform of a file of the mapper on
    /// (`note_content_mapper_transform`). Keyed as the maps above: two
    /// mappers of one package and version have their own options, and so
    /// their own project.
    // PORT: not in Go, where the parse goroutines transform.
    pub concurrent_transforms: RefCell<FxHashMap<*const Mapper, Arc<ConcurrentTransform>>>,
    /// Set when `concurrent_transforms` gets a mapper, so that the files
    /// parser queues the worker jobs of the files of that mapper that wait
    /// in its queue (`FilesParser::queue_mapped_prefetch`).
    // PORT: not in Go (see `concurrent_transforms`).
    pub mapped_prefetch_ready: Cell<bool>,
    // ts#64299. PORT: Go `moduleResolutionErrorOnce` plus the error is an
    // `Option` that keeps the first error.
    pub module_resolution_error: RefCell<Option<GoError>>,
}

// Go: fileloader.go:84 redirectsFile
#[derive(Clone, Debug, Default)]
pub struct RedirectsFile {
    // Index of file at which this redirect file needs to be iterated
    pub index: i32,
    pub file_name: String,
    pub path: Path,
    pub target: Path,
}

// Go: fileloader.go:92 DuplicateSourceFile
// PORT: Go keeps `Hash xxh3.Uint128` for the language server parse cache.
// Here `hash` is the hash that a parse cache set on the file
// (`ParsedSourceFile::hash`), and `text` is kept so the language server can
// hash it when none was set (`source_hash`), as Go `fh.Hash()`.
#[derive(Clone, Debug)]
pub struct DuplicateSourceFile {
    pub parse_options: SourceFileParseOptions,
    // ContentMapperParseOptions are the acquire-time options for a content-mapped parse-cache entry.
    // tsgo#4712
    pub content_mapper_parse_options: SourceFileParseOptions,
    pub text: FileText,
    pub hash: Option<u128>,
    pub script_kind: ScriptKind,
    // ContentMapper is the identity of the content mapper that produced this file,
    // or "" if the file is not content-mapped.
    // tsgo#4712
    pub content_mapper: String,
    // IsContentMapperFailureStub reports whether the file is an empty placeholder
    // from a failed transform.
    // tsgo#4712
    pub is_content_mapper_failure_stub: bool,
}

impl DuplicateSourceFile {
    /// Go `file.Hash`: `hash`, else the xxh3-128 of the text (see
    /// `ParsedSourceFile::source_hash`).
    #[must_use]
    pub fn source_hash(&self) -> u128 {
        self.hash
            .unwrap_or_else(|| xxhash_rust::xxh3::xxh3_128(self.text.as_bytes()))
    }
}

impl RedirectsFile {
    // Go: fileloader.go:108 (*redirectsFile).FileName
    pub fn file_name(&self) -> String {
        self.file_name.clone()
    }

    // Go: fileloader.go:109 (*redirectsFile).Path (at 673a5f17d713; ts#64159 renames it PathKey, compiler/fileloader.go:112)
    pub fn path(&self) -> Path {
        self.path.clone()
    }
}

// Go: fileloader.go:116 processedFiles
// PORT: Go nil maps that stay nil until first use are `Option`. Go
// `*includeProcessor` is owned by value. Go `UpdateProgram` copies this
// struct and so shares its maps with the old program. The maps that stay
// the same after loading are behind `Rc`, so a clone shares them too; a
// deep copy would cost time and memory once per edit. The module
// resolutions are behind `Arc`, because checker threads read the same map
// (`GoSharedState`) with no copy.
#[derive(Clone)]
pub struct ProcessedFiles {
    pub resolver: Option<Rc<dyn Resolver>>,
    pub files: Vec<Rc<ParsedSourceFile>>,
    // duplicateSourceFiles tracks parsed files loaded during program construction
    // that were later dropped from the final program, such as losing filename
    // casing variants for the same path or files hidden behind package redirect
    // deduplication. Their parse-cache acquires still need to be balanced when
    // the program is disposed.
    pub duplicate_source_files: Vec<DuplicateSourceFile>,
    pub files_by_path: FxHashMap<Path, Rc<ParsedSourceFile>>,
    pub project_reference_file_mapper: Option<Rc<RefCell<ProjectReferenceFileMapper>>>,
    pub missing_files: Vec<String>,
    pub resolved_modules: Arc<FxHashMap<Path, ModeAwareCache<Arc<ResolvedModule>>>>,
    pub type_resolutions_in_file:
        Rc<FxHashMap<Path, ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>>>,
    pub source_file_meta_datas: Rc<FxHashMap<Path, SourceFileMetaData>>,
    pub jsx_runtime_import_specifiers: Option<Rc<FxHashMap<Path, Rc<JsxRuntimeImportSpecifier>>>>,
    pub import_helpers_import_specifiers: Option<Rc<FxHashMap<Path, Node>>>,
    pub lib_files: Rc<FxHashMap<Path, Rc<LibFile>>>,
    // List of present unsupported extensions
    pub source_files_found_searching_node_modules: Rc<FxHashSet<Path>>,
    pub include_processor: IncludeProcessor,
    // if file was included using source file and its output is actually part of program
    // this contains mapping from output to source file
    pub output_file_to_project_reference_source: Option<Rc<FxHashMap<Path, String>>>,
    // Key is a file path. Value is the list of files that redirect to it (same package, different install location)
    pub redirect_targets_map: Option<Rc<FxHashMap<Path, Vec<String>>>>,
    // filesByPath for redirect files
    pub redirect_files_by_path: Option<Rc<FxHashMap<Path, RedirectsFile>>>,
    // Program-level diagnostics reported when a content mapper fails fatally (reported once per mapper).
    // tsgo#4712
    pub content_mapper_diagnostics: Vec<Diagnostic>,
    // Go `moduleResolutionError` (ts#64299) is a `Program` field since
    // ts#64519 (`NewProgram::module_resolution_error`).
    pub finished_processing: bool,
}

// Go: fileloader.go:161 jsxRuntimeImportSpecifier
#[derive(Clone, Debug)]
pub struct JsxRuntimeImportSpecifier {
    pub module_reference: String,
    pub specifier: Node,
}

// Go: fileloader.go:166 processAllProgramFiles
// PORT: Go ts#64519 returns the processed files, the resolver's
// `ResolutionData` and the module resolution error. The module part of
// ts#64519 is not ported, so the processed files keep the resolver and
// this returns the files and the error.
pub fn process_all_program_files(
    opts: ProgramConfig,
    hosts: ProgramHosts,
    factories: ProgramFactories,
    single_threaded: bool,
) -> (ProcessedFiles, Option<GoError>) {
    let ProgramHosts { host } = hosts;
    let compiler_options = opts.config.compiler_options().clone();
    let root_files: Vec<String> = opts.config.file_names().to_vec();
    let supported_extensions =
        get_supported_extensions(&compiler_options, &opts.config.content_mapper_extensions());
    let supported_extensions_with_json_if_resolve_json_module =
        get_supported_extensions_with_json_if_resolve_json_module(
            Some(&compiler_options),
            supported_extensions.clone(),
        );
    let mut max_node_module_js_depth = 0;
    if let Some(p) = opts.config.compiler_options().max_node_module_js_depth {
        // PORT: Go `int` (64-bit). The loader depth is `i32`; the file depth
        // is small and not negative, so a saturated limit compares the same.
        max_node_module_js_depth =
            i32::try_from(p).unwrap_or(if p < 0 { i32::MIN } else { i32::MAX });
    }
    let current_directory = host.get_current_directory().to_string();
    let mut loader = FileLoader {
        default_library_path: get_normalized_absolute_path(
            &host.default_library_path(),
            &current_directory,
        ),
        compare_paths_options: ComparePathsOptions {
            use_case_sensitive_file_names: host.fs().use_case_sensitive_file_names(),
            current_directory: current_directory.clone(),
        },
        files_parser: RefCell::new(FilesParser {
            queue: Vec::new(),
            task_data_by_path: FxHashMap::default(),
            max_depth: max_node_module_js_depth,
            single_threaded,
        }),
        root_tasks: Vec::with_capacity(
            root_files.len() + compiler_options.lib.as_ref().map_or(0, Vec::len),
        ),
        supported_extensions,
        supported_extensions_with_json_if_resolve_json_module,
        content_mapper_extensions: opts.config.content_mapper_extensions(),
        resolver: None,
        total_file_count: Cell::new(0),
        lib_file_count: Cell::new(0),
        // PORT: Go uses the zero `ast.NodeFactory`, which makes synthetic nodes.
        factory: NodeFactory::new(),
        project_reference_file_mapper: Rc::new(RefCell::new(ProjectReferenceFileMapper::new(
            &opts,
            host.clone(),
        ))),
        dts_directories: FxHashSet::default(),
        path_for_lib_file_cache: RefCell::new(FxHashMap::default()),
        path_for_lib_file_resolutions: RefCell::new(FxHashMap::default()),
        shared_resolution: None,
        keeps_import_names: false,
        noted_worker_logs: RefCell::new(FxHashSet::default()),
        content_mapper_failures: RefCell::new(FxHashMap::default()),
        content_mapper_init_failed: RefCell::new(FxHashSet::default()),
        content_mapper_diagnostics: RefCell::new(Vec::new()),
        concurrent_transforms: RefCell::new(FxHashMap::default()),
        mapped_prefetch_ready: Cell::new(false),
        module_resolution_error: RefCell::new(None),
        custom_module_resolver: factories.create_module_resolver.is_some(),
        opts,
        host,
    };
    loader.add_project_reference_tasks(single_threaded);
    let resolver_host: Rc<dyn ResolutionHost> = loader
        .project_reference_file_mapper
        .borrow()
        .host
        .clone()
        .expect("projectReferenceFileMapper.host is set until processing ends");
    let mut resolve_ahead = None;
    let resolver_options = ResolverOptions {
        host: Some(resolver_host),
        compiler_options: Some(compiler_options.clone()),
        typings_location: loader.opts.typings_location.clone(),
        project_name: loader.opts.project_name.clone(),
        extra_extensions: loader.opts.config.content_mapper_extensions(),
        package_json_cache: None,
    };
    if let Some(create_module_resolver) = factories.create_module_resolver {
        loader.resolver = Some(create_module_resolver(resolver_options));
    } else {
        let mut resolver = new_resolver(resolver_options);
        // PERF: Go resolves in all parse tasks with one shared cache. Here the
        // parse workers resolve ahead of the loader, and the loader reads their
        // answers. Only when every resolver sees the same files: the plain OS
        // file system (for `tsc -b`, the workers read the host's cache,
        // `BuildStatCache`), no project reference faking host (only a
        // program that uses the sources of its references has one) and no
        // traced resolution (Go then skips the cache too). A worker resolves
        // the files of a project reference with its redirect, from the
        // source file, as the loader does (Go `getRedirectForResolution`,
        // `WorkerResolveConfig::redirects`); the redirect is part of the
        // cache key, and the loader takes a file's answers only when the
        // worker used its redirect and containing file (`FilePrep::fits`).
        // Only for the default resolver: a parse worker cannot run a
        // `create_module_resolver` one.
        // PORT: with `skip_module_resolution` the loader resolves nothing
        // (ts#64024), so the workers do not either.
        if !single_threaded
            && super::files_parser::parse_workers_enabled()
            && workers_resolve_imports(&compiler_options)
            && !loader.opts.skip_module_resolution
            && loader.host.is_plain_os_fs()
            && compiler_options.trace_resolution != Tristate::True
            && !loader.opts.can_use_project_reference_source()
        {
            let shared = Arc::new(SharedResolutionCache::default());
            resolver.caches.shared = Some(SharedResolutionLink {
                cache: shared.clone(),
                publish: false,
            });
            loader.shared_resolution = Some(shared);
        } else if !single_threaded
            && super::resolve_ahead::mode() != super::resolve_ahead::Mode::Off
            && workers_resolve_imports(&compiler_options)
            && !loader.opts.skip_module_resolution
            && compiler_options.trace_resolution != Tristate::True
            && loader
                .opts
                .config
                .resolved_project_reference_paths()
                .is_empty()
            && let Some(host) = loader.host.resolve_ahead()
        {
            // PERF: a language server load after the first: workers resolve
            // the keys of the previous load ahead of the loader
            // (resolve_ahead.rs). Same conditions as above, but the host's
            // file system is not the plain OS one: the host checks each
            // answer before the loader takes it. No project references, so
            // no key has a redirect and the resolver host fakes no file.
            resolve_ahead = Some(super::resolve_ahead::ResolveAhead::start(host, &resolver));
            loader.keeps_import_names = true;
            drop_dead_import_names();
        }
        loader.resolver = Some(Rc::new(resolver));
    }
    let _trace = crate::tracing::get().map(|tr| {
        tr.push(
            crate::tracing::Phase::Program,
            "processRootFiles",
            vec![("count", root_files.len().into())],
            false,
        )
    });
    for (index, root_file) in root_files.iter().enumerate() {
        loader.add_root_file_task(
            root_file,
            None,
            new_file_include_reason(
                FileIncludeKind::ROOT_FILE,
                FileIncludeData::Index(index as i32),
            ),
        );
    }
    if !root_files.is_empty() && compiler_options.no_lib.is_false_or_unknown() {
        if let Some(libs) = &compiler_options.lib {
            for (index, lib) in libs.iter().enumerate() {
                let (name, ok) = get_lib_file_name(lib);
                if ok {
                    let lib_file = loader.path_for_lib_file(&name);
                    loader.add_root_task(
                        &lib_file.path,
                        Some(lib_file.clone()),
                        new_file_include_reason(
                            FileIncludeKind::LIB_FILE,
                            FileIncludeData::Index(index as i32),
                        ),
                    );
                }
                // !!! error on unknown name
            }
        } else {
            let name = get_default_lib_file_name(&compiler_options);
            let lib_file = loader.path_for_lib_file(&name);
            loader.add_root_task(
                &lib_file.path,
                Some(lib_file.clone()),
                new_file_include_reason(FileIncludeKind::LIB_FILE, FileIncludeData::None),
            );
        }
    }

    if !root_files.is_empty() && !loader.opts.skip_module_resolution {
        loader.add_automatic_type_directive_tasks();
    }

    let root_tasks = loader.root_tasks.clone();
    loader.files_parser.borrow_mut().parse(&loader, &root_tasks);
    if let Some(resolve_ahead) = resolve_ahead
        && let Some(resolver) = loader
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
    {
        resolve_ahead.finish(resolver);
    }
    // The parse workers have ended. The `tsc -b` host's cache keeps the
    // lookups of the worker answers that the loader took, as Go's cache
    // keeps the lookups of its parse tasks, and drops the other worker
    // lookups (`BuildStatCache`). Only the default resolver takes worker
    // answers.
    if let Some(stats) = loader.host.stat_cache() {
        let taken = loader
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
            .map(|resolver| resolver.take_worker_lookups())
            .unwrap_or_default();
        stats.end_load(&taken);
    }

    // The parse workers have ended: the program resolver's package.json
    // cache keeps the package.json texts that they read first and the
    // loader did not take, and parses one only when a lookup asks for it.
    if let Some(shared) = &loader.shared_resolution
        && let Some(resolver) = loader
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
    {
        shared.end_package_json_reads(&resolver.caches.package_json_info_cache);
    }

    // PORT: Go ts#64519 keeps the loader and the host in a
    // `projectReferenceFileMapperBuilder`, and the program gets only its
    // mapper. Here the mapper holds them while loading, and loses them now.
    {
        let mut mapper = loader.project_reference_file_mapper.borrow_mut();
        mapper.loader = None;
        mapper.host = None;
    }

    let files_parser = loader.files_parser.borrow();
    let processed_files = files_parser.get_processed_files(&loader);
    let module_resolution_error = loader.module_resolution_error.borrow().clone();
    drop(files_parser);
    drop(root_tasks);
    // PERF: in a one-program process the loader state (every parse task)
    // lives until the process ends anyway; freeing it is serial work before
    // the bind. Go leaves it to the garbage collector.
    if FORGET_LOADER_STATE.with(Cell::get) {
        std::mem::forget(loader);
    }
    (processed_files, module_resolution_error)
}

/// `process_all_program_files` of the flat options, for the tests.
#[cfg(test)]
fn process_all_program_files_of(opts: ProgramOptions, single_threaded: bool) -> ProcessedFiles {
    let (config, hosts, factories) = opts.split();
    process_all_program_files(config, hosts, factories, single_threaded).0
}

#[cfg(test)]
thread_local! {
    /// Each worker prep that the loads on this thread looked at: the file
    /// name and whether the loader could use it (`FilePrep::fits`).
    static PREP_FITS: std::cell::RefCell<Vec<(String, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// Set while `with_loader_state_forgotten` runs.
    static FORGET_LOADER_STATE: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, in which `process_all_program_files` does not free the loader
/// state (see there). Only for the program of a one-program process, which
/// is never freed.
pub fn with_loader_state_forgotten<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FORGET_LOADER_STATE.with(|forget| forget.set(self.0));
        }
    }
    let _restore = Restore(FORGET_LOADER_STATE.with(|forget| forget.replace(true)));
    f()
}

// Go: fileloader.go:465 contentMapperTransformDiagnostic (tsgo#4712)
// PORT: Go `*ast.SourceFile` is the file's SourceFile node. Go
// `errors.AsType` on a found `*TransformError` searches that error and the
// errors it wraps (`TransformError::to_go_error`).
fn content_mapper_transform_diagnostic(file: Node, label: &str, err: &GoError) -> Diagnostic {
    if let Some(collision) = errors::as_type::<SupplementalFileCollisionError>(err) {
        return content_mapper_transform_diagnostic_chain(
            file,
            label,
            diag::Content_mapper_supplemental_output_file_0_conflicts_with_an_existing_file,
            args![collision.file_name],
        );
    }
    if let Some(transform_error) = errors::as_type::<TransformError>(err) {
        let transform_err = transform_error.to_go_error();
        match transform_error.kind {
            TransformErrorKind::INITIALIZE => {
                if let Some(initialize_error) = errors::as_type::<InitializeError>(&transform_err) {
                    match initialize_error.kind {
                        InitializeErrorKind::POSITION_ENCODING => {
                            return content_mapper_transform_diagnostic_chain(
                                file,
                                label,
                                diag::The_content_mapper_selected_unsupported_position_encoding_0,
                                args![initialize_error.position_encoding.0],
                            );
                        }
                        InitializeErrorKind::EMPTY_DIAGNOSTIC_SOURCE => {
                            return content_mapper_transform_diagnostic_chain(
                                file,
                                label,
                                diag::The_content_mapper_diagnostic_source_must_not_be_empty,
                                args![],
                            );
                        }
                        InitializeErrorKind::RESERVED_DIAGNOSTIC_SOURCE => {
                            return content_mapper_transform_diagnostic_chain(
                                file,
                                label,
                                diag::The_content_mapper_diagnostic_source_0_is_reserved_by_TypeScript,
                                args![initialize_error.diagnostic_source],
                            );
                        }
                        _ => {}
                    }
                }
                return content_mapper_transform_diagnostic_chain(
                    file,
                    label,
                    diag::The_content_mapper_process_could_not_be_started_or_initialized,
                    args![],
                );
            }
            TransformErrorKind::PROJECT => {
                return content_mapper_transform_diagnostic_chain(
                    file,
                    label,
                    content_mapper_project_error_diagnostic(&transform_err),
                    args![],
                );
            }
            TransformErrorKind::REQUEST => {
                return content_mapper_transform_diagnostic_chain(
                    file,
                    label,
                    diag::The_content_mapper_process_failed_while_handling_the_transform_request,
                    args![],
                );
            }
            TransformErrorKind::RESPONSE => {
                if let Some(extension_error) =
                    errors::as_type::<InvalidVirtualExtensionError>(&transform_err)
                {
                    return content_mapper_transform_diagnostic_chain(
                        file,
                        label,
                        diag::The_content_mapper_returned_an_output_with_unsupported_virtual_extension_0,
                        args![extension_error.extension],
                    );
                }
                if let Some(directive_error) =
                    errors::as_type::<DiagnosticDirectiveError>(&transform_err)
                {
                    let detail = match directive_error.kind {
                        DiagnosticDirectiveErrorKind::INVALID_RANGE => Some(new_compiler_diagnostic(
                            diag::Diagnostic_directive_0_returned_by_the_content_mapper_has_an_invalid_range,
                            args![directive_error.index],
                        )),
                        DiagnosticDirectiveErrorKind::INVALID_POLICY => Some(new_compiler_diagnostic(
                            diag::The_content_mapper_returned_a_diagnostic_directive_with_invalid_policy_0,
                            args![directive_error.policy.0],
                        )),
                        DiagnosticDirectiveErrorKind::EXPECT_MISSING_UNUSED_DIAGNOSTIC => {
                            Some(new_compiler_diagnostic(
                                diag::Diagnostic_directive_0_returned_by_the_content_mapper_must_specify_unusedExpectDirectiveIndex_when_there_is_not_exactly_one_unusedExpectDirectiveDiagnostics_entry,
                                args![directive_error.index],
                            ))
                        }
                        DiagnosticDirectiveErrorKind::INVALID_UNUSED_DIAGNOSTIC_INDEX => {
                            Some(new_compiler_diagnostic(
                                diag::Diagnostic_directive_0_returned_by_the_content_mapper_has_an_invalid_unusedExpectDirectiveIndex,
                                args![directive_error.index],
                            ))
                        }
                        DiagnosticDirectiveErrorKind::OVERLAP => Some(new_compiler_diagnostic(
                            diag::The_content_mapper_returned_diagnostic_directives_with_overlapping_virtual_ranges,
                            args![],
                        )),
                        _ => None,
                    };
                    if let Some(mut detail) = detail {
                        if directive_error.supplemental_index >= 0 {
                            detail = new_diagnostic_chain(
                                Some(detail),
                                diag::The_invalid_diagnostic_directive_is_in_supplemental_output_0_returned_by_the_content_mapper,
                                args![directive_error.supplemental_index],
                            );
                        }
                        return content_mapper_transform_diagnostic_with_detail(
                            file, label, detail,
                        );
                    }
                }
                return content_mapper_transform_diagnostic_chain(
                    file,
                    label,
                    diag::The_content_mapper_returned_an_invalid_transform_response,
                    args![],
                );
            }
            TransformErrorKind::MAPPINGS => {
                return new_diagnostic(
                    file,
                    TextRange::new(0, 0),
                    diag::The_content_mapper_0_did_not_provide_the_required_position_mappings,
                    args![label],
                );
            }
            _ => {}
        }
    }
    new_diagnostic(
        file,
        TextRange::new(0, 0),
        diag::The_content_mapper_0_failed_to_transform_this_file,
        args![label],
    )
}

// Go: fileloader.go:521 ContentMapperProjectErrorDiagnostic (tsgo#4712)
// ContentMapperProjectErrorDiagnostic returns the localized diagnostic message for a project setup error.
pub fn content_mapper_project_error_diagnostic(err: &GoError) -> &'static Message {
    if let Some(project_error) = errors::as_type::<ProjectError>(err) {
        match project_error.kind {
            ProjectErrorKind::MALFORMED_RESPONSE => {
                return diag::The_content_mapper_returned_a_project_response_that_could_not_be_decoded;
            }
            ProjectErrorKind::MISSING_CONFIG_IDENTITY => {
                return diag::The_content_mapper_did_not_return_configIdentity_which_is_required_when_the_content_mapper_has_dynamicConfig_Colon_true_in_its_package_json;
            }
            ProjectErrorKind::NON_ABSOLUTE_WATCHED_FILE => {
                return diag::The_content_mapper_returned_a_non_absolute_path_in_watchedFiles;
            }
            ProjectErrorKind::UNEXPECTED_CONFIG_IDENTITY => {
                return diag::The_content_mapper_returned_configIdentity_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json;
            }
            ProjectErrorKind::UNEXPECTED_WATCHED_FILES => {
                return diag::The_content_mapper_returned_watchedFiles_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json;
            }
            _ => {}
        }
    }
    diag::The_content_mapper_process_failed_while_handling_the_project_request
}

// Go: fileloader.go:539 contentMapperTransformDiagnosticChain (tsgo#4712)
fn content_mapper_transform_diagnostic_chain(
    file: Node,
    label: &str,
    message: &'static Message,
    args: Vec<String>,
) -> Diagnostic {
    content_mapper_transform_diagnostic_with_detail(
        file,
        label,
        new_compiler_diagnostic(message, args),
    )
}

// Go: fileloader.go:543 contentMapperTransformDiagnosticWithDetail (tsgo#4712)
fn content_mapper_transform_diagnostic_with_detail(
    file: Node,
    label: &str,
    detail: Diagnostic,
) -> Diagnostic {
    let mut diagnostic = new_diagnostic(
        file,
        TextRange::new(0, 0),
        diag::The_content_mapper_0_failed_to_transform_this_file,
        args![label],
    );
    diagnostic.add_message_chain(Some(detail));
    diagnostic
}

// Go: fileloader.go:554 contentMapperMappingDiagnostic (tsgo#4712)
// contentMapperMappingDiagnostic builds the diagnostic reported against a mapper that produced an
// invalid span map, including the offsets involved so the mapper's author can locate the problem.
fn content_mapper_mapping_diagnostic(
    file: Node,
    label: &str,
    problem: &MappingError,
) -> Diagnostic {
    let loc = TextRange::new(0, 0);
    match problem.kind {
        MappingErrorKind::OVERLAP => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_produced_overlapping_or_out_of_order_position_mappings_near_virtual_offset_1,
            args![label, problem.virtual_pos],
        ),
        MappingErrorKind::OUT_OF_BOUNDS => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_produced_a_position_mapping_that_points_outside_the_original_content_original_offset_1,
            args![label, problem.original_pos],
        ),
        MappingErrorKind::VERBATIM_MISMATCH => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_produced_a_verbatim_mapping_that_does_not_match_the_original_content_virtual_offset_1_original_offset_2,
            args![label, problem.virtual_pos, problem.original_pos],
        ),
        MappingErrorKind::KIND => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_produced_a_position_mapping_with_an_invalid_kind_near_virtual_offset_1,
            args![label, problem.virtual_pos],
        ),
        MappingErrorKind::FEATURE => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_produced_invalid_mapping_features_near_original_offset_1,
            args![label, problem.original_pos],
        ),
        _ => new_diagnostic(
            file,
            loc,
            diag::The_content_mapper_0_did_not_provide_the_required_position_mappings,
            args![label],
        ),
    }
}

// Go: fileloader.go:599 ContentMapperInitializationDiagnostic (tsgo#4712)
// ContentMapperInitializationDiagnostic returns a fileless diagnostic for a mapper initialization failure.
pub fn content_mapper_initialization_diagnostic(label: &str, err: &GoError) -> Diagnostic {
    let initialize_error = errors::as_type::<InitializeError>(err);
    let label = match &initialize_error {
        Some(initialize_error) if label.is_empty() => initialize_error.mapper_name.clone(),
        _ => label.to_string(),
    };
    let mut diagnostic = new_compiler_diagnostic(
        diag::The_content_mapper_0_could_not_be_initialized,
        args![label],
    );
    let detail = initialize_error.and_then(|initialize_error| match initialize_error.kind {
        InitializeErrorKind::PROCESS_START => Some(new_compiler_diagnostic(
            diag::The_content_mapper_command_0_could_not_be_started_Colon_1,
            args![initialize_error.command, initialize_error.detail],
        )),
        InitializeErrorKind::PROCESS_EXIT => Some(new_compiler_diagnostic(
            diag::The_content_mapper_process_exited_before_responding_to_the_initialize_request_exit_code_0,
            args![initialize_error.exit_code],
        )),
        InitializeErrorKind::NO_RESPONSE => Some(new_compiler_diagnostic(
            diag::The_content_mapper_did_not_respond_to_the_initialize_request_within_0_seconds,
            args![initialize_error.timeout_seconds],
        )),
        InitializeErrorKind::INVALID_RESPONSE => Some(new_compiler_diagnostic(
            diag::The_content_mapper_returned_an_initialize_response_that_could_not_be_decoded_Colon_0,
            args![initialize_error.detail],
        )),
        InitializeErrorKind::REQUEST => Some(new_compiler_diagnostic(
            diag::The_content_mapper_s_initialize_request_failed_Colon_0,
            args![initialize_error.detail],
        )),
        InitializeErrorKind::POSITION_ENCODING => Some(new_compiler_diagnostic(
            diag::The_content_mapper_selected_unsupported_position_encoding_0,
            args![initialize_error.position_encoding.0],
        )),
        InitializeErrorKind::EMPTY_DIAGNOSTIC_SOURCE => Some(new_compiler_diagnostic(
            diag::The_content_mapper_diagnostic_source_must_not_be_empty,
            args![],
        )),
        InitializeErrorKind::RESERVED_DIAGNOSTIC_SOURCE => Some(new_compiler_diagnostic(
            diag::The_content_mapper_diagnostic_source_0_is_reserved_by_TypeScript,
            args![initialize_error.diagnostic_source],
        )),
        _ => None,
    });
    let detail = detail.unwrap_or_else(|| {
        new_compiler_diagnostic(
            diag::The_content_mapper_process_could_not_be_started_or_initialized,
            args![],
        )
    });
    diagnostic.add_message_chain(Some(detail));
    diagnostic
}

// Go: fileloader.go:628 ContentMapperProjectDiagnostic (tsgo#4712)
// ContentMapperProjectDiagnostic returns a fileless diagnostic for project setup or mapper initialization.
pub fn content_mapper_project_diagnostic(err: &GoError) -> Diagnostic {
    if errors::as_type::<InitializeError>(err).is_some() {
        return content_mapper_initialization_diagnostic("", err);
    }
    new_compiler_diagnostic(content_mapper_project_error_diagnostic(err), args![])
}

impl FileLoader {
    // Go: fileloader.go:238 (*fileLoader).toPath
    pub fn to_path(&self, file: &str) -> Path {
        to_path(
            file,
            &self.host.get_current_directory(),
            self.host.fs().use_case_sensitive_file_names(),
        )
    }

    // Go: fileloader.go:242 (*fileLoader).addRootTask
    pub fn add_root_task(
        &mut self,
        file_name: &str,
        lib_file: Option<Rc<LibFile>>,
        include_reason: Rc<FileIncludeReason>,
    ) {
        let abs_path = get_normalized_absolute_path(file_name, &self.host.get_current_directory());
        if self
            .opts
            .config
            .compiler_options()
            .allow_non_ts_extensions
            .is_true()
            || has_extension(&abs_path)
        {
            self.root_tasks.push(Rc::new(RefCell::new(ParseTask {
                normalized_file_path: abs_path,
                lib_file,
                include_reason: Some(include_reason),
                ..Default::default()
            })));
        }
    }

    // Go: fileloader.go:253 (*fileLoader).addRootFileTask
    pub fn add_root_file_task(
        &mut self,
        file_name: &str,
        lib_file: Option<Rc<LibFile>>,
        include_reason: Rc<FileIncludeReason>,
    ) {
        let curr_dir = self.host.get_current_directory().to_string();
        let abs_path = get_normalized_absolute_path(file_name, &curr_dir);
        let (resolved_file, diagnostic) = self.get_source_file_from_reference(&abs_path, file_name);
        let mut root_task = ParseTask {
            normalized_file_path: resolved_file,
            lib_file,
            include_reason: Some(include_reason.clone()),
            ..Default::default()
        };
        if let Some(diagnostic) = diagnostic {
            root_task.normalized_file_path = abs_path;
            root_task.failed_lookup = true;
            root_task.processing_diagnostics = vec![new_explaining_processing_diagnostic(
                Some(include_reason),
                diagnostic.message,
                diagnostic.args,
            )];
        }
        self.root_tasks.push(Rc::new(RefCell::new(root_task)));
    }

    // Go: fileloader.go:277 (*fileLoader).addAutomaticTypeDirectiveTasks
    pub fn add_automatic_type_directive_tasks(&mut self) {
        let containing_directory;
        let compiler_options = self.opts.config.compiler_options();
        if !compiler_options.config_file_path.is_empty() {
            containing_directory = get_directory_path(&compiler_options.config_file_path);
        } else {
            containing_directory = self.host.get_current_directory().to_string();
        }
        let containing_file_name =
            combine_paths(&containing_directory, &[INFERRED_TYPES_CONTAINING_FILE]);
        self.root_tasks.push(Rc::new(RefCell::new(ParseTask {
            normalized_file_path: containing_file_name,
            is_for_automatic_type_directive: true,
            ..Default::default()
        })));
    }

    // Go: fileloader.go:286 (*fileLoader).resolveAutomaticTypeDirectives
    #[allow(clippy::type_complexity)]
    pub fn resolve_automatic_type_directives(
        &self,
        containing_file_name: &str,
    ) -> (
        Vec<ResolvedRef>,
        ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>,
        Vec<DiagAndArgs>,
        Vec<Rc<ProcessingDiagnostic>>,
    ) {
        let mut to_parse: Vec<ResolvedRef> = Vec::new();
        let mut type_resolutions_in_file: ModeAwareCache<Rc<ResolvedTypeReferenceDirective>> =
            ModeAwareCache::default();
        let mut type_resolutions_trace: Vec<DiagAndArgs> = Vec::new();
        let mut p_diagnostics: Vec<Rc<ProcessingDiagnostic>> = Vec::new();
        // PORT: Go passes the compiler host as a `module.ResolutionHost`.
        let host = CompilerResolutionHost::new(self.host.clone());
        let host: &dyn ResolutionHost = &host;
        let automatic_type_directive_names =
            get_automatic_type_directive_names(self.opts.config.compiler_options(), host);
        if !automatic_type_directive_names.is_empty() {
            to_parse.reserve(automatic_type_directive_names.len());
            for name in &automatic_type_directive_names {
                // Under node16/nodenext module resolution, load `types`/ata include names as cjs resolution results by passing an `undefined` mode.
                // Under bundler module resolution, this also triggers the "import" condition to be used.
                let resolution_mode = RESOLUTION_MODE_NONE;
                let (resolved, trace) = self.resolver().resolve_type_reference_directive(
                    name,
                    containing_file_name,
                    resolution_mode,
                    None,
                );
                let trace_done = crate::tracing::get().map(|tr| {
                    tr.push(
                        crate::tracing::Phase::Program,
                        "processTypeReferenceDirective",
                        vec![
                            ("directive", name.clone().into()),
                            ("hasResolved", resolved.is_resolved().into()),
                            (
                                "refKind",
                                FileIncludeKind::AUTOMATIC_TYPE_DIRECTIVE_FILE.0.into(),
                            ),
                        ],
                        false,
                    )
                });
                type_resolutions_in_file.insert(
                    ModeAwareCacheKey {
                        name: name.clone(),
                        mode: resolution_mode,
                    },
                    resolved.clone(),
                );
                type_resolutions_trace.extend(trace);
                if resolved.is_resolved() {
                    to_parse.push(ResolvedRef {
                        file_name: resolved.resolved_file_name.clone(),
                        increase_depth: resolved.is_external_library_import,
                        elide_on_depth: false,
                        include_reason: Some(new_file_include_reason(
                            FileIncludeKind::AUTOMATIC_TYPE_DIRECTIVE_FILE,
                            FileIncludeData::AutomaticTypeDirectiveFile(
                                AutomaticTypeDirectiveFileData {
                                    type_reference: name.clone(),
                                    package_id: resolved.package_id.clone(),
                                },
                            ),
                        )),
                        package_id: resolved.package_id.clone(),
                    });
                } else {
                    p_diagnostics.push(new_explaining_processing_diagnostic(
                        Some(new_file_include_reason(
                            FileIncludeKind::AUTOMATIC_TYPE_DIRECTIVE_FILE,
                            FileIncludeData::AutomaticTypeDirectiveFile(
                                AutomaticTypeDirectiveFileData {
                                    type_reference: name.clone(),
                                    package_id: PackageId::default(),
                                },
                            ),
                        )),
                        diag::Cannot_find_type_definition_file_for_0,
                        args![name],
                    ));
                }
                drop(trace_done);
            }
        }
        (
            to_parse,
            type_resolutions_in_file,
            type_resolutions_trace,
            p_diagnostics,
        )
    }

    // Go: fileloader.go:340 (*fileLoader).addProjectReferenceTasks
    // PORT: Go makes the project reference file mapper here. The port makes
    // it in `process_all_program_files`, because the loader struct needs a
    // value for the field. It is made from the same `opts` and host, so the
    // result is the same.
    pub fn add_project_reference_tasks(&mut self, single_threaded: bool) {
        let project_references = self.opts.config.resolved_project_reference_paths().to_vec();
        if project_references.is_empty() {
            return;
        }

        let mut parser = ProjectReferenceParser::new(self, single_threaded);
        let root_tasks = create_project_reference_parse_tasks(&project_references);
        parser.parse(root_tasks);
    }

    // Go: fileloader.go:361 (*fileLoader).sortLibs
    // PORT: Go `slices.SortFunc` is pdqsort. It is not stable for more than
    // 12 items, so libs with the same priority can change places there.
    // `gostd::slices::sort_func` is the same pdqsort, so they move as in Go.
    pub fn sort_libs(&self, lib_files: &mut [Rc<ParsedSourceFile>]) {
        // Go: fileloader.go:362 slices.SortFunc(libFiles, cmp.Compare on the priorities)
        crate::gostd::slices::sort_func(lib_files, |f1, f2| {
            self.get_default_lib_file_priority(f1)
                .cmp(&self.get_default_lib_file_priority(f2)) as i32
        });
    }

    // Go: fileloader.go:367 (*fileLoader).getDefaultLibFilePriority
    pub fn get_default_lib_file_priority(&self, a: &ParsedSourceFile) -> usize {
        // defaultLibraryPath and a.FileName() are absolute and normalized; a prefix check should suffice.
        let default_library_path = remove_trailing_directory_separator(&self.default_library_path);
        let a_file_name = a.file_name();

        if a_file_name.starts_with(default_library_path)
            && a_file_name.len() > default_library_path.len()
            && a_file_name.as_bytes()[default_library_path.len()] == DIRECTORY_SEPARATOR
        {
            // avoid tspath.GetBaseFileName; we know these paths are already absolute and normalized.
            let basename = &a_file_name[a_file_name
                .rfind(DIRECTORY_SEPARATOR as char)
                .map_or(0, |i| i + 1)..];
            if basename == "lib.d.ts" || basename == "lib.es6.d.ts" {
                return 0;
            }
            let without_prefix = basename.strip_prefix("lib.").unwrap_or(basename);
            let name = without_prefix
                .strip_suffix(".d.ts")
                .unwrap_or(without_prefix);
            if let Some(index) = LIBS.iter().position(|lib| lib == name) {
                return index + 1;
            }
        }
        LIBS.len() + 2
    }

    // Go: fileloader.go:382 (*fileLoader).loadSourceFileMetaData
    pub fn load_source_file_meta_data(&self, file_name: &str) -> SourceFileMetaData {
        if self.opts.skip_module_resolution {
            return SourceFileMetaData {
                implied_node_format: get_implied_node_format_for_file(file_name, ""),
                ..SourceFileMetaData::default()
            };
        }

        source_file_meta_data(
            self.resolver(),
            self.opts.config.compiler_options(),
            file_name,
        )
    }

    // Go: fileloader.go:412 (*fileLoader).parseSourceFile
    pub fn parse_source_file(&self, t: &ParseTask) -> Option<Rc<ParsedSourceFile>> {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Parse,
                "createSourceFile",
                vec![("path", t.normalized_file_path.clone().into())],
                true,
            )
        });
        let path = self.to_path(&t.normalized_file_path);
        let options = self
            .project_reference_file_mapper
            .borrow()
            .get_compiler_options_for_file(&new_has_file_name(&t.normalized_file_path, &t.path));
        let parse_options = SourceFileParseOptions {
            file_name: t.normalized_file_path.clone(),
            path,
            external_module_indicator_options: get_external_module_indicator_options(
                &t.normalized_file_path,
                &options,
                &t.metadata,
            ),
        };
        // tsgo#4712
        // PERF: with no content mappers (most programs) the list is empty,
        // so the check makes no list.
        if !self.content_mapper_extensions.is_empty()
            && file_extension_is_one_of(
                &t.normalized_file_path,
                &self
                    .content_mapper_extensions
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        {
            return self.parse_content_mapped_file(parse_options);
        }
        self.host.get_source_file(&parse_options)
    }

    // Go: fileloader.go:436 (*fileLoader).parseContentMappedFile (tsgo#4712)
    // parseContentMappedFile produces a content-mapped virtual source file via the host's content
    // mapper, preserving the original file name and retaining the untransformed text on the
    // source file. Content mapper extensions only reach the parser when content mappers are configured.
    //
    // When initialization fails, one program diagnostic is reported and the mapper is not attempted for
    // subsequent files. Other failures produce per-file diagnostics and count toward a failure budget; after
    // maxContentMapperFailures, one program diagnostic reports that the mapper was disabled and subsequent
    // files are silently substituted with empty files. It returns nil only if the file cannot be read.
    // PORT: the content mapper host is dispatch-thread state
    // (`contentmapper` module docs), so the host transforms on this (the
    // loading) thread. Once that opened the mapper project, the parse
    // workers send the transforms of the later files, and the host takes
    // their results (`note_content_mapper_transform`).
    pub fn parse_content_mapped_file(
        &self,
        opts: SourceFileParseOptions,
    ) -> Option<Rc<ParsedSourceFile>> {
        // PORT: Go calls methods on the (never nil) mapper, which panics
        // for nil.
        let mapper = self
            .opts
            .config
            .get_content_mapper_for_file_name(&opts.file_name)
            .expect("nil pointer dereference: content mapper");
        let label = mapper.diagnostic_name();
        let transform_identity = self.get_content_mapper_transform_identity(&mapper);
        if self.content_mapper_unavailable(Some(&mapper)) {
            // The mapper failed initialization or exceeded its failure budget; add the file empty without re-reporting.
            return Some(Rc::new(self.empty_content_mapped_file(
                &opts,
                &mapper.identity(),
                &transform_identity,
            )));
        }
        let files = self.host.get_content_mapped_source_files(&opts, &mapper);
        self.note_content_mapper_transform(&mapper);
        match files {
            Ok(files) => files.canonical,
            Err(err) => {
                let mut source_file =
                    self.empty_content_mapped_file(&opts, &mapper.identity(), &transform_identity);
                if let Some(transform_error) = errors::as_type::<TransformError>(&err)
                    && transform_error.kind == TransformErrorKind::INITIALIZE
                {
                    self.record_content_mapper_initialization_failure(
                        &mapper,
                        &label,
                        &transform_error.to_go_error(),
                    );
                    return Some(Rc::new(source_file));
                }
                if self.record_content_mapper_failure(&mapper, &label) {
                    let diagnostic = if let Some(problem) = errors::as_type::<MappingError>(&err) {
                        content_mapper_mapping_diagnostic(source_file.root, &label, &problem)
                    } else {
                        content_mapper_transform_diagnostic(source_file.root, &label, &err)
                    };
                    // PORT: Go `sourceFile.SetDiagnostics(append(sourceFile.Diagnostics(), diagnostic))`.
                    // The parsed file keeps its diagnostics in its `diagnostics`
                    // field and in the table that `parse_source_file` fills;
                    // both change.
                    let mut diagnostics = source_file.diagnostics.clone();
                    diagnostics.push(diagnostic);
                    source_file.diagnostics = diagnostics.clone();
                    crate::frontend::parser::source_file::set_source_file_diagnostics(
                        source_file.root,
                        diagnostics,
                    );
                }
                Some(Rc::new(source_file))
            }
        }
    }

    // Go: fileloader.go:576 (*fileLoader).getContentMapperTransformIdentity (tsgo#4712)
    // PORT: Go `fmt.Sprintf("%x", u.Bytes())` of the `xxh3.Uint128` is the
    // 32 hex digits of the `u128`.
    fn get_content_mapper_transform_identity(&self, mapper: &Rc<Mapper>) -> String {
        if let Some(project) = self.host.content_mapper_project()
            && let Ok(identity) = project.identity(mapper)
        {
            return identity;
        }
        format!(
            "{:032x}",
            mapper.transform_identity(Some(&**self.opts.config.compiler_options()))
        )
    }

    // Go: fileloader.go:585 (*fileLoader).emptyContentMappedFile (tsgo#4712)
    // emptyContentMappedFile produces an empty TypeScript source file for a content-mapped file whose
    // transform could not be used, retaining the original content for diagnostics. Importers see it as an
    // empty module rather than triggering a "cannot find module" error. It is still marked as content-mapped
    // so it is excluded from emit like a successfully mapped file.
    // (Go has this comment above getContentMapperTransformIdentity.)
    // PORT: returns the file before it goes in an `Rc`, so the caller can
    // still add a diagnostic (Go `SetDiagnostics` after the call).
    fn empty_content_mapped_file(
        &self,
        opts: &SourceFileParseOptions,
        mapper_identity: &str,
        transform_identity: &str,
    ) -> ParsedSourceFile {
        let (content, _) = self.host.fs().read_file(&opts.file_name);
        let source_file = parse_source_file(opts, "", ScriptKind::TS);
        source_file.set_content_mapper_info(crate::ast::ContentMapperSourceFileInfo {
            content_mapper: mapper_identity.to_string(),
            transform_identity: transform_identity.to_string(),
            parse_options: opts.clone(),
            virtual_file_name: format!("{}{}", opts.file_name, EXTENSION_TS),
            original_text: content,
            span_map: None,
            diagnostic_directives: Vec::new(),
            supplemental_source_files: Vec::new(),
            canonical_source_file: None,
        });
        source_file
    }

    /// What a parse worker sends the transform of the content-mapped file
    /// `file_name` with (`FilesParser::prefetch_request`): `None` until
    /// the loader's transform of a file of its mapper opened the mapper
    /// project, and after the mapper is disabled.
    // PORT: not in Go, where the parse goroutines transform (tsgo#4712).
    pub(crate) fn concurrent_content_mapper_transform(
        &self,
        file_name: &str,
    ) -> Option<Arc<ConcurrentTransform>> {
        let transforms = self.concurrent_transforms.borrow();
        if transforms.is_empty() {
            return None;
        }
        let mapper = self
            .opts
            .config
            .get_content_mapper_for_file_name(file_name)?;
        if self.content_mapper_unavailable(Some(&mapper)) {
            return None;
        }
        transforms.get(&Rc::as_ptr(&mapper)).cloned()
    }

    /// After the loader's transform of a file of `mapper`: when the host
    /// lets the parse workers transform (`prefetch_content_mapped`) and the
    /// transform opened the mapper project, keeps what the workers send
    /// the later transforms with. Go opens the project on the first
    /// transform too, so no request goes out that Go does not send.
    /// `GOPORT_MAPPED_PREFETCH=0` turns it off (an A/B switch).
    // PORT: not in Go (see `concurrent_transforms`).
    fn note_content_mapper_transform(&self, mapper: &Rc<Mapper>) {
        let key = Rc::as_ptr(mapper);
        if self.concurrent_transforms.borrow().contains_key(&key)
            || !self.host.prefetch_content_mapped()
            || std::env::var_os("GOPORT_MAPPED_PREFETCH").is_some_and(|value| value == "0")
        {
            return;
        }
        let Some(transform) = self
            .host
            .content_mapper_project()
            .and_then(|project| project.concurrent_transform(mapper))
        else {
            return;
        };
        self.concurrent_transforms
            .borrow_mut()
            .insert(key, transform);
        self.mapped_prefetch_ready.set(true);
    }

    /// Stops the worker transforms of a mapper that the loader disabled.
    // PORT: not in Go (see `concurrent_transforms`).
    fn disable_concurrent_transform(&self, mapper: &Rc<Mapper>) {
        if let Some(transform) = self.concurrent_transforms.borrow().get(&Rc::as_ptr(mapper)) {
            transform.disable();
        }
    }

    // Go: fileloader.go:636 (*fileLoader).contentMapperUnavailable (tsgo#4712)
    // contentMapperUnavailable reports whether mapper failed initialization or exceeded its failure budget.
    fn content_mapper_unavailable(&self, mapper: Option<&Rc<Mapper>>) -> bool {
        let Some(mapper) = mapper else {
            return false;
        };
        let key = Rc::as_ptr(mapper);
        self.content_mapper_init_failed.borrow().contains(&key)
            || self
                .content_mapper_failures
                .borrow()
                .get(&key)
                .copied()
                .unwrap_or(0)
                >= MAX_CONTENT_MAPPER_FAILURES
    }

    // Go: fileloader.go:645 (*fileLoader).recordContentMapperInitializationFailure (tsgo#4712)
    fn record_content_mapper_initialization_failure(
        &self,
        mapper: &Rc<Mapper>,
        label: &str,
        err: &GoError,
    ) {
        if !self
            .content_mapper_init_failed
            .borrow_mut()
            .insert(Rc::as_ptr(mapper))
        {
            return;
        }
        self.content_mapper_diagnostics
            .borrow_mut()
            .push(content_mapper_initialization_diagnostic(label, err));
        self.disable_concurrent_transform(mapper);
    }

    // Go: fileloader.go:658 (*fileLoader).recordContentMapperFailure (tsgo#4712)
    // recordContentMapperFailure counts a transform failure for mapper. It returns whether the failure
    // should be reported for this file (false once the mapper is already disabled). On the failure that
    // reaches maxContentMapperFailures it appends a single program diagnostic disabling the mapper.
    fn record_content_mapper_failure(&self, mapper: &Rc<Mapper>, label: &str) -> bool {
        let mut failures = self.content_mapper_failures.borrow_mut();
        let count = failures.entry(Rc::as_ptr(mapper)).or_insert(0);
        if *count >= MAX_CONTENT_MAPPER_FAILURES {
            return false;
        }
        *count += 1;
        if *count >= MAX_CONTENT_MAPPER_FAILURES {
            self.content_mapper_diagnostics
                .borrow_mut()
                .push(new_compiler_diagnostic(
                    diag::The_content_mapper_0_failed_1_times_and_will_not_be_used,
                    args![label, MAX_CONTENT_MAPPER_FAILURES],
                ));
            drop(failures);
            self.disable_concurrent_transform(mapper);
        }
        true
    }

    // Go: fileloader.go:678 (*fileLoader).isSupportedExtension
    pub fn is_supported_extension(&self, canonical_file_name: &str) -> bool {
        for group in &self.supported_extensions_with_json_if_resolve_json_module {
            let group: Vec<&str> = group.iter().map(String::as_str).collect();
            if file_extension_is_one_of(canonical_file_name, &group) {
                return true;
            }
        }
        false
    }

    // Go: fileloader.go:682 (*fileLoader).getSourceFileFromReference
    // ts#64159: the "A file cannot have a reference to itself" check moved
    // to `resolve_tripleslash_path_reference`, after the extension lookup.
    pub fn get_source_file_from_reference(
        &self,
        file_name: &str,
        reference_text: &str,
    ) -> (String, Option<SourceFileFromReferenceDiagnostic>) {
        let options = self.opts.config.compiler_options();
        let allow_non_ts_extensions = options.allow_non_ts_extensions.is_true();
        let diagnostic_file_name = normalize_slashes(reference_text);
        let fs = self.host.fs();

        if has_extension(file_name) {
            let canonical_file_name =
                get_canonical_file_name(file_name, fs.use_case_sensitive_file_names());
            if !allow_non_ts_extensions && !self.is_supported_extension(&canonical_file_name) {
                if has_js_file_extension(&canonical_file_name) {
                    return (
                        String::new(),
                        Some(SourceFileFromReferenceDiagnostic {
                            message: diag::File_0_is_a_JavaScript_file_Did_you_mean_to_enable_the_allowJs_option,
                            args: args![diagnostic_file_name],
                        }),
                    );
                }
                return (
                    String::new(),
                    Some(SourceFileFromReferenceDiagnostic {
                        message: diag::File_0_has_an_unsupported_extension_The_only_supported_extensions_are_1,
                        args: args![
                            diagnostic_file_name,
                            format!("'{}'", join_flattened_extensions(&self.supported_extensions))
                        ],
                    }),
                );
            }

            if !fs.file_exists(file_name) {
                return (
                    String::new(),
                    Some(SourceFileFromReferenceDiagnostic {
                        message: diag::File_0_not_found,
                        args: args![diagnostic_file_name],
                    }),
                );
            }
            return (file_name.to_string(), None);
        }

        if allow_non_ts_extensions && fs.file_exists(file_name) {
            return (file_name.to_string(), None);
        }

        if allow_non_ts_extensions {
            return (
                String::new(),
                Some(SourceFileFromReferenceDiagnostic {
                    message: diag::File_0_not_found,
                    args: args![diagnostic_file_name],
                }),
            );
        }

        for ext in &self.supported_extensions[0] {
            let candidate = format!("{file_name}{ext}");
            if fs.file_exists(&candidate) {
                return (candidate, None);
            }
        }

        (
            String::new(),
            Some(SourceFileFromReferenceDiagnostic {
                message: diag::Could_not_resolve_the_path_0_with_the_extensions_Colon_1,
                args: args![
                    diagnostic_file_name,
                    format!(
                        "'{}'",
                        join_flattened_extensions(&self.supported_extensions)
                    )
                ],
            }),
        )
    }

    // Go: fileloader.go:723 (*fileLoader).resolveTripleslashPathReference
    pub fn resolve_tripleslash_path_reference(
        &self,
        module_name: &str,
        containing_file: &str,
        index: i32,
    ) -> (Option<ResolvedRef>, Option<Rc<ProcessingDiagnostic>>) {
        let base_path = get_directory_path(containing_file);
        let mut referenced_file_name = module_name.to_string();

        if !is_rooted_disk_path(module_name) {
            referenced_file_name = combine_paths(&base_path, &[module_name]);
        }
        let normalized_file_name = normalize_path(&referenced_file_name);
        let containing_path = self.to_path(containing_file);
        let include_reason = new_file_include_reason(
            FileIncludeKind::REFERENCE_FILE,
            FileIncludeData::ReferencedFile(ReferencedFileData {
                file: containing_path.clone(),
                index,
                synthetic: Node::NIL,
            }),
        );

        let (resolved_file_name, diagnostic) =
            self.get_source_file_from_reference(&normalized_file_name, module_name);
        if let Some(diagnostic) = diagnostic {
            return (
                None,
                Some(new_explaining_processing_diagnostic(
                    Some(include_reason),
                    diagnostic.message,
                    diagnostic.args,
                )),
            );
        }
        // ts#64159 (fileloader.go:747): the check is on the resolved file,
        // so `/// <reference path="a" />` in a.ts (found as "a" + ".ts")
        // is an error too. N checked only a name with an extension.
        if containing_path == self.to_path(&resolved_file_name) {
            return (
                None,
                Some(new_explaining_processing_diagnostic(
                    Some(include_reason),
                    diag::A_file_cannot_have_a_reference_to_itself,
                    Vec::new(),
                )),
            );
        }

        (
            Some(ResolvedRef {
                file_name: resolved_file_name,
                include_reason: Some(include_reason),
                ..Default::default()
            }),
            None,
        )
    }

    // Go: fileloader.go:765 (*fileLoader).resolveTypeReferenceDirectives
    pub fn resolve_type_reference_directives(&self, t: &mut ParseTask) {
        let file = t
            .file
            .clone()
            .expect("resolveTypeReferenceDirectives runs on a parsed file");
        if file.type_reference_directives.is_empty() {
            return;
        }
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Program,
                "resolveTypeReferenceDirectiveNamesWorker",
                vec![("containingFileName", file.file_name().to_string().into())],
                false,
            )
        });
        let meta = t.metadata.clone();

        let mut type_resolutions_in_file: ModeAwareCache<Rc<ResolvedTypeReferenceDirective>> =
            ModeAwareCache::default();
        type_resolutions_in_file.reserve(file.type_reference_directives.len());
        let mut type_resolutions_trace: Vec<DiagAndArgs> = Vec::new();
        for (index, ref_) in file.type_reference_directives.iter().enumerate() {
            let (redirect, file_name) = self
                .project_reference_file_mapper
                .borrow()
                .get_redirect_for_resolution(&new_has_file_name(file.file_name(), file.path()));
            let redirect_ref = redirect
                .as_deref()
                .map(|r| r as &dyn ModuleResolvedProjectReference);
            let resolution_mode = get_mode_for_type_reference_directive_in_file(
                ref_,
                &file,
                &meta,
                &get_compiler_options_with_redirect(
                    self.opts.config.compiler_options(),
                    redirect_ref,
                ),
            );
            let (resolved, trace) = self.resolver().resolve_type_reference_directive(
                &ref_.file_name,
                &file_name,
                resolution_mode,
                redirect_ref,
            );
            let trace_done = crate::tracing::get().map(|tr| {
                tr.push(
                    crate::tracing::Phase::Program,
                    "processTypeReferenceDirective",
                    vec![
                        ("directive", ref_.file_name.clone().into()),
                        ("hasResolved", resolved.is_resolved().into()),
                        (
                            "refKind",
                            FileIncludeKind::TYPE_REFERENCE_DIRECTIVE.0.into(),
                        ),
                        ("refPath", t.path.0.clone().into()),
                    ],
                    false,
                )
            });
            type_resolutions_in_file.insert(
                ModeAwareCacheKey {
                    name: ref_.file_name.clone(),
                    mode: resolution_mode,
                },
                resolved.clone(),
            );
            let include_reason = new_file_include_reason(
                FileIncludeKind::TYPE_REFERENCE_DIRECTIVE,
                FileIncludeData::ReferencedFile(ReferencedFileData {
                    file: t.path.clone(),
                    index: index as i32,
                    synthetic: Node::NIL,
                }),
            );
            type_resolutions_trace.extend(trace);

            if resolved.is_resolved() {
                t.add_sub_task(
                    ResolvedRef {
                        file_name: resolved.resolved_file_name.clone(),
                        increase_depth: resolved.is_external_library_import,
                        elide_on_depth: false,
                        include_reason: Some(include_reason),
                        package_id: resolved.package_id.clone(),
                    },
                    None,
                );
            } else {
                t.processing_diagnostics
                    .push(new_unknown_reference_processing_diagnostic(include_reason));
            }
            drop(trace_done);
        }

        t.type_resolutions_in_file = type_resolutions_in_file;
        t.type_resolutions_trace = type_resolutions_trace;
    }

    // Go: fileloader.go:819 externalHelpersModuleNameText
    // PORT: the Go constant is `EXTERNAL_HELPERS_MODULE_NAME_TEXT` in
    // checker/types.rs. It is reused here.

    // Go: fileloader.go:821 (*fileLoader).resolveImportsAndModuleAugmentations
    pub(crate) fn resolve_imports_and_module_augmentations(
        &self,
        t: &mut ParseTask,
        prep: Option<TakenParse>,
    ) {
        let _trace = crate::tracing::get().map(|tr| {
            let containing_file_name = t
                .file
                .as_ref()
                .expect("resolveImportsAndModuleAugmentations runs on a parsed file")
                .file_name()
                .to_string();
            tr.push(
                crate::tracing::Phase::Program,
                "resolveModuleNamesWorker",
                vec![("containingFileName", containing_file_name.into())],
                false,
            )
        });
        let file = t
            .file
            .clone()
            .expect("resolveImportsAndModuleAugmentations runs on a parsed file");
        let meta = t.metadata.clone();

        let mut module_names: Vec<Node> =
            Vec::with_capacity(file.imports.len() + file.module_augmentations.len() + 2);

        let (redirect, file_name) = self
            .project_reference_file_mapper
            .borrow()
            .get_redirect_for_resolution(&new_has_file_name(file.file_name(), file.path()));
        let redirect_ref = redirect
            .as_deref()
            .map(|r| r as &dyn ModuleResolvedProjectReference);
        let options_for_file =
            get_compiler_options_with_redirect(self.opts.config.compiler_options(), redirect_ref);
        let synthetic = SyntheticImports::of(&file, &options_for_file);
        if synthetic.helpers {
            let specifier = self.create_synthetic_import(EXTERNAL_HELPERS_MODULE_NAME_TEXT, &file);
            module_names.push(specifier);
            t.import_helpers_import_specifier = specifier;
        }

        if !synthetic.jsx.is_empty() {
            let specifier = self.create_synthetic_import(&synthetic.jsx, &file);
            module_names.push(specifier);
            t.jsx_runtime_import_specifier = Some(Rc::new(JsxRuntimeImportSpecifier {
                module_reference: synthetic.jsx.clone(),
                specifier,
            }));
        }

        let imports_start = module_names.len() as i32;

        module_names.extend(file.imports.iter().copied());
        for imp in &file.module_augmentations {
            if imp.kind() == SyntaxKind::StringLiteral {
                module_names.push(*imp);
            }
            // Do nothing if it's an Identifier; we don't need to do module resolution for `declare global`.
        }

        if self.opts.skip_module_resolution {
            return;
        }

        if !module_names.is_empty() {
            // PERF (loadpar1): the answers that the parse worker of this
            // file resolved, when it resolved for this loader. Else every
            // name resolves here, as in Go.
            let prep = prep
                .filter(|_| self.shared_resolution.is_some() && crate::tracing::get().is_none())
                .and_then(TakenParse::take_prep)
                .filter(|prep| {
                    let usable = prep.fits(
                        &meta,
                        redirect_ref.map_or("", |r| r.config_name()),
                        &file_name,
                        &synthetic,
                        file.imports.len(),
                    );
                    if !usable {
                        count_prep(|c| c.preps_unfit += 1);
                    }
                    #[cfg(test)]
                    PREP_FITS.with(|fits| {
                        fits.borrow_mut()
                            .push((file.file_name().to_string(), usable));
                    });
                    usable
                });
            let (mut names_taken, mut names_own) = (0, 0);
            let mut resolutions_in_file: ModeAwareCache<Arc<ResolvedModule>> =
                ModeAwareCache::default();
            resolutions_in_file.reserve(module_names.len());
            let mut resolutions_trace: Vec<DiagAndArgs> = Vec::new();
            // PERF: the names, usages and JSDoc flags of the imports and
            // module augmentations of a parse that the loads keep
            // (`import_names`), and the file's emit format, once per file.
            let kept = self
                .keeps_import_names
                .then(|| import_names(&file))
                .filter(|kept| kept.len() == module_names.len() - imports_start as usize);
            let mut file_emit_mode = None;

            for (index, entry) in module_names.iter().copied().enumerate() {
                let (module_name, usage, adds) =
                    match (&kept, index.checked_sub(imports_start as usize)) {
                        (Some(kept), Some(kept_index)) => kept.get(kept_index),
                        _ => (entry.text(), import_usage(entry), None),
                    };
                // ts#63915: a source phase import is not resolved
                // (fileloader.go:884).
                if module_name.is_empty() || is_source_phase_import(entry.parent()) {
                    continue;
                }

                let mode = mode_for_import_usage(
                    usage,
                    file.file_name(),
                    &meta,
                    Some(&options_for_file),
                    &mut file_emit_mode,
                );
                // The worker resolved name `index` with one mode for the
                // whole file (`guess_import_mode`): most names have it.
                let taken = prep
                    .as_ref()
                    .and_then(|prep| prep.answer(index, mode))
                    .map(|answer| {
                        self.note_worker_logs(&answer.package_jsons, &answer.lookups);
                        answer.value.clone()
                    });
                let resolved_module =
                    match taken {
                        Some(resolved_module) => {
                            names_taken += 1;
                            resolved_module
                        }
                        None => {
                            names_own += 1;
                            let (resolved_module, trace, err) = self
                                .resolver()
                                .resolve_module_name(module_name, &file_name, mode, redirect_ref);
                            if let Some(err) = err {
                                self.note_module_resolution_error(err);
                            }
                            resolutions_trace.extend(trace);
                            resolved_module.unwrap_or_else(|| Arc::new(ResolvedModule::default()))
                        }
                    };
                resolutions_in_file.insert(
                    ModeAwareCacheKey {
                        name: module_name.to_string(),
                        mode,
                    },
                    resolved_module.clone(),
                );

                if !resolved_module.is_resolved() {
                    continue;
                }

                let resolved_file_name = &resolved_module.resolved_file_name;
                let is_from_node_modules_search = resolved_module.is_external_library_import;
                // Don't treat redirected files as JS files.
                // tsgo#4712: nor files resolved through a content mapper extension.
                let is_js_file = !resolved_module.resolved_using_extra_extensions
                    && !file_extension_is_one_of(
                        resolved_file_name,
                        SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
                    )
                    && self
                        .project_reference_file_mapper
                        .borrow()
                        .get_redirect_parsed_command_line_for_resolution(&new_has_file_name(
                            resolved_file_name,
                            &self.to_path(resolved_file_name),
                        ))
                        .is_none();
                let is_js_file_from_node_modules = is_from_node_modules_search
                    && is_js_file
                    && resolved_file_name.contains("/node_modules/");

                // add file to program only if:
                // - resolution was successful
                // - noResolve is falsy
                // - module name comes from the list of imports
                // - it's not a top level JavaScript module that exceeded the search max

                let import_index = index as i32 - imports_start;

                let should_add_file = !module_name.is_empty()
                    && get_resolution_diagnostic(&options_for_file, &resolved_module, &file)
                        .is_none()
                    && !options_for_file.no_resolve.is_true()
                    && !(is_js_file && !options_for_file.get_allow_js())
                    && (import_index < 0
                        || ((import_index as usize) < file.imports.len()
                            && adds.unwrap_or_else(|| {
                                import_adds_file(file.imports[import_index as usize])
                            })));

                if should_add_file {
                    t.add_sub_task(
                        ResolvedRef {
                            file_name: resolved_file_name.clone(),
                            increase_depth: resolved_module.is_external_library_import,
                            elide_on_depth: is_js_file_from_node_modules,
                            include_reason: Some(new_file_include_reason(
                                FileIncludeKind::IMPORT,
                                FileIncludeData::ReferencedFile(ReferencedFileData {
                                    file: t.path.clone(),
                                    index: import_index,
                                    synthetic: if import_index < 0 { entry } else { Node::NIL },
                                }),
                            )),
                            package_id: resolved_module.package_id.clone(),
                        },
                        None,
                    );
                }
            }

            if prep.is_some() {
                note_prep_taken();
                count_prep(|c| {
                    c.preps_taken += 1;
                    c.names_taken += names_taken;
                    c.names_own += names_own;
                });
            }
            t.resolutions_in_file = resolutions_in_file;
            t.resolutions_trace = resolutions_trace;
        }
    }

    /// Notes the lookup logs of a worker answer that the load took from a
    /// prep, once per log (`noted_worker_logs`): the package.json lookups
    /// for the build info, and the `tsc -b` stat lookups for the build
    /// host's cache, as the resolver notes a shared answer that it reads.
    // PORT: not in Go (loadpar1).
    pub(crate) fn note_worker_logs(
        &self,
        package_jsons: &Arc<[PackageJsonLookup]>,
        lookups: &Option<Arc<[StatLookup]>>,
    ) {
        let Some(resolver) = self
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
        else {
            return;
        };
        let mut noted = self.noted_worker_logs.borrow_mut();
        if !package_jsons.is_empty()
            && noted.insert(Arc::as_ptr(package_jsons) as *const () as usize)
        {
            resolver.caches.note_worker_package_jsons(package_jsons);
        }
        if let Some(lookups) = lookups.as_ref().filter(|lookups| !lookups.is_empty())
            && noted.insert(Arc::as_ptr(lookups) as *const () as usize)
        {
            resolver.caches.note_worker_lookups(&Some(lookups.clone()));
        }
    }

    /// Keeps the package.json reads of the package scope walk of a file's
    /// metadata that a parse worker made in the program resolver's cache
    /// (`Caches::adopt_worker_package_jsons`), when the load takes the
    /// metadata (`take_prefetched_meta`). The cache parses a read only when
    /// a lookup asks for it.
    // PORT: not in Go (loadpar1). Go finds the metadata with the program's
    // resolver (fileloader.go:391).
    pub(crate) fn adopt_worker_package_jsons(&self, package_jsons: &[PackageJsonLookup]) {
        if let Some(resolver) = self
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
        {
            resolver.caches.adopt_worker_package_jsons(package_jsons);
        }
    }

    // Go: fileloader.go:943 (*fileLoader).createSyntheticImport
    pub fn create_synthetic_import(&self, text: &str, file: &ParsedSourceFile) -> Node {
        let external_helpers_module_reference =
            self.factory.new_string_literal(text, TokenFlags::NONE);
        let import_decl = self.factory.new_import_declaration(
            ModifierList::NIL,
            Node::NIL,
            external_helpers_module_reference,
            Node::NIL,
        );
        set_node_parent(external_helpers_module_reference, import_decl);
        set_node_parent(import_decl, file.root);
        external_helpers_module_reference
    }

    // Go: fileloader.go:953 (*fileLoader).pathForLibFile
    pub fn path_for_lib_file(&self, name: &str) -> Rc<LibFile> {
        if let Some(cached) = self.path_for_lib_file_cache.borrow().get(name) {
            return cached.clone();
        }

        let mut path = combine_paths(&self.default_library_path, &[name]);
        let mut replaced = false;
        if !self.opts.skip_module_resolution
            && self
                .opts
                .config
                .compiler_options()
                .lib_replacement
                .is_true()
            && name != "lib.d.ts"
        {
            let library_name = get_library_name_from_lib_file_name(name);
            let resolve_from = get_inferred_library_name_resolve_from(
                self.opts.config.compiler_options(),
                &self.host.get_current_directory(),
                name,
            );
            let (resolution, trace) = self.resolve_library(&library_name, &resolve_from);
            if resolution.is_resolved() {
                path = resolution.resolved_file_name.clone();
                replaced = true;
            }
            self.path_for_lib_file_resolutions
                .borrow_mut()
                .entry(self.to_path(&resolve_from))
                .or_insert_with(|| {
                    Rc::new(LibResolution {
                        library_name,
                        resolution,
                        trace,
                    })
                });
        }

        self.path_for_lib_file_cache
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| {
                Rc::new(LibFile {
                    name: name.to_string(),
                    path,
                    replaced,
                })
            })
            .clone()
    }

    // Go: fileloader.go:981 (*fileLoader).resolveLibrary
    pub fn resolve_library(
        &self,
        library_name: &str,
        resolve_from: &str,
    ) -> (Arc<ResolvedModule>, Vec<DiagAndArgs>) {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Program,
                "resolveLibrary",
                vec![("resolveFrom", resolve_from.to_string().into())],
                false,
            )
        });
        let (resolved, trace, err) = self.resolver().resolve_module_name(
            library_name,
            resolve_from,
            ModuleKind::COMMON_JS,
            None,
        );
        if let Some(err) = err {
            self.note_module_resolution_error(err);
        }
        // PORT: Go returns a nil result from a `create_module_resolver`
        // resolver as is; its callers only ask `IsResolved`, which is false
        // for nil, as for an empty result.
        (
            resolved.unwrap_or_else(|| Arc::new(ResolvedModule::default())),
            trace,
        )
    }

    /// Go `p.moduleResolutionErrorOnce.Do(func() { p.moduleResolutionError = err })`
    /// (ts#64299): keeps the first error.
    fn note_module_resolution_error(&self, err: GoError) {
        let mut module_resolution_error = self.module_resolution_error.borrow_mut();
        if module_resolution_error.is_none() {
            *module_resolution_error = Some(err);
        }
    }

    /// Go `p.resolver`. It is set before any file is loaded.
    fn resolver(&self) -> &dyn Resolver {
        self.resolver
            .as_deref()
            .expect("fileLoader.resolver is set before loading")
    }
}

/// Whether Go `loadSourceFileMetaData` takes the package.json `type` of the
/// scope of `file_name` (fileloader.go:398 to :401): for node16 to nodenext
/// module resolution (not for `.mts`, `.cts`, `.mjs` and `.cjs` files), or
/// for a path under `/node_modules/`.
pub(crate) fn package_json_type_applies(
    file_name: &str,
    module_resolution_kind: ModuleResolutionKind,
) -> bool {
    !file_extension_is_one_of(
        file_name,
        &[EXTENSION_MTS, EXTENSION_CTS, EXTENSION_MJS, EXTENSION_CJS],
    ) && ModuleResolutionKind::NODE16 <= module_resolution_kind
        && module_resolution_kind <= ModuleResolutionKind::NODE_NEXT
        || file_name.contains("/node_modules/")
}

/// The body of Go `(*fileLoader).loadSourceFileMetaData` with the loader's
/// resolver and options as parameters, so a parse worker can run it with
/// its own resolver (`files_parser.rs`).
// Go: fileloader.go:382 (*fileLoader).loadSourceFileMetaData
// PORT: the scope is the parts of it that this function reads
// (`PackageScope`). In a resolve-ahead load, the loader takes the scope that
// the workers found, or the scope that it found for the directory before
// (`Caches::package_scope_ahead`).
pub(crate) fn source_file_meta_data(
    resolver: &dyn Resolver,
    options: &CompilerOptions,
    file_name: &str,
) -> SourceFileMetaData {
    let directory = get_directory_path(file_name);
    let find = || PackageScope::of(resolver.get_package_scope_for_path(&directory).as_deref());
    let package_json_scope = resolver
        .as_default_resolver()
        .and_then(|default| default.caches.package_scope_ahead(&directory, find))
        .unwrap_or_else(find);
    meta_of_package_scope(package_json_scope, options, file_name)
}

/// The metadata of `file_name` in the package scope `package_json_scope`
/// (`source_file_meta_data` after its scope lookup). A parse worker finds
/// the scope of a directory once for all its files (`FilePrep`).
// Go: fileloader.go:382 (*fileLoader).loadSourceFileMetaData
pub(crate) fn meta_of_package_scope(
    package_json_scope: Option<PackageScope>,
    options: &CompilerOptions,
    file_name: &str,
) -> SourceFileMetaData {
    let module_resolution_kind = options.get_module_resolution_kind();

    let mut package_json_type = String::new();
    let mut package_json_directory = String::new();
    if let Some(scope) = package_json_scope {
        package_json_directory = scope.package_directory;
        if let Some(value) = scope.type_
            && package_json_type_applies(file_name, module_resolution_kind)
        {
            package_json_type = value;
        }
    }

    let implied_node_format = get_implied_node_format_for_file(file_name, &package_json_type);
    SourceFileMetaData {
        package_json_type,
        package_json_directory,
        implied_node_format,
    }
}

/// True when parse workers resolve the imports of the files they parse
/// with `options`: the loader adds resolved files (no `noResolve`), and Go
/// `ResolveModuleName` supports the module resolution kind (the others
/// panic, on the loader too).
pub(crate) fn workers_resolve_imports(options: &CompilerOptions) -> bool {
    let kind = options.get_module_resolution_kind();
    !options.no_resolve.is_true()
        && (kind == ModuleResolutionKind::NODE16
            || kind == ModuleResolutionKind::NODE_NEXT
            || kind == ModuleResolutionKind::BUNDLER)
}

// Go: fileloader.go:994 getLibraryNameFromLibFileName
pub fn get_library_name_from_lib_file_name(lib_file_name: &str) -> String {
    // Support resolving to lib.dom.d.ts -> @typescript/lib-dom, and
    //                      lib.dom.iterable.d.ts -> @typescript/lib-dom/iterable
    //                      lib.es2015.symbol.wellknown.d.ts -> @typescript/lib-es2015/symbol-wellknown
    let components: Vec<&str> = lib_file_name.split('.').collect();
    let mut path = String::from("@typescript/lib-");
    if components.len() > 1 {
        path.push_str(components[1]);
    }
    let mut i = 2;
    while i < components.len() && !components[i].is_empty() && components[i] != "d" {
        if i == 2 {
            path.push('/');
        } else {
            path.push('-');
        }
        path.push_str(components[i]);
        i += 1;
    }
    path
}

// Go: fileloader.go:1017 getInferredLibraryNameResolveFrom
pub fn get_inferred_library_name_resolve_from(
    options: &CompilerOptions,
    current_directory: &str,
    lib_file_name: &str,
) -> String {
    let containing_directory = if !options.config_file_path.is_empty() {
        get_directory_path(&options.config_file_path)
    } else {
        current_directory.to_string()
    };
    combine_paths(
        &containing_directory,
        &[&format!("__lib_node_modules_lookup_{lib_file_name}__.ts")],
    )
}

// Go: fileloader.go:1021 getModeForTypeReferenceDirectiveInFile
pub fn get_mode_for_type_reference_directive_in_file(
    ref_: &FileReference,
    file: &ParsedSourceFile,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    if ref_.resolution_mode != RESOLUTION_MODE_NONE {
        ref_.resolution_mode
    } else {
        get_default_resolution_mode_for_file(file.file_name(), meta, options)
    }
}

// Go: fileloader.go:1029 getDefaultResolutionModeForFile
// PORT: private, because program.rs has a public
// `get_default_resolution_mode_for_file` (Go program.go) with another shape.
pub(crate) fn get_default_resolution_mode_for_file(
    file_name: &str,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    if import_syntax_affects_module_resolution(options) {
        get_implied_node_format_for_emit_worker(file_name, options.get_emit_module_kind(), meta)
    } else {
        RESOLUTION_MODE_NONE
    }
}

// Go: fileloader.go:1037 getModeForUsageLocation
// PORT: private, because program.rs has a public `get_mode_for_usage_location`
// (Go program.go) with another shape. Go `options` can be nil (`None`). The
// node reads are `import_usage`, and the rest is `mode_for_import_usage`,
// so the loader can keep the node reads of a parse (`import_names`).
pub(crate) fn get_mode_for_usage_location(
    file_name: &str,
    meta: &SourceFileMetaData,
    usage: Node,
    options: Option<&CompilerOptions>,
) -> ResolutionMode {
    mode_for_import_usage(import_usage(usage), file_name, meta, options, &mut None)
}

/// What Go `getModeForUsageLocation` reads of a module name node: a
/// resolution-mode override, or the kind of usage that
/// `getEmitSyntaxForUsageLocationWorker` tells apart (`import_usage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportUsage {
    /// The `resolution-mode` of a type-only import or export, or of an
    /// import type.
    Override(ResolutionMode),
    /// `require(...)`, or `import x = require(...)`.
    Require,
    /// `import(...)`.
    ImportCall,
    /// `import.source(...)` (ts#63915).
    SourcePhaseImportCall,
    /// Any other module name.
    Other,
}

/// The node reads of Go `getModeForUsageLocation` for the module name
/// `usage`, in their order, and those of
/// `getEmitSyntaxForUsageLocationWorker` (`emit_usage`).
// Go: fileloader.go:1037 getModeForUsageLocation
fn import_usage(usage: Node) -> ImportUsage {
    let parent = usage.parent();
    if is_import_declaration(parent)
        || parent.kind() == SyntaxKind::JsImportDeclaration
        || is_export_declaration(parent)
        || is_js_doc_import_tag(parent)
    {
        let is_type_only = is_exclusively_type_only_import_or_export(parent);
        if is_type_only {
            // PORT: the Go switch on the parent kind reads `Attributes` of
            // the import declaration, export declaration or JSDoc import tag.
            // `Node::attributes` reads the field of each of these kinds.
            let (override_, ok) = match parent.kind() {
                SyntaxKind::ImportDeclaration
                | SyntaxKind::JsImportDeclaration
                | SyntaxKind::ExportDeclaration
                | SyntaxKind::JsDocImportTag => {
                    parent.attributes().get_resolution_mode_override(None)
                }
                _ => (RESOLUTION_MODE_NONE, false),
            };
            if ok {
                return ImportUsage::Override(override_);
            }
        }
    }
    if is_literal_type_node(parent) && is_import_type_node(parent.parent()) {
        let (override_, ok) = parent
            .parent()
            .attributes()
            .get_resolution_mode_override(None);
        if ok {
            return ImportUsage::Override(override_);
        }
    }
    emit_usage(parent)
}

/// The node reads of Go `getEmitSyntaxForUsageLocationWorker` for a module
/// name whose parent is `parent`.
// Go: fileloader.go:1075 getEmitSyntaxForUsageLocationWorker
fn emit_usage(parent: Node) -> ImportUsage {
    if is_require_call(parent, false /*requireStringLiteralLikeArgument*/)
        || is_external_module_reference(parent) && is_import_equals_declaration(parent.parent())
    {
        return ImportUsage::Require;
    }
    let call = walk_up_parenthesized_expressions(parent);
    if is_import_call(call) {
        // ts#63915
        if is_source_phase_import_call(call) {
            return ImportUsage::SourcePhaseImportCall;
        }
        return ImportUsage::ImportCall;
    }
    ImportUsage::Other
}

/// Go `getModeForUsageLocation` for a module name of `usage` in
/// `file_name`. `file_emit_mode` keeps the file's emit format for the other
/// names of the file (`emit_syntax_for_usage`).
// Go: fileloader.go:1037 getModeForUsageLocation
fn mode_for_import_usage(
    usage: ImportUsage,
    file_name: &str,
    meta: &SourceFileMetaData,
    options: Option<&CompilerOptions>,
    file_emit_mode: &mut Option<ModuleKind>,
) -> ResolutionMode {
    if let ImportUsage::Override(mode) = usage {
        return mode;
    }
    if let Some(options) = options {
        if import_syntax_affects_module_resolution(options) {
            return emit_syntax_for_usage(usage, file_name, meta, options, file_emit_mode);
        }
    }

    RESOLUTION_MODE_NONE
}
/// Whether an import adds its file to the program, as far as the node
/// tells (Go `resolveImportsAndModuleAugmentations`: a JSDoc import of a TS
/// file adds no file).
// Go: fileloader.go (*fileLoader).resolveImportsAndModuleAugmentations, the
// importIndex condition (pin 673a5f17d713: fileloader.go:926)
fn import_adds_file(import: Node) -> bool {
    is_in_js_file(import) || !import.flags().intersects(NodeFlags::JS_DOC)
}

/// The module names of the imports and string module augmentations of a
/// parse, in the order of `resolveImportsAndModuleAugmentations` after its
/// synthetic imports, with each name's `ImportUsage` and, for an import,
/// `import_adds_file`.
// PORT: not in Go (perf, `import_names`).
struct ImportNames {
    /// The names, one after another.
    text: String,
    /// Per name: its end in `text`, its usage, and whether it adds its file
    /// (false for a module augmentation, which adds none).
    names: Vec<(usize, ImportUsage, bool)>,
}

impl ImportNames {
    fn of(file: &ParsedSourceFile) -> ImportNames {
        let augmentations = file
            .module_augmentations
            .iter()
            .filter(|augmentation| augmentation.kind() == SyntaxKind::StringLiteral);
        let count = file.imports.len() + augmentations.clone().count();
        let mut names = ImportNames {
            text: String::new(),
            names: Vec::with_capacity(count),
        };
        for &import in &file.imports {
            names.push(import, import_adds_file(import));
        }
        for &augmentation in augmentations {
            names.push(augmentation, false);
        }
        names
    }

    fn push(&mut self, name: Node, adds: bool) {
        self.text.push_str(name.text());
        self.names.push((self.text.len(), import_usage(name), adds));
    }

    fn len(&self) -> usize {
        self.names.len()
    }

    /// The name, usage and `import_adds_file` of name `index`.
    fn get(&self, index: usize) -> (&str, ImportUsage, Option<bool>) {
        let (end, usage, adds) = self.names[index];
        let start = index
            .checked_sub(1)
            .map_or(0, |before| self.names[before].0);
        (&self.text[start..end], usage, Some(adds))
    }
}

thread_local! {
    /// The `ImportNames` of the parses that the language server loads on
    /// this thread read, by the address of the parse. The weak link keeps
    /// the address from a new parse while the entry is here.
    static IMPORT_NAMES: RefCell<FxHashMap<usize, (std::rc::Weak<ParsedSourceFile>, Rc<ImportNames>)>> =
        RefCell::default();
}

/// The `ImportNames` of `file`, kept with the parse for the next loads. A
/// language server load reads the parses of the unchanged files of the
/// last load again, and `Node::text` of their module names is slow there
/// (lspedit1: 0.72 ms per effect load).
// PORT: not in Go (perf). Go reads `entry.Text()` and the usage nodes in
// each load.
fn import_names(file: &Rc<ParsedSourceFile>) -> Rc<ImportNames> {
    let key = Rc::as_ptr(file) as usize;
    IMPORT_NAMES.with(|kept| {
        if let Some((_, names)) = kept.borrow().get(&key) {
            return names.clone();
        }
        let names = Rc::new(ImportNames::of(file));
        kept.borrow_mut()
            .insert(key, (Rc::downgrade(file), names.clone()));
        names
    })
}

/// Drops the `ImportNames` of the parses that no one holds now (at the
/// start of each language server load).
fn drop_dead_import_names() {
    IMPORT_NAMES.with(|kept| {
        kept.borrow_mut()
            .retain(|_, (file, _)| file.strong_count() > 0);
    });
}

// Go: fileloader.go:1069 importSyntaxAffectsModuleResolution
fn import_syntax_affects_module_resolution(options: &CompilerOptions) -> bool {
    let module_resolution = options.get_module_resolution_kind();
    ModuleResolutionKind::NODE16 <= module_resolution
        && module_resolution <= ModuleResolutionKind::NODE_NEXT
        || options.get_resolve_package_json_exports()
        || options.get_resolve_package_json_imports()
}

// Go: fileloader.go:1075 getEmitSyntaxForUsageLocationWorker
pub(crate) fn get_emit_syntax_for_usage_location_worker(
    file_name: &str,
    meta: &SourceFileMetaData,
    usage: Node,
    options: &CompilerOptions,
) -> ResolutionMode {
    emit_syntax_for_usage(
        emit_usage(usage.parent()),
        file_name,
        meta,
        options,
        &mut None,
    )
}

/// Go `getEmitSyntaxForUsageLocationWorker` for a module name of `usage`
/// (`emit_usage`). `file_emit_mode` keeps Go `GetEmitModuleFormatOfFileWorker`
/// of the file, which reads only the file name, the options and the
/// metadata, for the other names of the file.
// Go: fileloader.go:1075 getEmitSyntaxForUsageLocationWorker
fn emit_syntax_for_usage(
    usage: ImportUsage,
    file_name: &str,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
    file_emit_mode: &mut Option<ModuleKind>,
) -> ResolutionMode {
    if usage == ImportUsage::Require {
        return ModuleKind::COMMON_JS;
    }
    let file_emit_mode = *file_emit_mode
        .get_or_insert_with(|| get_emit_module_format_of_file_worker(file_name, options, meta));
    // ts#63915: `import.source(...)` is ESNext syntax (fileloader.go:1092).
    if usage == ImportUsage::SourcePhaseImportCall {
        return ModuleKind::ES_NEXT;
    }
    if usage == ImportUsage::ImportCall {
        return if should_transform_import_call(file_name, options, file_emit_mode) {
            ModuleKind::COMMON_JS
        } else {
            ModuleKind::ES_NEXT
        };
    }
    // If we're in --module preserve on an input file, we know that an import
    // is an import. But if this is a declaration file, we'd prefer to use the
    // impliedNodeFormat. Since we want things to be consistent between the two,
    // we need to issue errors when the user writes ESM syntax in a definitely-CJS
    // file, until/unless declaration emit can indicate a true ESM import. On the
    // other hand, writing CJS syntax in a definitely-ESM file is fine, since declaration
    // emit preserves the CJS syntax.
    if file_emit_mode == ModuleKind::COMMON_JS {
        return ModuleKind::COMMON_JS;
    } else if file_emit_mode.is_non_node_esm() || file_emit_mode == ModuleKind::PRESERVE {
        return ModuleKind::ES_NEXT;
    }
    ModuleKind::NONE
}

/// Go `getModeForUsageLocation` (with `options`) for a plain `import ..
/// from` or `export .. from` in `file_name`, the most common form. A parse
/// worker has no import nodes after the parse, so it resolves each import
/// with this mode. A wrong guess only resolves another cache key: the
/// loader resolves each import with its real mode.
pub(crate) fn guess_import_mode(
    file_name: &str,
    meta: &SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    if !import_syntax_affects_module_resolution(options) {
        return RESOLUTION_MODE_NONE;
    }
    // Go: fileloader.go:1075 getEmitSyntaxForUsageLocationWorker, for a
    // usage that is not a require or an import call.
    let file_emit_mode = get_emit_module_format_of_file_worker(file_name, options, meta);
    if file_emit_mode == ModuleKind::COMMON_JS {
        return ModuleKind::COMMON_JS;
    } else if file_emit_mode.is_non_node_esm() || file_emit_mode == ModuleKind::PRESERVE {
        return ModuleKind::ES_NEXT;
    }
    ModuleKind::NONE
}

/// The synthetic imports of a file (Go `resolveImportsAndModuleAugmentations`,
/// before its imports): `tslib` for `importHelpers`, then the JSX runtime
/// import. The parse worker of the file resolves them too (`FilePrep`).
// Go: fileloader.go:835 to :853 (resolveImportsAndModuleAugmentations)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyntheticImports {
    /// The file gets the `tslib` import (`EXTERNAL_HELPERS_MODULE_NAME_TEXT`).
    pub helpers: bool,
    /// The JSX runtime import, or empty.
    pub jsx: String,
}

impl SyntheticImports {
    /// The synthetic imports of `file` with the options of the file
    /// (with its project reference redirect).
    pub(crate) fn of(file: &ParsedSourceFile, options_for_file: &CompilerOptions) -> Self {
        // PORT: Go `ast.IsSourceFileJS(file)` and `ast.IsExternalModule(file)`
        // read the `ast.SourceFile` fields. The crate versions read
        // `source_file_info`, which does not exist during load, so the
        // fields of `ParsedSourceFile` are read here.
        let is_java_script_file = file.is_js();
        let is_external_module_file = file.external_module_indicator.is_some();
        let helpers = (is_java_script_file
            || (!file.is_declaration_file
                && (options_for_file.get_isolated_modules() || is_external_module_file)))
            && options_for_file.import_helpers.is_true();
        let jsx = if is_java_script_file || file.script_kind == ScriptKind::TSX {
            get_jsx_runtime_import(
                &get_jsx_implicit_import_base_of_file(options_for_file, file),
                options_for_file,
            )
        } else {
            String::new()
        };
        SyntheticImports { helpers, jsx }
    }

    /// The module names, in their order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.helpers
            .then_some(EXTERNAL_HELPERS_MODULE_NAME_TEXT)
            .into_iter()
            .chain((!self.jsx.is_empty()).then_some(self.jsx.as_str()))
    }
}

/// Go `ast.GetJSXImplicitImportBase(options, file)` for a file that is still
/// loading.
// PORT: the crate `get_jsx_implicit_import_base` reads the pragmas through
// `source_file_info`, which does not exist during load. This is the same Go
// logic (ast/utilities.go GetJSXImplicitImportBase and
// GetPragmaFromSourceFile) on the `ParsedSourceFile` pragmas.
pub(crate) fn get_jsx_implicit_import_base_of_file(
    compiler_options: &CompilerOptions,
    file: &ParsedSourceFile,
) -> String {
    // Go: GetPragmaFromSourceFile, the last one wins.
    let pragma = |name: &str| file.pragmas.iter().rev().find(|pragma| pragma.name == name);
    let jsx_import_source_pragma = pragma("jsximportsource");
    let jsx_runtime_pragma = pragma("jsxruntime");
    if get_pragma_argument(jsx_runtime_pragma, "factory") == "classic" {
        return String::new();
    }
    if compiler_options.jsx == JsxEmit::REACT_JSX
        || compiler_options.jsx == JsxEmit::REACT_JSX_DEV
        || !compiler_options.jsx_import_source.is_empty()
        || jsx_import_source_pragma.is_some()
        || get_pragma_argument(jsx_runtime_pragma, "factory") == "automatic"
    {
        let mut result = get_pragma_argument(jsx_import_source_pragma, "factory");
        if result.is_empty() {
            result = compiler_options.jsx_import_source.clone();
        }
        if result.is_empty() {
            result = "react".to_string();
        }
        return result;
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::super::files_parser::{preps_taken, set_load_prep, set_meta_wait};
    use super::*;
    use crate::frontend::bundled;
    use crate::frontend::module::cache::set_answer_wait;
    use crate::frontend::tsoptions::{ParseConfigHost, get_parsed_command_line_of_config_file};
    use crate::frontend::vfs::osvfs_fs;

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

    /// Writes `files` (path, text) and a tsconfig.json with `tsconfig` to a
    /// temp dir, loads the program and returns its file names in program
    /// order: paths relative to the dir, or the base name for a lib file.
    fn program_file_names(label: &str, tsconfig: &str, files: &[(&str, &str)]) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
        let cwd = dir.to_string_lossy().replace('\\', "/");
        let fs = bundled::wrap_fs(osvfs_fs());
        let sys = System {
            fs: fs.clone(),
            current_directory: cwd.clone(),
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &format!("{cwd}/tsconfig.json"),
            None,
            None,
            &sys,
            None,
        );
        assert!(errors.is_empty());
        let host = new_cached_fs_compiler_host(&cwd, fs, &bundled::lib_path(), None, None, None);
        let processed = process_all_program_files_of(
            ProgramOptions {
                host,
                config: Rc::new(config.unwrap()),
                use_source_of_project_reference: false,
                single_threaded: Tristate::True,
                typings_location: String::new(),
                project_name: String::new(),
                create_module_resolver: None,
                skip_module_resolution: false,
            },
            true,
        );
        let _ = std::fs::remove_dir_all(&dir);
        let prefix = format!("{cwd}/");
        processed
            .files
            .iter()
            .map(|file| match file.file_name().strip_prefix(&prefix) {
                Some(relative) => relative.to_string(),
                None => get_base_file_name(file.file_name()),
            })
            .collect()
    }

    /// Writes `files` and a tsconfig.json with `tsconfig` to a temp dir,
    /// loads the program with parse workers (not single threaded), and
    /// returns what the load found (`load_text`).
    fn parallel_load(label: &str, tsconfig: &str, files: &[(&str, &str)]) -> String {
        load_text(label, tsconfig, files, false)
    }

    /// Writes `files` and a tsconfig.json with `tsconfig` to a temp dir,
    /// loads the program, and returns what the load found, as text: the
    /// files in program order, the metadata, module and type reference
    /// resolutions (in map order) and include reasons of each file, the
    /// synthetic imports, the processing diagnostics, the missing files
    /// and the package.json entries of the resolver.
    fn load_text(
        label: &str,
        tsconfig: &str,
        files: &[(&str, &str)],
        single_threaded: bool,
    ) -> String {
        use std::fmt::Write as _;
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
        let cwd = dir.to_string_lossy().replace('\\', "/");
        // Taken while the dir exists: the real path of a removed dir is the
        // path itself.
        let real = osvfs_fs().realpath(&cwd);
        let fs = bundled::wrap_fs(osvfs_fs());
        let sys = System {
            fs: fs.clone(),
            current_directory: cwd.clone(),
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &format!("{cwd}/tsconfig.json"),
            None,
            None,
            &sys,
            None,
        );
        assert!(errors.is_empty());
        let host = new_cached_fs_compiler_host(&cwd, fs, &bundled::lib_path(), None, None, None);
        let processed = process_all_program_files_of(
            ProgramOptions {
                host,
                config: Rc::new(config.unwrap()),
                use_source_of_project_reference: false,
                single_threaded: if single_threaded {
                    Tristate::True
                } else {
                    Tristate::False
                },
                typings_location: String::new(),
                project_name: String::new(),
                create_module_resolver: None,
                skip_module_resolution: false,
            },
            single_threaded,
        );
        let _ = std::fs::remove_dir_all(&dir);
        let mut out = String::new();
        // The include reasons name files by path (Go `tspath.Path`), which
        // is in lower case on a case-insensitive file system (macOS).
        let case_sensitive = osvfs_fs().use_case_sensitive_file_names();
        let cwd_path = to_path(&cwd, "", case_sensitive).0;
        // A node_modules file is named by its real path (Go
        // `resolutionState.realPath`, module/resolver.go:1859). On macOS the
        // temp dir is under /var, a symlink to /private/var, and the real
        // path contains the plain one, so it is replaced first.
        let real_path = to_path(&real, "", case_sensitive).0;
        let name = |file_name: &str| {
            file_name
                .replace(&real, "<dir>")
                .replace(&real_path, "<dir>")
                .replace(&cwd, "<dir>")
                .replace(&cwd_path, "<dir>")
        };
        for file in &processed.files {
            let path = file.path();
            writeln!(out, "file {}", name(file.file_name())).unwrap();
            let meta = format!("{:?}", processed.source_file_meta_datas.get(path));
            writeln!(out, "  meta {}", name(&meta)).unwrap();
            for (key, resolved) in processed.resolved_modules.get(path).into_iter().flatten() {
                writeln!(
                    out,
                    "  module {:?} {:?} -> {} {:?} external {} package {}",
                    key.name,
                    key.mode,
                    name(&resolved.resolved_file_name),
                    resolved.extension,
                    resolved.is_external_library_import,
                    resolved.package_id.string(),
                )
                .unwrap();
            }
            for (key, resolved) in processed
                .type_resolutions_in_file
                .get(path)
                .into_iter()
                .flatten()
            {
                writeln!(
                    out,
                    "  type {:?} {:?} -> {}",
                    key.name,
                    key.mode,
                    name(&resolved.resolved_file_name)
                )
                .unwrap();
            }
            for reason in processed
                .include_processor
                .file_include_reasons
                .get(path)
                .into_iter()
                .flatten()
            {
                let data = match &reason.data {
                    FileIncludeData::ReferencedFile(data) => format!(
                        "{} #{} synthetic {}",
                        name(&data.file.0),
                        data.index,
                        data.synthetic.is_some()
                    ),
                    other => format!("{other:?}"),
                };
                writeln!(out, "  reason {:?} {data}", reason.kind).unwrap();
            }
            let jsx = processed
                .jsx_runtime_import_specifiers
                .as_ref()
                .and_then(|map| map.get(path))
                .map(|jsx| jsx.module_reference.clone());
            let helpers = processed
                .import_helpers_import_specifiers
                .as_ref()
                .is_some_and(|map| map.contains_key(path));
            writeln!(out, "  jsx {jsx:?} helpers {helpers}").unwrap();
        }
        for diagnostic in &processed.include_processor.processing_diagnostics {
            writeln!(out, "processing diagnostic {:?}", diagnostic.kind.0).unwrap();
        }
        writeln!(out, "missing {:?}", processed.missing_files).unwrap();
        let mut package_jsons = Vec::new();
        if let Some(resolver) = processed
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
        {
            resolver.package_json_cache_entries(|_, entry| {
                package_jsons.push(format!(
                    "{} {} {}",
                    name(entry.package_directory),
                    entry.directory_exists,
                    entry.exists
                ));
                true
            });
        }
        package_jsons.sort();
        package_jsons.dedup();
        writeln!(out, "package.json {package_jsons:?}").unwrap();
        out
    }

    // loadpar1: the parse workers resolve each file's synthetic imports and
    // imports for the loader (`FilePrep`). A load that takes their answers
    // finds what a load that resolves every name itself finds, in the same
    // order: node16 modes from package.json `type`, `.cts` and `.mts`,
    // `require`, `import()` (another mode than the worker's), type-only
    // imports with `resolution-mode`, JSDoc imports in TS and JS files,
    // `importHelpers`, the JSX runtime import, `/// <reference types>` and
    // a module augmentation.
    #[test]
    fn worker_prep_gives_the_same_load() {
        let tsconfig = r#"{ "compilerOptions": { "module": "node16", "allowJs": true,
             "checkJs": true, "importHelpers": true, "jsx": "react-jsx", "types": [],
             "noEmit": true }, "include": ["src"] }"#;
        let files = [
            ("package.json", r#"{ "name": "app", "type": "commonjs" }"#),
            ("src/esm/package.json", r#"{ "type": "module" }"#),
            (
                "src/esm/a.ts",
                r#"import { b } from "../b.js";
import type { T } from "lib" with { "resolution-mode": "require" };
import { e } from "lib";
export const a = b + e;
export type U = T;
export const later = () => import("../c.cjs");
"#,
            ),
            (
                "src/b.ts",
                r#"/// <reference types="node-ish" />
import x = require("./c.cjs");
import { e } from "lib";
export const b: number = x.c + e;
export const lazy = () => import("lib");
declare module "lib" { export const extra: number; }
"#,
            ),
            ("src/c.cts", "export const c = 1;\n"),
            (
                "src/d.mts",
                "import { e } from \"lib\";\nexport const d = e;\n",
            ),
            (
                "src/j.js",
                r#"/** @import { T } from "lib" */
const { c } = require("./c.cjs");
/** @type {T} */
export const j = c;
"#,
            ),
            (
                "src/k.ts",
                "/** @import { T } from \"./c.cjs\" */\nexport const k = 1;\n",
            ),
            ("src/v.tsx", "export const v = <div />;\n"),
            (
                "node_modules/lib/package.json",
                r#"{ "name": "lib", "version": "1.0.0",
                   "exports": { ".": { "import": "./esm.d.mts", "require": "./cjs.d.cts" } } }"#,
            ),
            (
                "node_modules/lib/esm.d.mts",
                "export declare const e: number;\nexport type T = string;\n",
            ),
            (
                "node_modules/lib/cjs.d.cts",
                "export declare const e: number;\nexport type T = number;\n",
            ),
            (
                "node_modules/tslib/package.json",
                r#"{ "name": "tslib", "version": "2.0.0", "types": "tslib.d.ts" }"#,
            ),
            (
                "node_modules/tslib/tslib.d.ts",
                "export declare function __assign(t: any): any;\n",
            ),
            (
                "node_modules/react/package.json",
                r#"{ "name": "react", "version": "18.0.0",
                   "exports": { "./jsx-runtime": { "types": "./jsx-runtime.d.ts" } } }"#,
            ),
            (
                "node_modules/react/jsx-runtime.d.ts",
                r#"export declare function jsx(): any;
export declare namespace JSX { interface IntrinsicElements { [name: string]: any } }
"#,
            ),
            (
                "node_modules/@types/node-ish/package.json",
                r#"{ "name": "@types/node-ish", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/@types/node-ish/index.d.ts",
                "declare var nodeIsh: number;\n",
            ),
        ];
        set_load_prep(Some(true));
        let before = preps_taken();
        let with_prep = parallel_load("prep_on", tsconfig, &files);
        let taken = preps_taken() - before;
        set_load_prep(Some(false));
        let without_prep = parallel_load("prep_off", tsconfig, &files);
        set_load_prep(None);
        assert_eq!(with_prep, without_prep);
        assert!(with_prep.contains(r#"module "lib""#), "{with_prep}");
        // With no parse workers (one CPU, or `GOPORT_PARSE_THREADS=0`) the
        // loader resolves every name itself.
        if super::super::files_parser::parse_workers_enabled() {
            assert!(taken > 0, "no prep taken:\n{with_prep}");
        }
    }

    /// A program with a project reference (`lib`) that it loads by the
    /// output `.d.ts` files (Go `getParseFileRedirect`, a program that does
    /// not use the sources of its references). The names in `lib/out` and
    /// in `lib/src/types.d.ts` (a source with no output) resolve with the
    /// options of `lib/tsconfig.json`, from their source files (Go
    /// `getRedirectForResolution`): its `paths` find `@lib/helper`, and its
    /// custom condition finds `dep/lib.d.ts`. `src/a.ts` resolves `dep`
    /// with the root options, to `dep/index.d.ts`.
    const REFERENCE_TSCONFIG: &str = r#"{ "compilerOptions": { "module": "esnext",
         "moduleResolution": "bundler", "types": [], "noEmit": true },
         "include": ["src"], "references": [{ "path": "./lib" }] }"#;
    const REFERENCE_FILES: [(&str, &str); 10] = [
        (
            "lib/tsconfig.json",
            r#"{ "compilerOptions": { "composite": true, "module": "esnext",
                 "moduleResolution": "bundler", "types": [], "rootDir": "src",
                 "outDir": "out", "paths": { "@lib/*": ["./src/*"] },
                 "customConditions": ["lib"] }, "include": ["src"] }"#,
        ),
        (
            "lib/src/index.ts",
            "export { h } from \"@lib/helper\";\nexport const x = 1;\n",
        ),
        ("lib/src/helper.ts", "export const h = 2;\n"),
        (
            "lib/src/types.d.ts",
            "import type { Dep } from \"dep\";\nimport type { h } from \"@lib/helper\";\nexport type T = Dep | typeof h;\n",
        ),
        (
            "lib/out/index.d.ts",
            "export { h } from \"@lib/helper\";\nexport declare const x = 1;\n",
        ),
        (
            "lib/out/helper.d.ts",
            "import type { Dep } from \"dep\";\nexport declare const h: Dep;\n",
        ),
        (
            "node_modules/dep/package.json",
            r#"{ "name": "dep", "version": "1.0.0",
               "exports": { ".": { "lib": "./lib.d.ts", "types": "./index.d.ts" } } }"#,
        ),
        ("node_modules/dep/lib.d.ts", "export type Dep = \"lib\";\n"),
        (
            "node_modules/dep/index.d.ts",
            "export type Dep = \"root\";\n",
        ),
        (
            "src/a.ts",
            r#"import { x, h } from "../lib/src/index";
import type { T } from "../lib/src/types";
import type { Dep } from "dep";
export const a: T | Dep | number = x + (h as never);
"#,
        ),
    ];

    // loadcrit2: with project references, the parse workers resolve each
    // file of a reference with the reference's options, from its source
    // file, as the loader does (Go `getRedirectForResolution`,
    // fileloader.go:842): the output `.d.ts` files and a `.d.ts` source.
    // The loader takes their preps (`FilePrep::fits`), and the load finds
    // what a load that resolves every name itself finds, and what a single
    // threaded load finds.
    #[test]
    fn workers_resolve_with_the_reference_redirect_of_the_loader() {
        let load = |label: &str, prep: bool, single_threaded: bool| {
            set_load_prep(Some(prep));
            set_meta_wait(prep && !single_threaded);
            let text = load_text(label, REFERENCE_TSCONFIG, &REFERENCE_FILES, single_threaded);
            set_load_prep(None);
            set_meta_wait(false);
            text
        };
        let single = load("refs_st", false, true);
        assert_eq!(load("refs_prep_off", false, false), single);
        for name in [
            "lib/out/index.d.ts",
            "lib/out/helper.d.ts",
            "lib/src/types.d.ts",
        ] {
            assert!(single.contains(&format!("file <dir>/{name}")), "{single}");
        }
        for target in ["dep/lib.d.ts", "dep/index.d.ts"] {
            assert!(
                single.contains(&format!("-> <dir>/node_modules/{target} ")),
                "{single}"
            );
        }
        // A worker publishes its prep after the parse that the loader
        // waits for, so the loader may resolve a file itself before it.
        // Each redirected file's prep fits in some load, and no prep is
        // unfit.
        let redirected = [
            "/lib/out/index.d.ts",
            "/lib/out/helper.d.ts",
            "/lib/src/types.d.ts",
        ];
        let mut fitted = [false; 3];
        for run in 0..20 {
            PREP_FITS.with(|fits| fits.borrow_mut().clear());
            assert_eq!(load(&format!("refs_prep_on_{run}"), true, false), single);
            let fits = PREP_FITS.with(|fits| std::mem::take(&mut *fits.borrow_mut()));
            assert!(fits.iter().all(|(_, fit)| *fit), "unfit preps: {fits:?}");
            for (index, name) in redirected.iter().enumerate() {
                fitted[index] |= fits.iter().any(|(file, _)| file.ends_with(name));
            }
            if !super::super::files_parser::parse_workers_enabled() || fitted.iter().all(|f| *f) {
                break;
            }
        }
        if super::super::files_parser::parse_workers_enabled() {
            assert_eq!(fitted, [true; 3], "{redirected:?}");
        }
    }

    /// Loads a one-file project with `compilerOptions` and returns the base
    /// names of the lib files in the program.
    fn lib_file_names(label: &str, compiler_options: &str) -> Vec<String> {
        program_file_names(
            label,
            &format!(r#"{{ "compilerOptions": {compiler_options}, "files": ["a.ts"] }}"#),
            &[("a.ts", "export const a = 1;\n")],
        )
        .into_iter()
        .filter(|name| name.starts_with("lib."))
        .collect()
    }

    /// Program files that are not lib files.
    fn user_file_names(label: &str, tsconfig: &str, files: &[(&str, &str)]) -> Vec<String> {
        program_file_names(label, tsconfig, files)
            .into_iter()
            .filter(|name| !name.starts_with("lib."))
            .collect()
    }

    // Go `compilerOptions.Lib == nil` loads the default lib. An explicit
    // empty list is not nil, so it loads no lib.
    #[test]
    fn empty_lib_list_loads_no_lib() {
        assert_eq!(
            lib_file_names("empty", r#"{ "lib": [] }"#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn absent_lib_loads_default_lib() {
        let names = lib_file_names("absent", r#"{ "target": "es2015" }"#);
        assert!(names.iter().any(|name| name == "lib.es6.d.ts"), "{names:?}");
    }

    // Go fileloader.go:839 adds the implicit jsx-runtime import only to a
    // JS or JSX file (IsSourceFileJS reads the script kind) or a TSX file.
    // A JSON file also has the JAVA_SCRIPT_FILE flag, but gets no import.
    #[test]
    fn json_file_gets_no_jsx_runtime_import() {
        let names = user_file_names(
            "json_jsx",
            r#"{ "compilerOptions": { "jsx": "react-jsx", "jsxImportSource": "preact",
                 "module": "esnext", "moduleResolution": "bundler",
                 "resolveJsonModule": true, "types": [] },
                 "files": ["a.ts"] }"#,
            &[
                (
                    "a.ts",
                    "import d from \"./d.json\";\nexport const x = d.a;\n",
                ),
                ("d.json", "{ \"a\": 1 }\n"),
                (
                    "node_modules/preact/package.json",
                    "{ \"name\": \"preact\" }\n",
                ),
                ("node_modules/preact/jsx-runtime/index.d.ts", "export {};\n"),
            ],
        );
        assert_eq!(names, ["d.json", "a.ts"]);
    }

    // Go fileloader.go:852 adds the importHelpers tslib import to a JS file
    // or a non-declaration module, never to a JSON file.
    #[test]
    fn json_file_gets_no_tslib_import() {
        let names = user_file_names(
            "json_tslib",
            r#"{ "compilerOptions": { "importHelpers": true, "module": "esnext",
                 "moduleResolution": "bundler", "resolveJsonModule": true, "types": [] },
                 "files": ["a.ts", "d.json"] }"#,
            &[
                ("a.ts", "const x = 1;\n"),
                ("d.json", "{ \"a\": 1 }\n"),
                (
                    "node_modules/tslib/package.json",
                    "{ \"name\": \"tslib\", \"types\": \"tslib.d.ts\" }\n",
                ),
                ("node_modules/tslib/tslib.d.ts", "export {};\n"),
            ],
        );
        assert_eq!(names, ["a.ts", "d.json"]);
    }

    /// Writes `files` and a tsconfig.json with `tsconfig` to a temp dir and
    /// loads the program with parse workers (not single threaded). Returns
    /// the load, the dir and its name; the caller removes the dir.
    fn load_with_workers(
        label: &str,
        tsconfig: &str,
        files: &[(&str, &str)],
    ) -> (ProcessedFiles, std::path::PathBuf, String) {
        load_with_workers_on(label, tsconfig, files, bundled::wrap_fs(osvfs_fs()))
    }

    /// `load_with_workers` with `fs` as the loader's file system: the OS
    /// file system, maybe wrapped (`wrapvfs`). The parse workers read the
    /// OS file system (`run_prefetch_worker`).
    fn load_with_workers_on(
        label: &str,
        tsconfig: &str,
        files: &[(&str, &str)],
        fs: Rc<dyn Fs>,
    ) -> (ProcessedFiles, std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
        let cwd = dir.to_string_lossy().replace('\\', "/");
        let sys = System {
            fs: bundled::wrap_fs(osvfs_fs()),
            current_directory: cwd.clone(),
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &format!("{cwd}/tsconfig.json"),
            None,
            None,
            &sys,
            None,
        );
        assert!(errors.is_empty());
        // The parse workers run when the host shows the plain OS file
        // system (`CompilerHost::is_plain_os_fs`): `fs` reads it.
        let host = super::super::host::new_compiler_host_over(
            &cwd,
            crate::frontend::vfs::cachedvfs_from(fs),
            &bundled::wrap_fs(osvfs_fs()),
            &bundled::lib_path(),
        );
        let processed = process_all_program_files_of(
            ProgramOptions {
                host,
                config: Rc::new(config.unwrap()),
                use_source_of_project_reference: false,
                single_threaded: Tristate::False,
                typings_location: String::new(),
                project_name: String::new(),
                create_module_resolver: None,
                skip_module_resolution: false,
            },
            false,
        );
        (processed, dir, cwd)
    }

    // The parse workers find the package scope of each file for its
    // metadata, and the loader keeps the package.json reads of that walk
    // in the program resolver's cache when it takes the metadata
    // (`Caches::adopt_worker_package_jsons`), as the Go loader finds the
    // metadata with the program's resolver (fileloader.go:391,
    // module/resolver.go:1755 getPackageJsonInfo, packagejson/cache.go:190
    // Set keeps the first). A lookup on the loading thread after the load
    // (Go `Program.GetPackageJsonInfo`, compiler/program.go:157) finds what
    // the load read, also when the files changed on disk since: the
    // project's package.json (the scope of `src/a.ts`) and the package
    // that `src/a.ts` imports (the scope of its `index.d.ts`). With the prep
    // on, the loader waits for the worker of each file (`set_meta_wait`), so
    // it takes the worker's metadata whatever the timing.
    #[test]
    fn the_loader_keeps_the_package_json_files_that_workers_read() {
        let tsconfig = r#"{ "compilerOptions": { "module": "nodenext", "types": [],
             "noEmit": true }, "include": ["src"] }"#;
        let files = [
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "src/a.ts",
                "import { x } from \"lib\";\nexport const a = x;\n",
            ),
            (
                "node_modules/lib/package.json",
                r#"{ "name": "lib", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            (
                "node_modules/lib/index.d.ts",
                "export declare const x: number;\n",
            ),
        ];
        // With no parse workers (one CPU, or `GOPORT_PARSE_THREADS=0`) the
        // loader reads every file itself.
        let workers = super::super::files_parser::parse_workers_enabled();
        for prep in [true, false] {
            set_load_prep(Some(prep));
            set_meta_wait(prep && workers);
            let before = crate::frontend::module::cache::adopted_package_jsons();
            let (processed, dir, cwd) =
                load_with_workers(&format!("keeps_package_jsons_{prep}"), tsconfig, &files);
            for path in ["package.json", "node_modules/lib/package.json"] {
                std::fs::write(dir.join(path), r#"{ "name": "changed" }"#).unwrap();
            }
            let resolver = processed
                .resolver
                .as_ref()
                .and_then(|resolver| resolver.as_default_resolver())
                .expect("the default resolver");
            // The name in the package.json of the package scope of `path`.
            let name = |path: &str| {
                resolver
                    .get_package_scope_for_path(path)
                    .and_then(|entry| entry.contents.clone())
                    .map(|contents| contents.fields.header_fields.name.get_value().0)
            };
            // The resolver gives a node_modules file by its real path (Go
            // `resolutionState.realPath`, module/resolver.go:1859), so the
            // scope of `lib/index.d.ts` is under the real path of the dir.
            // On macOS the temp dir is under /var, a symlink to /private/var.
            let real = osvfs_fs().realpath(&cwd);
            let names = (
                name(&format!("{cwd}/src")),
                name(&format!("{real}/node_modules/lib")),
            );
            let adopted = crate::frontend::module::cache::adopted_package_jsons() - before;
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(
                names,
                (Some("app".to_string()), Some("lib".to_string())),
                "prep {prep}"
            );
            if prep && workers {
                assert!(adopted > 0, "no worker package.json taken");
            }
        }
        set_load_prep(None);
        set_meta_wait(false);
    }

    // followups32: when the loader takes a file's metadata from its parse
    // worker, it keeps the package.json reads of the worker's package scope
    // walk in the program resolver's cache (`Caches::adopt_worker_package_jsons`),
    // so its own lookups later in the load find them and do not read the
    // files again, as the Go loader finds the metadata with the program's
    // resolver (fileloader.go:391, module/resolver.go:1755
    // getPackageJsonInfo, packagejson/cache.go:190 Set). No worker resolves
    // the module augmentation "aug" (the workers resolve imports), so the
    // loader resolves it while it loads `src/a.ts`, after it took that
    // file's metadata, and its self-name lookup (module/resolver.go:576
    // loadModuleFromSelfNameReference) asks for the package scope of `src`,
    // which the worker's walk read. The loader waits for the worker's
    // metadata (`set_meta_wait`), so this holds whatever the timing. Without
    // the take-time adoption the loader reads the project's package.json
    // itself, and the end of the load does not change that.
    #[test]
    fn the_loader_reads_no_package_json_that_a_taken_metadata_walk_read() {
        if !super::super::files_parser::parse_workers_enabled() {
            return;
        }
        let tsconfig = r#"{ "compilerOptions": { "module": "nodenext", "types": [],
             "noEmit": true }, "include": ["src"] }"#;
        let files = [
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "src/a.ts",
                "export const a = 1;\ndeclare module \"aug\" {}\n",
            ),
        ];
        // The package.json files that the loader reads.
        let reads = Rc::new(RefCell::new(Vec::<String>::new()));
        let os = bundled::wrap_fs(osvfs_fs());
        let read_file = {
            let (os, reads) = (os.clone(), reads.clone());
            move |path: &str| {
                if path.ends_with("/package.json") {
                    reads.borrow_mut().push(path.to_string());
                }
                os.read_file(path)
            }
        };
        let fs = crate::frontend::vfs::wrapvfs_wrap(
            os,
            crate::frontend::vfs::Replacements {
                read_file: Some(Box::new(read_file)),
                ..Default::default()
            },
        );
        set_load_prep(Some(true));
        set_meta_wait(true);
        let before = crate::frontend::module::cache::adopted_package_jsons();
        let (processed, dir, cwd) =
            load_with_workers_on("no_package_json_reads", tsconfig, &files, fs);
        let adopted = crate::frontend::module::cache::adopted_package_jsons() - before;
        set_load_prep(None);
        set_meta_wait(false);
        let _ = std::fs::remove_dir_all(&dir);
        let resolver = processed
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
            .expect("the default resolver");
        let cache = &resolver.caches.package_json_info_cache;
        assert!(adopted > 0, "no worker package.json taken");
        // A lookup of the load asked for the entry (a read that no lookup
        // asks for stays a pending read, which `contains_key` does not see).
        assert!(
            cache.contains_key(&cache.key(&format!("{cwd}/package.json"))),
            "the loader did not look up the package scope of src"
        );
        assert_eq!(*reads.borrow(), Vec::<String>::new());
    }

    // A parse worker that panics after it starts a job ends the job
    // (`RunningJob`), so the loader that waits for it finds the metadata and
    // parses the file itself, and the load ends with the file. The loader
    // waits for the worker of each file (`set_meta_wait`), so without the
    // guard it would wait forever: the test fails after 60 s. Go has no
    // parse worker; its loader finds the metadata (fileloader.go:391).
    #[test]
    fn a_worker_panic_leaves_no_loader_waiting() {
        use super::super::files_parser::{PANIC_IN_JOB, parse_workers_enabled};
        if !parse_workers_enabled() {
            return;
        }
        let label = "worker_panic";
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let a = format!("{}/src/a.ts", dir.to_string_lossy().replace('\\', "/"));
        *PANIC_IN_JOB.lock().unwrap() = Some(a.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            set_load_prep(Some(true));
            set_meta_wait(true);
            let tsconfig = r#"{ "compilerOptions": { "module": "nodenext", "types": [],
                 "noEmit": true }, "include": ["src"] }"#;
            let files = [
                ("package.json", r#"{ "name": "app", "type": "module" }"#),
                ("src/a.ts", "export const a = 1;\n"),
            ];
            let (processed, dir, _) = load_with_workers(label, tsconfig, &files);
            let _ = std::fs::remove_dir_all(&dir);
            let a = processed
                .files
                .iter()
                .find(|file| file.file_name().ends_with("/src/a.ts"))
                .map(|file| processed.source_file_meta_datas[file.path()].implied_node_format);
            let _ = sender.send(a);
        });
        let loaded = receiver.recv_timeout(std::time::Duration::from_secs(60));
        *PANIC_IN_JOB.lock().unwrap() = None;
        let loaded =
            loaded.expect("the loader still waits for the job of the worker that panicked");
        assert_eq!(loaded, Some(ModuleKind::ES_NEXT), "{a}");
    }

    // loadcrit2: the job of an output `.d.ts` file of a project reference,
    // which a worker parses in place of its source (`PrefetchQueue::redirects`)
    // and resolves with the reference's redirect. When its worker panics
    // after it starts the job, the loader that waits for it still loads the
    // file and resolves its names with the redirect. Without the wake the
    // test fails after 60 s. The panic ends its worker, and the loader also
    // waits for the jobs that no worker has started (`set_meta_wait`), so
    // the test needs a second worker to run them: with one
    // (`GOPORT_PARSE_THREADS=1`, two CPUs) it does not run.
    #[test]
    fn a_worker_panic_in_a_redirected_job_leaves_no_loader_waiting() {
        use super::super::files_parser::{
            PANIC_IN_JOB, parse_workers_enabled, prefetch_worker_count,
        };
        if !parse_workers_enabled() || prefetch_worker_count() < 2 {
            return;
        }
        let label = "worker_panic_redirect";
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let output = format!(
            "{}/lib/out/helper.d.ts",
            dir.to_string_lossy().replace('\\', "/")
        );
        *PANIC_IN_JOB.lock().unwrap() = Some(output.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            set_load_prep(Some(true));
            set_meta_wait(true);
            let (processed, dir, _) =
                load_with_workers(label, REFERENCE_TSCONFIG, &REFERENCE_FILES);
            let _ = std::fs::remove_dir_all(&dir);
            let dep = processed
                .files
                .iter()
                .find(|file| file.file_name().ends_with("/lib/out/helper.d.ts"))
                .and_then(|file| processed.resolved_modules.get(file.path()))
                .and_then(|resolutions| {
                    resolutions
                        .iter()
                        .find(|(key, _)| key.name == "dep")
                        .map(|(_, resolved)| get_base_file_name(&resolved.resolved_file_name))
                });
            let _ = sender.send(dep);
        });
        let loaded = receiver.recv_timeout(std::time::Duration::from_secs(60));
        *PANIC_IN_JOB.lock().unwrap() = None;
        let loaded =
            loaded.expect("the loader still waits for the job of the worker that panicked");
        assert_eq!(loaded, Some("lib.d.ts".to_string()), "{output}");
    }

    // lazypj1: the parse workers also read package.json files for the
    // module answers that the loader takes. At the end of the load those
    // reads go into the program resolver's cache, which parses one only
    // when a lookup asks for it (`SharedResolutionCache::end_package_json_reads`,
    // `InfoCache::get`), as Go's parse tasks put every read into the
    // resolver's one cache (module/resolver.go:1755 getPackageJsonInfo,
    // packagejson/cache.go:190 Set keeps the first). The package.json of
    // `lib` is in no file's package scope (its types are under `sub`, which
    // has its own package.json), so only the answer for "lib" reads it. A
    // lookup on the loading thread after the load finds what the load
    // read, also when the file changed on disk since. With the prep on, the
    // loader waits for the worker's metadata (`set_meta_wait`) and answer
    // (`set_answer_wait`), so it takes the answer for "lib" and does not
    // read the file, whatever the timing.
    #[test]
    fn the_loader_keeps_the_package_json_files_of_worker_module_answers() {
        let tsconfig = r#"{ "compilerOptions": { "module": "nodenext", "types": [],
             "noEmit": true }, "include": ["src"] }"#;
        let files = [
            ("package.json", r#"{ "name": "app", "type": "module" }"#),
            (
                "src/a.ts",
                "import { x } from \"lib\";\nexport const a = x;\n",
            ),
            (
                "node_modules/lib/package.json",
                r#"{ "name": "lib", "version": "1.0.0", "types": "sub/index.d.ts" }"#,
            ),
            ("node_modules/lib/sub/package.json", r#"{ "name": "sub" }"#),
            (
                "node_modules/lib/sub/index.d.ts",
                "export declare const x: number;\n",
            ),
        ];
        // With no parse workers the loader reads every file itself.
        let workers = super::super::files_parser::parse_workers_enabled();
        for prep in [true, false] {
            set_load_prep(Some(prep));
            set_meta_wait(prep && workers);
            set_answer_wait(prep && workers);
            let (processed, dir, cwd) = load_with_workers(
                &format!("keeps_answer_package_jsons_{prep}"),
                tsconfig,
                &files,
            );
            std::fs::write(
                dir.join("node_modules/lib/package.json"),
                r#"{ "name": "changed" }"#,
            )
            .unwrap();
            let resolver = processed
                .resolver
                .as_ref()
                .and_then(|resolver| resolver.as_default_resolver())
                .expect("the default resolver");
            let cache = &resolver.caches.package_json_info_cache;
            let package_json = format!("{cwd}/node_modules/lib/package.json");
            // The loader took the answer for "lib" and did not read the file.
            let loader_read = cache.contains_key(&cache.key(&package_json));
            let name = resolver
                .get_package_scope_for_path(&format!("{cwd}/node_modules/lib"))
                .and_then(|entry| entry.contents.clone())
                .map(|contents| contents.fields.header_fields.name.get_value().0);
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(name, Some("lib".to_string()), "prep {prep}");
            if prep && workers {
                assert!(!loader_read, "the loader read the package.json of lib");
            }
        }
        set_load_prep(None);
        set_meta_wait(false);
        set_answer_wait(false);
    }

    /// A fake content mapper process over a socket pair (Go `net.Pipe` in
    /// the Go host tests). It answers `initialize` and `openProject` at
    /// once, and the transform requests newest first on another thread, so
    /// the answers come out of order when requests overlap. The text of a
    /// mapped file is TypeScript: `@diag` adds a mapper diagnostic, `@fail`
    /// answers with an error, `@badmap` gives a mapping that fails
    /// `SpanMap::validate`, `@supp` adds a supplemental output. After
    /// `exit_after` transform requests it closes the connection (a mapper
    /// crash). `transforms` gets the file name of each transform request.
    #[cfg(unix)]
    mod fake_mapper {
        use crate::contentmapper::{
            Diagnostic, InitializeResult, MappedOutput, OpenProjectResult, PositionEncoding,
            ProcessExitState, SupplementalOutput, TransformParams, TransformResult,
        };
        use crate::frontend::json::json_unmarshal;
        use crate::frontend::json_ext::{AnyValue, JsonValue};
        use crate::gostd::GoError;
        use crate::ipc::{self, Message, Protocol as _, ReadWriteCloser};
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Condvar, Mutex};

        struct End(UnixStream);

        impl ReadWriteCloser for End {
            fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
                (&self.0).read(buf)
            }

            fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
                (&self.0).write(buf)
            }

            fn flush(&self) -> std::io::Result<()> {
                (&self.0).flush()
            }

            fn close(&self) -> Result<(), GoError> {
                let _ = self.0.shutdown(std::net::Shutdown::Both);
                Ok(())
            }
        }

        impl ProcessExitState for End {}

        type Queue = Arc<(Mutex<(Vec<Message>, bool)>, Condvar)>;

        pub(super) fn spawn(
            transforms: Arc<Mutex<Vec<String>>>,
            exit_after: Option<usize>,
        ) -> Arc<dyn ProcessExitState> {
            let (client, server) = UnixStream::pair().expect("socket pair");
            let server: Arc<dyn ReadWriteCloser> = Arc::new(End(server));
            let write = Arc::new(Mutex::new(()));
            let queue: Queue = Arc::default();
            let answer = {
                let (server, write) = (server.clone(), write.clone());
                move |msg: &Message, result: Option<Box<dyn AnyValue>>| {
                    let _write = write.lock().unwrap();
                    let _ = ipc::new_jsonrpc_protocol(server.clone())
                        .write_response(msg.id.as_ref(), result);
                }
            };
            {
                let (server, write, queue) = (server.clone(), write.clone(), queue.clone());
                std::thread::spawn(move || {
                    loop {
                        let msg = {
                            let (lock, ready) = &*queue;
                            let mut queue = lock.lock().unwrap();
                            loop {
                                if let Some(msg) = queue.0.pop() {
                                    break msg;
                                }
                                if queue.1 {
                                    return;
                                }
                                queue = ready.wait(queue).unwrap();
                            }
                        };
                        let mut params = TransformParams::default();
                        json_unmarshal(&msg.params.0, &mut params, &[]).unwrap();
                        let _write = write.lock().unwrap();
                        let mut protocol = ipc::new_jsonrpc_protocol(server.clone());
                        if params.content.contains("@fail") {
                            let _ = protocol.write_error(
                                msg.id.as_ref(),
                                &crate::jsonrpc::ResponseError {
                                    code: crate::jsonrpc::CODE_INTERNAL_ERROR,
                                    message: "fake failure".to_string(),
                                    data: None,
                                },
                            );
                            continue;
                        }
                        let _ = protocol.write_response(msg.id.as_ref(), Some(transform(&params)));
                    }
                });
            }
            std::thread::spawn(move || {
                let mut protocol = ipc::new_jsonrpc_protocol(server.clone());
                let mut count = 0;
                while let Ok(msg) = protocol.read_message() {
                    match msg.method.as_str() {
                        "initialize" => answer(
                            &msg,
                            Some(Box::new(InitializeResult {
                                position_encoding: PositionEncoding::UTF8,
                                diagnostic_source: "fake".to_string(),
                            })),
                        ),
                        "openProject" => {
                            answer(&msg, Some(Box::new(OpenProjectResult::default())));
                        }
                        "closeProject" => answer(&msg, None),
                        "transform" => {
                            count += 1;
                            if exit_after.is_some_and(|limit| count > limit) {
                                let _ = server.close();
                                break;
                            }
                            let mut params = TransformParams::default();
                            json_unmarshal(&msg.params.0, &mut params, &[]).unwrap();
                            transforms.lock().unwrap().push(params.file_name);
                            let (lock, ready) = &*queue;
                            lock.lock().unwrap().0.push(msg);
                            ready.notify_one();
                        }
                        _ => {}
                    }
                }
                let (lock, ready) = &*queue;
                lock.lock().unwrap().1 = true;
                ready.notify_one();
            });
            Arc::new(End(client))
        }

        /// `text` mapped to itself, up to `end`.
        fn verbatim(text: &str, extension: &str, end: usize) -> MappedOutput {
            let mappings = crate::spanmap::new(&[crate::spanmap::Segment {
                virtual_end: end as i32,
                original_end: text.len() as i32,
                kind: crate::spanmap::Kind::VERBATIM,
                ..Default::default()
            }])
            .marshal()
            .unwrap();
            MappedOutput {
                text: text.to_string(),
                extension: extension.to_string(),
                mappings: JsonValue(mappings),
                diagnostic_directives: None,
            }
        }

        fn transform(params: &TransformParams) -> Box<dyn AnyValue> {
            // `@badmap`: the verbatim mapping ends one byte before the
            // virtual text, so its two lengths differ. The decode of the
            // answer takes it, and `SpanMap::validate` fails it.
            let end = params.content.len() - usize::from(params.content.contains("@badmap"));
            let mut result = TransformResult {
                mapped_output: verbatim(&params.content, ".ts", end),
                ..Default::default()
            };
            if params.content.contains("@diag") {
                result.diagnostics.push(Diagnostic {
                    message_text: "fake diagnostic".to_string(),
                    start: 0,
                    length: 1,
                    code: 9001,
                });
            }
            if params.content.contains("@supp") {
                result.supplemental.push(SupplementalOutput {
                    mapped_output: MappedOutput {
                        text: "export const supplemental = 1;\n".to_string(),
                        extension: ".ts".to_string(),
                        mappings: JsonValue(b"[]".to_vec()),
                        diagnostic_directives: None,
                    },
                });
            }
            Box::new(result)
        }
    }

    /// Loads a program of `files` in a temp dir with the fake mapper for
    /// `.vue` files. Returns each program file (name, text and parse
    /// diagnostics) and the content mapper diagnostics, one per line, the
    /// file name of each transform request, the transforms that the loader
    /// took from the parse workers (file name, and whether a worker parse
    /// came with it), and the spawns.
    #[cfg(unix)]
    fn load_mapped(
        label: &str,
        tsconfig: &str,
        files: &[(String, String)],
        single_threaded: bool,
        exit_after: Option<usize>,
    ) -> (Vec<String>, Vec<String>, Vec<(String, bool)>, usize) {
        use crate::contentmapper::{self, HostOptions, ProjectSpec, SpawnerFunc};
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_file_loader_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mapper_package = (
            "node_modules/fake-mapper/package.json".to_string(),
            r#"{ "name": "fake-mapper", "version": "1.0.0",
                 "typescript": { "contentMapper": { "exec": ["fake"] } } }"#
                .to_string(),
        );
        for (path, text) in files.iter().chain([&mapper_package]) {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
        let cwd = dir.to_string_lossy().replace('\\', "/");
        let fs = bundled::wrap_fs(osvfs_fs());
        let sys = System {
            fs: fs.clone(),
            current_directory: cwd.clone(),
        };
        // Go `tsc --runExternalCode`.
        let options = CompilerOptions {
            run_external_code: Tristate::True,
            ..Default::default()
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &format!("{cwd}/tsconfig.json"),
            Some(&options),
            None,
            &sys,
            None,
        );
        assert!(errors.is_empty());
        let config = Rc::new(config.unwrap());
        assert_eq!(config.content_mappers().len(), 1);
        let transforms = Arc::new(std::sync::Mutex::new(Vec::new()));
        let spawns = Rc::new(Cell::new(0));
        let spawner = {
            let (transforms, spawns) = (transforms.clone(), spawns.clone());
            SpawnerFunc(Box::new(move |_, _, _| {
                spawns.set(spawns.get() + 1);
                Ok(fake_mapper::spawn(transforms.clone(), exit_after))
            }))
        };
        let ctx = crate::gostd::context::background();
        let host = contentmapper::new_host_with_options(
            &ctx,
            Rc::new(spawner),
            crate::locale::DEFAULT,
            HostOptions { logger: None },
        );
        let project = host.project(ProjectSpec {
            config_file_name: config.config_name().to_string(),
            mappers: config.content_mappers().to_vec(),
            compiler_options: Some(config.compiler_options().clone()),
        });
        let compiler_host = new_cached_fs_compiler_host(
            &cwd,
            fs,
            &bundled::lib_path(),
            None,
            None,
            project.clone(),
        );
        let processed = process_all_program_files_of(
            ProgramOptions {
                host: compiler_host,
                config,
                use_source_of_project_reference: false,
                single_threaded: if single_threaded {
                    Tristate::True
                } else {
                    Tristate::False
                },
                typings_location: String::new(),
                project_name: String::new(),
                create_module_resolver: None,
                skip_module_resolution: false,
            },
            single_threaded,
        );
        if let Some(project) = project {
            let _ = project.close();
        }
        let _ = host.close();
        let _ = std::fs::remove_dir_all(&dir);
        let diagnostic = |d: &Diagnostic| {
            format!(
                "{:?}",
                (
                    d.pos,
                    d.end,
                    d.code,
                    &d.source,
                    &d.message_text,
                    &d.message_args
                )
            )
        };
        let mut lines: Vec<String> = processed
            .files
            .iter()
            .filter(|file| file.file_name().starts_with(&cwd))
            .map(|file| {
                let name = file.file_name().replace(&cwd, "");
                let diagnostics: Vec<String> = file.diagnostics.iter().map(diagnostic).collect();
                format!("{name} {:?} {diagnostics:?}", file.text())
            })
            .collect();
        lines.extend(processed.content_mapper_diagnostics.iter().map(diagnostic));
        let transforms = std::mem::take(&mut *transforms.lock().unwrap());
        let transforms = transforms
            .into_iter()
            .map(|name| name.replace(&cwd, ""))
            .collect();
        let taken = super::super::files_parser::MAPPED_TAKEN
            .lock()
            .unwrap()
            .extract_if(.., |(name, _)| name.starts_with(&cwd))
            .map(|(name, parsed)| (name.replace(&cwd, ""), parsed))
            .collect();
        (lines, transforms, taken, spawns.get())
    }

    #[cfg(unix)]
    const MAPPED_TSCONFIG: &str = r#"{ "compilerOptions": { "module": "preserve",
         "moduleResolution": "bundler", "types": [], "noEmit": true },
         "include": ["src"],
         "contentMappers": [{ "package": "fake-mapper", "extensions": [".vue"] }] }"#;

    /// `count` mapped files that import the next one, with `marker` in
    /// file `i` when `marker(i)` names one.
    #[cfg(unix)]
    fn mapped_files(count: usize, marker: fn(usize) -> &'static str) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = (0..count)
            .map(|i| {
                let next = (i + 1) % count;
                (
                    format!("src/C{i}.vue"),
                    format!(
                        "// {}\nimport {{ v{next} }} from \"./C{next}.vue\";\nexport const v{i} = v{next};\n",
                        marker(i)
                    ),
                )
            })
            .collect();
        files.push((
            "src/main.ts".to_string(),
            "import { v0 } from \"./C0.vue\";\nexport const main = v0;\n".to_string(),
        ));
        files
    }

    // cmpar1: once the loader's transform of the first mapped file opened
    // the mapper project, the parse workers send the transforms of the
    // other mapped files and parse the virtual texts, and the loader takes
    // their results. Each file gets one transform request, also with a
    // mapper diagnostic, an error response, a mapping that fails
    // `SpanMap::validate` (the worker does not parse it, and the loader
    // sends no second request for it) or a supplemental output, and
    // the program equals a load on one thread. The fake mapper answers the
    // newest request first, so answers come out of order when requests
    // overlap. Which files the workers take varies, so the parallel load
    // runs again, at most 10 times, until a worker sent the transform of
    // the `@badmap` file. Go sends the transforms from its parse goroutines
    // (fileloader.go:438 parseContentMappedFile), and parses only after the
    // mapping check (transform.go:48-58 ParseResult).
    #[cfg(unix)]
    #[test]
    fn workers_send_the_content_mapper_transforms() {
        use super::super::files_parser::parse_workers_enabled;
        let files = mapped_files(40, |i| match i {
            3 | 17 | 31 => "@diag",
            9 | 26 => "@fail",
            21 => "@badmap",
            12 | 35 => "@supp",
            _ => "",
        });
        let (serial, mut serial_transforms, _, _) =
            load_mapped("mapped_serial", MAPPED_TSCONFIG, &files, true, None);
        // One request per mapped file: a file with two requests, or with
        // none, fails here.
        let mut want: Vec<String> = (0..40).map(|i| format!("/src/C{i}.vue")).collect();
        want.sort();
        serial_transforms.sort();
        assert_eq!(serial_transforms, want);
        for attempt in 0..10 {
            let (parallel, mut parallel_transforms, taken, _) = load_mapped(
                &format!("mapped_parallel_{attempt}"),
                MAPPED_TSCONFIG,
                &files,
                false,
                None,
            );
            parallel_transforms.sort();
            assert_eq!(parallel_transforms, want, "one transform per file");
            assert_eq!(parallel.join("\n"), serial.join("\n"));
            if !parse_workers_enabled() {
                return;
            }
            assert!(
                taken.iter().any(|(_, parsed)| *parsed),
                "the loader took no worker parse: {taken:?}"
            );
            if let Some((_, parsed)) = taken.iter().find(|(name, _)| name == "/src/C21.vue") {
                assert!(
                    !parsed,
                    "a worker parsed the virtual text of a failed mapping"
                );
                return;
            }
        }
        panic!("no worker sent the transform of the @badmap file in 10 loads");
    }

    // A mapper that crashes during the load: the calls in flight and all
    // later calls fail, the first 5 failures in load order are reported,
    // then the mapper is disabled, and the load ends.
    #[cfg(unix)]
    #[test]
    fn a_mapper_crash_during_worker_transforms_ends_the_load() {
        let files = mapped_files(40, |_| "");
        for single_threaded in [true, false] {
            let (lines, _, _, _) = load_mapped(
                &format!("mapped_crash_{single_threaded}"),
                MAPPED_TSCONFIG,
                &files,
                single_threaded,
                Some(10),
            );
            let count = |code: u32| {
                lines
                    .iter()
                    .filter(|line| line.contains(&format!(", {code}, ")))
                    .count()
            };
            let failed = diag::The_content_mapper_0_failed_to_transform_this_file.code();
            let disabled = diag::The_content_mapper_0_failed_1_times_and_will_not_be_used.code();
            assert_eq!(
                (count(failed), count(disabled)),
                (5, 1),
                "single threaded {single_threaded}:\n{}",
                lines.join("\n")
            );
        }
    }

    // A mapped file that the program names but that does not exist sends
    // no request: the mapper does not start, so the project does not open
    // and reports no option diagnostics (Go program.go:828 reports them
    // only for opened projects).
    #[cfg(unix)]
    #[test]
    fn a_missing_mapped_file_starts_no_mapper() {
        let tsconfig = r#"{ "compilerOptions": { "types": [], "noEmit": true },
             "files": ["missing.vue", "a.ts"],
             "contentMappers": [{ "package": "fake-mapper", "extensions": [".vue"] }] }"#;
        let files = [("a.ts".to_string(), "export const a = 1;\n".to_string())];
        for single_threaded in [true, false] {
            let (_, transforms, _, spawns) = load_mapped(
                &format!("mapped_missing_{single_threaded}"),
                tsconfig,
                &files,
                single_threaded,
                None,
            );
            assert_eq!((transforms.len(), spawns), (0, 0));
        }
    }

    /// A temp dir with `files` and a `tsconfig.json` with `tsconfig`, its
    /// parsed config and a compiler host on it. The caller removes the dir.
    fn ts64519_project(
        label: &str,
        tsconfig: &str,
        files: &[(&str, &str)],
    ) -> (std::path::PathBuf, String, Rc<ParsedCommandLine>) {
        let dir =
            std::env::temp_dir().join(format!("ts_goport_ts64519_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        std::fs::write(dir.join("tsconfig.json"), tsconfig).unwrap();
        let cwd = dir.to_string_lossy().replace('\\', "/");
        let sys = System {
            fs: bundled::wrap_fs(osvfs_fs()),
            current_directory: cwd.clone(),
        };
        let (config, errors) = get_parsed_command_line_of_config_file(
            &format!("{cwd}/tsconfig.json"),
            None,
            None,
            &sys,
            None,
        );
        assert!(errors.is_empty());
        (dir, cwd, Rc::new(config.unwrap()))
    }

    /// A fresh compiler host on the OS file system in `cwd`.
    fn ts64519_host(cwd: &str) -> Rc<dyn CompilerHost> {
        new_cached_fs_compiler_host(
            cwd,
            bundled::wrap_fs(osvfs_fs()),
            &bundled::lib_path(),
            None,
            None,
            None,
        )
    }

    fn ts64519_options(
        host: Rc<dyn CompilerHost>,
        config: &Rc<ParsedCommandLine>,
        create_module_resolver: Option<CreateModuleResolver>,
    ) -> ProgramOptions {
        ProgramOptions {
            host,
            config: config.clone(),
            use_source_of_project_reference: false,
            single_threaded: Tristate::True,
            typings_location: String::new(),
            project_name: String::new(),
            create_module_resolver,
            skip_module_resolution: false,
        }
    }

    /// A module resolver that fails every module name, as an API resolver
    /// whose callback fails (ts#64299).
    struct FailingResolver(DefaultResolver);

    impl Resolver for FailingResolver {
        fn resolve_module_name(
            &self,
            _module_name: &str,
            _containing_file: &str,
            _resolution_mode: ResolutionMode,
            _redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
        ) -> (
            Option<Arc<ResolvedModule>>,
            Vec<DiagAndArgs>,
            Option<GoError>,
        ) {
            (None, Vec::new(), Some(errors::new("resolver failed")))
        }
        fn resolve_module_name_from_directory(
            &self,
            module_name: &str,
            containing_directory: &str,
            resolution_mode: ResolutionMode,
        ) -> (
            Option<Arc<ResolvedModule>>,
            Vec<DiagAndArgs>,
            Option<GoError>,
        ) {
            let (resolved, trace, err) = self.0.resolve_module_name_from_directory(
                module_name,
                containing_directory,
                resolution_mode,
            );
            (Some(resolved), trace, err)
        }
        fn resolve_type_reference_directive(
            &self,
            name: &str,
            containing_file: &str,
            resolution_mode: ResolutionMode,
            redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
        ) -> (Rc<ResolvedTypeReferenceDirective>, Vec<DiagAndArgs>) {
            self.0.resolve_type_reference_directive(
                name,
                containing_file,
                resolution_mode,
                redirected_reference,
            )
        }
        fn get_package_scope_for_path(&self, directory: &str) -> Option<Rc<InfoCacheEntry>> {
            self.0.get_package_scope_for_path(directory)
        }
        fn package_json_cache_entries(
            &self,
            f: &mut dyn FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
        ) {
            self.0.package_json_cache_entries(f);
        }
        fn resolve_package_directory(
            &self,
            module_name: &str,
            containing_file: &str,
            resolution_mode: ResolutionMode,
            redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
        ) -> Option<ResolvedModule> {
            self.0.resolve_package_directory(
                module_name,
                containing_file,
                resolution_mode,
                redirected_reference,
            )
        }
    }

    // Go: compiler/program_test.go:80 TestIncludeReasonDiagnosticsAreProgramLocal
    // (ts#64519). PORT: Go compares the cached pointers of two programs; a
    // reused program here shares the reason, and each program caches its own
    // diagnostics.
    #[test]
    fn include_reason_diagnostics_are_program_local() {
        let (dir, cwd, config) = ts64519_project(
            "reasons",
            r#"{"compilerOptions":{"noLib":true},"files":["index.ts"]}"#,
            &[("index.ts", "export const a = 1;")],
        );
        let _scope = crate::core::enter_program(None);
        let old = new_program(ts64519_options(ts64519_host(&cwd), &config, None));
        let path = old
            .get_source_file(&format!("{cwd}/index.ts"))
            .unwrap()
            .path()
            .clone();
        let (new, _, reused) = old.reuse_program(&path, ts64519_host(&cwd), None);
        assert!(reused);
        let new = new.unwrap();
        let reason = old.get_include_reasons()[&path][0].clone();
        assert!(Rc::ptr_eq(&reason, &new.get_include_reasons()[&path][0]));
        for relative in [false, true] {
            let old_diagnostic = reason.to_diagnostic(&old, relative, "");
            assert_eq!(
                old.include_processor.reason_diagnostics.borrow().len(),
                usize::from(relative) + 1
            );
            assert_eq!(
                new.include_processor.reason_diagnostics.borrow().len(),
                usize::from(relative)
            );
            let new_diagnostic = reason.to_diagnostic(&new, relative, "");
            assert_eq!(
                new.include_processor.reason_diagnostics.borrow().len(),
                usize::from(relative) + 1
            );
            assert_eq!(new_diagnostic.message_text(), old_diagnostic.message_text());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Go: compiler/program.go:396 (ts#64519): a program whose module resolver
    // failed is never reused; it is built again.
    #[test]
    fn module_resolution_error_blocks_reuse() {
        let (dir, cwd, config) = ts64519_project(
            "resolver_error",
            r#"{"compilerOptions":{"noLib":true},"files":["index.ts"]}"#,
            &[
                (
                    "index.ts",
                    r#"import { b } from "./b"; export const a = b;"#,
                ),
                ("b.ts", "export const b = 1;"),
            ],
        );
        let _scope = crate::core::enter_program(None);
        let failing: CreateModuleResolver = Rc::new(|options: ResolverOptions| {
            let resolver: Rc<dyn Resolver> = Rc::new(FailingResolver(new_resolver(options)));
            resolver
        });
        let p = new_program(ts64519_options(ts64519_host(&cwd), &config, Some(failing)));
        assert!(p.module_resolution_error().is_some());
        let path = p
            .get_source_file(&format!("{cwd}/index.ts"))
            .unwrap()
            .path()
            .clone();
        let (cloned, new_file, reused) = p.reuse_program(&path, ts64519_host(&cwd), None);
        assert!(!reused && cloned.is_none() && new_file.is_some());
        // The full build with the default resolver resolves the import.
        let (rebuilt, _, reused) = p.update_program(&path, ts64519_host(&cwd), None);
        assert!(!reused);
        assert!(rebuilt.module_resolution_error().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Go: compiler/program_test.go:95 TestProgramHostsAndFactories (ts#64519):
    // a program does not keep its factories. A reuse calls none of them, and
    // an update that builds again uses only the factory that it is given.
    // PORT: the checker pool factory is `ls_program`'s; this checks the
    // module resolver factory.
    #[test]
    fn program_does_not_keep_its_factories() {
        let (dir, cwd, config) = ts64519_project(
            "factories",
            r#"{"compilerOptions":{"noLib":true},"files":["index.ts"]}"#,
            &[
                (
                    "index.ts",
                    r#"import { b } from "./b"; export const a = b;"#,
                ),
                ("b.ts", "export const b = 1;"),
            ],
        );
        let _scope = crate::core::enter_program(None);
        let resolvers = Rc::new(Cell::new(0));
        let counting: CreateModuleResolver = {
            let resolvers = resolvers.clone();
            Rc::new(move |options: ResolverOptions| {
                resolvers.set(resolvers.get() + 1);
                let resolver: Rc<dyn Resolver> = Rc::new(new_resolver(options));
                resolver
            })
        };
        let p = new_program(ts64519_options(
            ts64519_host(&cwd),
            &config,
            Some(counting.clone()),
        ));
        assert_eq!(resolvers.get(), 1);
        let path = p
            .get_source_file(&format!("{cwd}/index.ts"))
            .unwrap()
            .path()
            .clone();
        let new_host = ts64519_host(&cwd);
        let (cloned, _, reused) = p.reuse_program(&path, new_host.clone(), Some(counting.clone()));
        assert!(reused);
        assert_eq!(resolvers.get(), 1);
        assert!(Rc::ptr_eq(cloned.unwrap().host(), &new_host));
        // An import change builds the program again.
        std::fs::write(dir.join("index.ts"), "export const a = 2;").unwrap();
        let (rebuilt, _, reused) = p.update_program(&path, ts64519_host(&cwd), None);
        assert!(!reused);
        assert_eq!(
            resolvers.get(),
            1,
            "the old program's factory is not used again"
        );
        let (_, _, reused) = rebuilt.update_program(&path, ts64519_host(&cwd), Some(counting));
        assert!(reused);
        assert_eq!(resolvers.get(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Go: compiler/program_test.go:482 TestImportSourceProgram (ts#63915)
    #[test]
    fn import_source_program() {
        for (name, source, evaluation) in [
            (
                "static",
                r#"import source a from "./a.js";"#,
                r#"import { a as value } from "./a.js";"#,
            ),
            (
                "dynamic",
                r#"import.source("./a.js");"#,
                r#"import("./a.js");"#,
            ),
        ] {
            let content =
                format!(r#"{source}import source b from "missing"; import.source("other");"#);
            let (dir, cwd, config) = ts64519_project(
                &format!("import_source_{name}"),
                r#"{"compilerOptions":{"module":"esnext","noLib":true},"files":["index.ts"]}"#,
                &[("index.ts", &content), ("a.ts", "export const a = 1;")],
            );
            let _scope = crate::core::enter_program(None);
            let index = format!("{cwd}/index.ts");
            let a = format!("{cwd}/a.ts");
            let program = new_program(ts64519_options(ts64519_host(&cwd), &config, None));
            let file = program.get_source_file(&index).unwrap();
            assert!(program.get_source_file(&a).is_none(), "{name}");
            assert!(
                program
                    .resolved_modules
                    .get(file.path())
                    .is_none_or(|resolutions| resolutions.is_empty()),
                "{name}"
            );
            assert!(program.get_unresolved_imports().is_empty(), "{name}");
            assert!(program.unresolved_package_names().is_empty(), "{name}");
            let path = file.path().clone();
            let assert_resolutions = |program: &NewProgram, file: &ParsedSourceFile| {
                for &specifier in &file.imports {
                    let resolved =
                        program.get_resolved_module_from_module_specifier(file, specifier);
                    assert_eq!(
                        resolved.is_some_and(|resolved| resolved.is_resolved()),
                        !is_source_phase_import(specifier.parent()),
                        "{name}"
                    );
                }
            };

            let edited = format!("{evaluation}{}", content.strip_prefix(source).unwrap());
            std::fs::write(dir.join("index.ts"), &edited).unwrap();
            let (program, file, reused) = program.update_program(&path, ts64519_host(&cwd), None);
            assert!(!reused, "{name}");
            assert!(program.get_source_file(&a).is_some(), "{name}");
            assert_resolutions(&program, &file.unwrap());

            std::fs::write(dir.join("index.ts"), &content).unwrap();
            let (program, _, reused) = program.update_program(&path, ts64519_host(&cwd), None);
            assert!(!reused, "{name}");
            assert!(program.get_source_file(&a).is_none(), "{name}");

            std::fs::write(dir.join("index.ts"), format!("{evaluation}{content}")).unwrap();
            let program = new_program(ts64519_options(ts64519_host(&cwd), &config, None));
            let file = program.get_source_file(&index).unwrap();
            assert_resolutions(&program, &file);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // ts#64159 (fileloader.go:747): "A file cannot have a reference to
    // itself" is checked on the resolved file, so a reference without an
    // extension that resolves to its own file is TS1006 too. A reference to
    // another spelling that does not exist stays "not found".
    #[test]
    fn triple_slash_self_reference_is_checked_after_resolution() {
        let (dir, _cwd, config) = ts64519_project(
            "self_reference",
            r#"{"compilerOptions":{"noLib":true},"files":["a.ts","b.ts","c.ts"]}"#,
            &[
                (
                    "a.ts",
                    "/// <reference path=\"a\" />\nexport const a = 1;\n",
                ),
                (
                    "b.ts",
                    "/// <reference path=\"./b.ts\" />\nexport const b = 1;\n",
                ),
                (
                    "c.ts",
                    "/// <reference path=\"./C.ts\" />\nexport const c = 1;\n",
                ),
            ],
        );
        let _scope = crate::core::enter_program(None);
        let cwd = _cwd;
        let program = new_program(ts64519_options(ts64519_host(&cwd), &config, None));
        let codes = |name: &str| {
            let file = program.get_source_file(&format!("{cwd}/{name}")).unwrap();
            program
                .include_processor
                .get_diagnostics(&program)
                .borrow_mut()
                .get_diagnostics_for_file(file.root)
                .iter()
                .map(Diagnostic::code)
                .collect::<Vec<_>>()
        };
        assert_eq!(codes("a.ts"), [1006]);
        assert_eq!(codes("b.ts"), [1006]);
        assert_eq!(codes("c.ts"), [6053]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
