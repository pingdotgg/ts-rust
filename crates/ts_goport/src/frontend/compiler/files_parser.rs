//! Go: compiler/filesparser.go (parse tasks, the files parser and
//! `getProcessedFiles`).

use super::file_loader::{ImportNames, SyntheticImports};
use crate::frontend::prelude::*;
use crate::gostd::slices::stable_sort_by;
use crate::program::ThreadBudget;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::hash::BuildHasher;
use std::rc::Weak;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use crate::contentmapper::{ConcurrentTransform, PrefetchedTransform};

// Go: filesparser.go:19 parseTask
// PORT: Go `*parseTask` is shared by the root task list, sub task lists,
// `parseTaskData.tasks` and `loadedTask`, and changed through all of them.
// It is `Rc<RefCell<ParseTask>>` (`ParseTaskRef`).
#[derive(Default)]
pub struct ParseTask {
    pub normalized_file_path: String,
    pub path: Path,
    pub file: Option<Rc<ParsedSourceFile>>,
    pub lib_file: Option<Rc<LibFile>>,
    pub redirected_parse_task: Option<ParseTaskRef>,
    pub sub_tasks: Vec<ParseTaskRef>,
    pub loaded: bool,
    pub started_sub_tasks: bool,
    pub is_for_automatic_type_directive: bool,
    // tsgo#4712
    pub is_content_mapper_supplemental: bool,
    pub failed_lookup: bool,
    pub include_reason: Option<Rc<FileIncludeReason>>,
    pub package_id: PackageId,

    pub metadata: SourceFileMetaData,
    pub resolutions_in_file: ModeAwareCache<Arc<ResolvedModule>>,
    pub resolutions_trace: Vec<DiagAndArgs>,
    pub type_resolutions_in_file: ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>,
    pub type_resolutions_trace: Vec<DiagAndArgs>,
    pub resolution_diagnostics: Vec<Diagnostic>,
    pub processing_diagnostics: Vec<Rc<ProcessingDiagnostic>>,
    pub import_helpers_import_specifier: Node,
    pub jsx_runtime_import_specifier: Option<Rc<JsxRuntimeImportSpecifier>>,

    pub increase_depth: bool,
    pub elide_on_depth: bool,

    pub loaded_task: Option<ParseTaskRef>,
    pub all_include_reasons: Vec<Rc<FileIncludeReason>>,
    /// The task data of the task's path (`task_data_by_path`), set when
    /// `FilesParser::start` queues the task.
    // PERF (parload1 step 4): Go looks the data up by path at each visit of
    // `collectFiles`; the link saves the string hash per visit. A `Weak`
    // link makes no `Rc` cycle (task, data, task).
    pub data: Weak<RefCell<ParseTaskData>>,
    /// The hash of `path` in `task_data_by_path` (`FxBuildHasher`) that a
    /// parse worker made (`PrepAdd::path_hash`), or 0: `start` makes it (a
    /// worker hash of 0 is made again).
    // PERF (loadfree1 C6): `start` then does not read the path for the
    // hash on the loader. A `u64`, not an `Option`: the `Rc` of a task stays
    // in the 512-byte size class. Not in Go.
    pub path_hash: u64,
}

/// Go `*parseTask`.
pub type ParseTaskRef = Rc<RefCell<ParseTask>>;

impl ParseTask {
    // Go: filesparser.go:51 (*parseTask).FileName
    pub fn file_name(&self) -> String {
        self.normalized_file_path.clone()
    }

    // Go: filesparser.go:55 (*parseTask).Path (at 673a5f17d713; ts#64159 renames it PathKey, compiler/filesparser.go:55)
    pub fn path(&self) -> Path {
        self.path.clone()
    }

    // Go: filesparser.go:59 (*parseTask).load
    pub fn load(&mut self, loader: &FileLoader) {
        self.loaded = true;
        if self.is_for_automatic_type_directive {
            self.load_automatic_type_directives(loader);
            return;
        }
        if self.failed_lookup {
            // The root file name did not resolve to a supported extension; the task
            // exists only to carry its processing diagnostic, so nothing is parsed.
            return;
        }
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Program,
                "findSourceFile",
                vec![("fileName", self.normalized_file_path.clone().into())],
                false,
            )
        });
        let redirect = loader
            .project_reference_file_mapper
            .borrow()
            .get_parse_file_redirect(&new_has_file_name(&self.normalized_file_path, &self.path));
        if !redirect.is_empty() {
            self.redirect(loader, &redirect);
            return;
        }

        if !self.is_content_mapper_supplemental && has_extension(&self.normalized_file_path) {
            let compiler_options = loader.opts.config.compiler_options();
            let allow_non_ts_extensions = compiler_options.allow_non_ts_extensions.is_true();
            if !allow_non_ts_extensions {
                let canonical_file_name = get_canonical_file_name(
                    &self.normalized_file_path,
                    loader.host.fs().use_case_sensitive_file_names(),
                );
                if !loader.is_supported_extension(&canonical_file_name) {
                    if has_js_file_extension(&canonical_file_name) {
                        self.processing_diagnostics.push(new_explaining_processing_diagnostic(
                            self.include_reason.clone(),
                            diag::File_0_is_a_JavaScript_file_Did_you_mean_to_enable_the_allowJs_option,
                            args![self.normalized_file_path.clone()],
                        ));
                    } else {
                        self.processing_diagnostics.push(new_explaining_processing_diagnostic(
                            self.include_reason.clone(),
                            diag::File_0_has_an_unsupported_extension_The_only_supported_extensions_are_1,
                            args![
                                self.normalized_file_path.clone(),
                                format!("'{}'", join_flattened_extensions(&loader.supported_extensions))
                            ],
                        ));
                    }
                    return;
                }
            }
        }

        loader
            .total_file_count
            .set(loader.total_file_count.get() + 1);
        if self.lib_file.is_some() {
            loader.lib_file_count.set(loader.lib_file_count.get() + 1);
            // Default lib files are all scripts; we can safely skip looking up their package.json
            // to avoid adding spurious lookups to file watcher tracking.
            self.metadata = SourceFileMetaData {
                implied_node_format: ModuleKind::COMMON_JS,
                ..Default::default()
            };
        } else {
            // PERF (loadpar1): the metadata that the parse worker of the
            // file found, when it found it for this loader.
            self.metadata = take_prefetched_meta(loader, &self.normalized_file_path)
                .unwrap_or_else(|| loader.load_source_file_meta_data(&self.normalized_file_path));
        }

        // tsgo#4712: a content mapper supplemental task comes with its file.
        TAKEN.with(|taken| taken.borrow_mut().take());
        let file = match self.file.clone() {
            Some(file) => Some(file),
            None => loader.parse_source_file(self),
        };
        let Some(file) = file else {
            return;
        };
        // The worker parse that `parse_source_file` took, if `file` is it.
        let taken = TakenParse::of(&file);

        self.file = Some(file.clone());
        let virtual_file_name = file.virtual_file_name();
        if !virtual_file_name.is_empty() {
            self.metadata.implied_node_format = get_implied_node_format_for_file(
                virtual_file_name,
                &self.metadata.package_json_type,
            );
        }
        self.sub_tasks = Vec::with_capacity(
            file.referenced_files.len() + file.imports.len() + file.module_augmentations.len(),
        );

        let compiler_options = loader.opts.config.compiler_options();
        if !compiler_options.no_resolve.is_true() && !loader.opts.skip_module_resolution {
            for (index, ref_) in file.referenced_files.iter().enumerate() {
                let (resolved_ref, processing_diagnostic) = loader
                    .resolve_tripleslash_path_reference(
                        &ref_.file_name,
                        &file.file_name(),
                        index as i32,
                    );
                if let Some(processing_diagnostic) = processing_diagnostic {
                    self.processing_diagnostics.push(processing_diagnostic);
                    continue;
                }
                self.add_sub_task(
                    resolved_ref.expect("resolved reference without a diagnostic"),
                    None,
                );
            }

            loader.resolve_type_reference_directives(self);
        }

        if compiler_options.no_lib != Tristate::True && !loader.opts.skip_module_resolution {
            for (index, lib) in file.lib_reference_directives.iter().enumerate() {
                let include_reason = new_file_include_reason(
                    FileIncludeKind::LIB_REFERENCE_DIRECTIVE,
                    FileIncludeData::ReferencedFile(ReferencedFileData {
                        file: self.path.clone(),
                        index: index as i32,
                        synthetic: Node::NIL,
                    }),
                );
                let (name, ok) = get_lib_file_name(&lib.file_name);
                if ok {
                    let lib_file = loader.path_for_lib_file(&name);
                    self.add_sub_task(
                        ResolvedRef {
                            file_name: lib_file.path.clone(),
                            path: lib_file.path_key.clone(),
                            include_reason: Some(include_reason),
                            ..Default::default()
                        },
                        Some(lib_file),
                    );
                } else {
                    self.processing_diagnostics
                        .push(new_unknown_reference_processing_diagnostic(include_reason));
                }
            }
        }

        loader.resolve_imports_and_module_augmentations(self, taken);
        // tsgo#4712
        for supplemental in file.supplemental_source_files() {
            self.sub_tasks.push(Rc::new(RefCell::new(ParseTask {
                normalized_file_path: supplemental.file_name().to_string(),
                // ts#64159 (filesparser.go:176)
                path: loader.to_path(supplemental.file_name()),
                file: Some(supplemental.clone()),
                is_content_mapper_supplemental: true,
                include_reason: Some(new_file_include_reason(
                    FileIncludeKind::CONTENT_MAPPER_SUPPLEMENTAL,
                    FileIncludeData::Path(self.path.clone()),
                )),
                ..Default::default()
            })));
        }
    }

    // Go: filesparser.go:187 (*parseTask).redirect
    pub fn redirect(&mut self, _loader: &FileLoader, file_name: &str) {
        let redirected = Rc::new(RefCell::new(ParseTask {
            normalized_file_path: normalize_path(file_name),
            lib_file: self.lib_file.clone(),
            include_reason: self.include_reason.clone(),
            ..Default::default()
        }));
        self.redirected_parse_task = Some(redirected.clone());
        // increaseDepth and elideOnDepth are not copied to redirects, otherwise their depth would be double counted.
        self.sub_tasks = vec![redirected];
    }

    // Go: filesparser.go:198 (*parseTask).loadAutomaticTypeDirectives
    pub fn load_automatic_type_directives(&mut self, loader: &FileLoader) {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Program,
                "processTypeReferences",
                Vec::new(),
                false,
            )
        });
        let (to_parse_type_refs, type_resolutions_in_file, type_resolutions_trace, p_diagnostics) =
            loader.resolve_automatic_type_directives(&self.normalized_file_path);
        self.type_resolutions_in_file = type_resolutions_in_file;
        self.type_resolutions_trace = type_resolutions_trace;
        self.processing_diagnostics.extend(p_diagnostics);
        for type_resolution in to_parse_type_refs {
            self.add_sub_task(type_resolution, None);
        }
    }

    // Go: filesparser.go:220 (*parseTask).addSubTask
    pub fn add_sub_task(&mut self, mut ref_: ResolvedRef, lib_file: Option<Rc<LibFile>>) {
        // PERF: a resolved name is normal already (Go normalizes it again);
        // then the name moves, with no copy.
        if !is_normalized_path(&ref_.file_name) {
            ref_.file_name = normalize_path(&ref_.file_name);
        }
        self.add_normalized_sub_task(ref_, lib_file, 0);
    }

    /// `add_sub_task` of a ref whose name is normalized already (a parse
    /// worker normalized it, `PrepAdd`). `path_hash`: the hash of its path
    /// that the worker made, or 0 (`ParseTask::path_hash`).
    pub(crate) fn add_normalized_sub_task(
        &mut self,
        ref_: ResolvedRef,
        lib_file: Option<Rc<LibFile>>,
        path_hash: u64,
    ) {
        let sub_task = Rc::new(RefCell::new(ParseTask {
            normalized_file_path: ref_.file_name,
            path: ref_.path,
            lib_file,
            increase_depth: ref_.increase_depth,
            elide_on_depth: ref_.elide_on_depth,
            include_reason: ref_.include_reason,
            package_id: ref_.package_id,
            path_hash,
            ..Default::default()
        }));
        self.sub_tasks.push(sub_task);
    }
}

// Go: filesparser.go:211 resolvedRef
#[derive(Clone, Default)]
pub struct ResolvedRef {
    pub file_name: String,
    /// ts#64159 (Go N' `resolvedRef.path`): the path of `file_name`, from
    /// the resolution. Empty: `FilesParser::start` makes it.
    pub path: Path,
    pub increase_depth: bool,
    pub elide_on_depth: bool,
    pub include_reason: Option<Rc<FileIncludeReason>>,
    pub package_id: PackageId,
}

/// Sets the path of a task that `FilesParser::start` queues: the one it
/// came with (Go `task.path`, ts#64159), else the loader's `to_path` of its
/// file name.
fn set_task_path(loader: &FileLoader, task: &mut ParseTask) {
    if task.path.is_empty() {
        // PERF: Go `loader.toPath` asks the host for its current directory
        // and case sensitivity each time; the loader keeps the same values
        // (`compare_paths_options`), so no copy of the directory is made.
        task.path = to_path(
            &task.normalized_file_path,
            &loader.compare_paths_options.current_directory,
            loader.compare_paths_options.use_case_sensitive_file_names,
        );
    } else {
        debug_assert_eq!(
            task.path,
            to_path(
                &task.normalized_file_path,
                &loader.compare_paths_options.current_directory,
                loader.compare_paths_options.use_case_sensitive_file_names,
            ),
            "path of task {}",
            task.normalized_file_path
        );
    }
}

/// Go `filesParser.taskDataByPath`.
// PERF (loadfree1 C6): hashbrown's map (the map of std) with
// `FxBuildHasher`, as `FxHashMap`: `start` looks a path up with the hash
// that a parse worker made (`ParseTask::path_hash`).
pub type TaskDataByPath = hashbrown::HashMap<Path, Rc<RefCell<ParseTaskData>>, FxBuildHasher>;

/// One queued run of the closure that Go `filesParser.start` passes to
/// `wg.Queue`, with the values it captures.
pub(crate) struct QueuedParseTask {
    task: ParseTaskRef,
    data: Rc<RefCell<ParseTaskData>>,
    loaded: bool,
    depth: i32,
}

// Go: filesparser.go:233 filesParser
// PORT: Go `core.WorkGroup` is single threaded here (contract 10). Go
// `singleThreadedWorkGroup` keeps queued functions in a slice and
// `RunAndWait` pops the last one first, so `queue` is a stack with the same
// order. The Go `sync.Pool` of `parseTaskData` values is not ported: a new
// value is made only when the path is new, which is the same result.
// PORT: the parses run in parallel, like the Go work group: parse workers
// parse queued files ahead of the loader (`run_prefetch_worker`), and the
// loader takes their results (`take_prefetched`). The loader still
// loads files in the serial order, so store ids, resolution order and file
// order do not change.
pub struct FilesParser {
    pub(crate) queue: Vec<QueuedParseTask>,
    pub task_data_by_path: TaskDataByPath,
    pub max_depth: i32,
    /// Go `singleThreaded`: no parse workers.
    pub single_threaded: bool,
    /// The lookups that the loader and the parse workers of `parse` shared
    /// (`HostStatCache::Load`), until `get_processed_files` moves them to
    /// the program.
    pub(crate) load_stats: Cell<Option<HostStatCache>>,
}

// PORT: Go's garbage collector frees the parse tasks with the loader. Here
// a loaded task holds its sub tasks, and a sub task of a file that was
// queued before holds the task that loaded the file (`loaded_task`). When
// files import each other, these links make an `Rc` cycle that is never
// freed. Every task that has links is in `task_data_by_path` (only those
// tasks load), so taking their links out lets the whole graph go
// (`ParseTaskLinks`). On the dispatch thread of the LSP server that free
// waits until the answer is sent (`gostd::local::drop_later`). A
// one-program process does not drop the parser
// (`with_loader_state_forgotten`).
impl Drop for FilesParser {
    fn drop(&mut self) {
        crate::gostd::local::drop_later(Box::new(ParseTaskLinks(std::mem::take(
            &mut self.task_data_by_path,
        ))));
    }
}

/// The parse tasks of a dropped `FilesParser`. The drop takes their links
/// out, which frees the task graph (see the `FilesParser` drop).
struct ParseTaskLinks(TaskDataByPath);

impl Drop for ParseTaskLinks {
    fn drop(&mut self) {
        for data in self.0.values() {
            for task in data.borrow().tasks.values() {
                let mut task = task.borrow_mut();
                task.sub_tasks = Vec::new();
                task.loaded_task = None;
                task.redirected_parse_task = None;
            }
        }
    }
}

// Go: filesparser.go:247 getParseTaskData
// PORT: Go takes the value from `parseTaskDataPool`; `putParseTaskData`
// (filesparser.go:226) returns an unused one. No pool is needed here.
fn get_parse_task_data(task: &ParseTaskRef) -> Rc<RefCell<ParseTaskData>> {
    let mut tasks = IndexMap::with_capacity(1);
    tasks.insert(task.borrow().normalized_file_path.clone(), task.clone());
    Rc::new(RefCell::new(ParseTaskData {
        tasks,
        // PORT: Go `math.MaxInt`. Depths are small, so `i32::MAX` gives the same comparisons.
        lowest_depth: i32::MAX,
        started_sub_tasks: false,
        package_id: PackageId::default(),
    }))
}

// Go: filesparser.go:259 parseTaskData
// PORT: Go iterates `tasks` (a Go map) in random order. `IndexMap` keeps
// insertion order. The map holds more than one task only when one path is
// reached through file names that differ in casing.
pub struct ParseTaskData {
    // map of tasks by file casing
    pub tasks: IndexMap<String, ParseTaskRef>,
    pub lowest_depth: i32,
    pub started_sub_tasks: bool,
    pub package_id: PackageId,
}

/// True when `cached` has the file of every root task, and there is one,
/// for the parse options that the workers guess (`FileRefs::fits`). The
/// task of the automatic type directives has no file.
// PORT: not in Go (parse workers, `FilesParser::parse`).
fn all_roots_cached(
    loader: &FileLoader,
    tasks: &[ParseTaskRef],
    cached: &FxHashMap<String, Arc<FileRefs>>,
) -> bool {
    let options = loader.opts.config.compiler_options();
    let mut files = tasks
        .iter()
        .map(|task| task.borrow())
        .filter(|task| !task.is_for_automatic_type_directive)
        .peekable();
    files.peek().is_some()
        && files.all(|task| {
            let file_name = &task.normalized_file_path;
            cached.get(file_name).is_some_and(|refs| {
                refs.fits(|| {
                    get_external_module_indicator_options(
                        file_name,
                        &options,
                        &SourceFileMetaData::default(),
                    )
                })
            })
        })
}

impl FilesParser {
    // Go: filesparser.go:268 (*filesParser).parse
    pub fn parse(&mut self, loader: &FileLoader, tasks: &[ParseTaskRef]) {
        if PREFETCH.with(|p| p.borrow().is_some()) {
            self.run(loader, tasks);
            return;
        }
        // A large program gets more parse workers (below) and bind threads.
        let large = crate::program::note_program_load(tasks.len());
        // Workers that `start_default_lib_prefetch` started for this load.
        let mut early = EARLY_POOL.with(|early| early.borrow_mut().take());
        // The worker parses of these paths are freeable file versions
        // (`PrefetchQueue::freeable`). An early pool queued its jobs before
        // this set existed, so its parses would be static: it goes.
        let freeable = if loader.host.freeable_worker_parses() {
            crate::ast::published_paths()
        } else {
            Arc::default()
        };
        if !freeable.is_empty()
            && let Some(early) = early.take()
        {
            early.discard();
        }
        // A host with its own file cache (`CompilerHost::prefetch_parses`)
        // gets no workers. This does not set `single_threaded`, so the
        // queue order stays that of a parallel load.
        let mut workers = if self.single_threaded || !loader.host.prefetch_parses() {
            0
        } else {
            prefetch_worker_count()
        };
        // PERF: a later `tsc -b` program gets its shared `.d.ts` and `.json`
        // files from the build host's cache, and a language server project
        // that is made again gets most files from the parse cache. A worker
        // parse of such a file is not used: it takes CPU, the loader waits
        // for running parses at the end, and its nodes stay in the worker's
        // AST arena. The workers still resolve their references, as Go parse
        // tasks do for a file that the host gives from its cache.
        let cached = if workers == 0 {
            FxHashMap::default()
        } else {
            loader.host.cached_source_file_refs()
        };
        // When the host gives every root file from its cache, no worker
        // parse can be used, so no worker starts. The loader parses the
        // other files itself, as a load with no workers.
        if !cached.is_empty() && all_roots_cached(loader, tasks, &cached) {
            CACHED_LOADS.fetch_add(1, AtomicOrdering::Relaxed);
            workers = 0;
        }
        if workers == 0 {
            if let Some(early) = early {
                early.discard();
            }
            self.run(loader, tasks);
            return;
        }
        POOL_LOADS.fetch_add(1, AtomicOrdering::Relaxed);
        let config = PrefetchConfig::of_loader(loader);
        // The workers use the cache of the loader's file system
        // (`SharedStatCache`). Only on the plain OS file system: another
        // file system caches other files than the workers read. It is set
        // before the resolve config, so a worker's resolver sees it
        // (`WorkerResolver::new`).
        let build_host_cache = loader
            .host
            .stat_cache()
            .filter(|_| loader.host.is_plain_os_fs());
        let resolve = WorkerResolveConfig::of_loader(loader);
        // An early pool whose workers resolve already (its root jobs) fits
        // only with the same config, and with no host cache, which they
        // did not see.
        let fits = |pool: &PrefetchPool| {
            pool.shared.config == config
                && pool.shared.resolve.get().is_none_or(|early| {
                    build_host_cache.is_none()
                        && match (early, &resolve) {
                            (Some(early), Some(resolve)) => early.same(resolve),
                            (early, resolve) => early.is_none() && resolve.is_none(),
                        }
                })
        };
        let mut pool = match early {
            Some(pool) if fits(&pool) => pool,
            other => {
                if let Some(other) = other {
                    other.discard();
                }
                PrefetchPool::start(config, workers)
            }
        };
        pool.shared
            .count(|c| c.loader_started = Some(std::time::Instant::now()));
        if let Some(cache) = build_host_cache {
            cache.start_load();
            let _ = pool.shared.stats.host.set(cache);
        }
        // An early pool with root jobs has this config already.
        let _ = pool.shared.resolve.set(resolve);
        {
            let mut queue = lock(&pool.shared.queue);
            queue.cached = cached;
            queue.freeable = freeable;
        }
        // A program that does not use the sources of its references loads
        // their output `.d.ts` files in place of the sources (Go
        // `getParseFileRedirect`), so the workers parse those.
        if !loader.opts.can_use_project_reference_source() {
            let redirects: FxHashMap<String, (String, Path)> = loader
                .project_reference_file_mapper
                .borrow()
                .source_to_project_reference
                .values()
                .filter(|reference| !reference.output_dts.is_empty())
                .map(|reference| {
                    let output = reference.output_dts.clone();
                    let path = loader.to_path(&output);
                    (reference.source.clone(), (output, path))
                })
                .collect();
            pool.shared
                .has_redirects
                .store(!redirects.is_empty(), AtomicOrdering::Relaxed);
            lock(&pool.shared.queue).redirects = redirects;
        }
        let prefetch = PrefetchGuard::install(pool.shared.clone());
        self.start(loader, tasks, 0);
        pool.shared
            .rank_roots(tasks, ROOT_RANK_PER_WORKER * pool.threads.len());
        // The root jobs are queued now, so the added workers start at once.
        pool.add_workers(extra_worker_count(large));
        self.run_queue(loader, Some(&mut pool));
        // Closes the queue, then waits for the workers.
        drop(prefetch);
        let shared = Arc::clone(&pool.shared);
        drop(pool);
        // The program keeps the lookups of the plain OS host's load for the
        // thread that finishes its symlink cache (`HostStatCache`). A `tsc
        // -b` host keeps its own (`CompilerHost::stat_cache`).
        if loader.host.is_plain_os_fs() && shared.stats.host.get().is_none() {
            self.load_stats
                .set(Some(HostStatCache::Load(Arc::clone(&shared.stats))));
        }
        // PERF: the workers' state (the resolution cache, the jobs) is freed
        // beside the rest of the load, as Go's GC frees it; this thread has
        // the last reference now. Not in Go. The free thread is one per
        // process (`goport-free`): the first load with workers starts it,
        // also in the LSP and API servers, and it then waits on its queue
        // for the frees of later loads and builds. The last batch of the
        // loader's `WorkerGarbage` goes with it.
        crate::execute::build::build_task::drop_in_background((shared, take_worker_garbage()));
    }

    /// Go `parse` without the workers: queue the root tasks and run the
    /// queue until it is empty.
    fn run(&mut self, loader: &FileLoader, tasks: &[ParseTaskRef]) {
        self.start(loader, tasks, 0);
        self.run_queue(loader, None);
    }

    // Go: core/workgroup.go singleThreadedWorkGroup.RunAndWait
    // PORT: Go runs each queued func through `core.WorkGroup.Queue`, on its
    // own goroutine unless single threaded. The port runs them here in
    // queue order. With `go_work_group_task`, a Go panic in a queued func
    // ends the run as it does in Go.
    // PORT: with the parse workers (`pool`), a run that opened a mapper
    // project queues the content-mapped jobs (`queue_mapped_prefetch`).
    fn run_queue(&mut self, loader: &FileLoader, mut pool: Option<&mut PrefetchPool>) {
        while let Some(queued) = self.queue.pop() {
            if self.single_threaded {
                self.run_queued(loader, queued);
            } else {
                crate::core::go_work_group_task(|| self.run_queued(loader, queued));
            }
            if let Some(pool) = pool.as_deref_mut()
                && loader.mapped_prefetch_ready.replace(false)
            {
                self.queue_mapped_prefetch(loader, pool);
            }
        }
    }

    /// Queues the worker jobs of the content-mapped files of the queued
    /// tasks once the loader's transform of a first file opened their
    /// mapper project (`FileLoader::note_content_mapper_transform`), and
    /// starts the workers of such jobs the first time. Later tasks get
    /// their jobs when they are queued (`start`), so the workers start
    /// also when no mapped task waits yet (the mapped files that later
    /// imports reach).
    // PORT: not in Go (see `PrefetchJob::mapped`).
    fn queue_mapped_prefetch(&self, loader: &FileLoader, pool: &mut PrefetchPool) {
        let requests: Vec<PrefetchRequest> = self
            .queue
            .iter()
            .filter(|queued| !queued.loaded)
            .filter_map(|queued| {
                let task = queued.task.borrow();
                self.prefetch_request(loader, &task, Some(task.path.clone()), queued.depth)
            })
            .filter(|request| matches!(request, PrefetchRequest::Mapped(..)))
            .collect();
        pool.shared.queue_batch(requests);
        if !pool.mapped_workers {
            pool.mapped_workers = true;
            pool.add_mapped_workers(mapped_worker_count());
        }
    }

    // Go: filesparser.go:273 (*filesParser).start
    pub fn start(&mut self, loader: &FileLoader, tasks: &[ParseTaskRef], depth: i32) {
        let prefetch = PREFETCH.with(|p| p.borrow().clone());
        let mut requests = Vec::new();
        for task in tasks {
            // ts#64159 (filesparser.go:275): Go panics on a task without a
            // path. PORT: the tasks that Go gives a path when it makes them
            // and the port does not (root, lib, automatic type directive
            // and redirect tasks) get it here.
            set_task_path(loader, &mut task.borrow_mut());
            // Go `taskDataByPath.LoadOrStore(task.path, ..)`
            // (filesparser.go:279), the load part, with the hash that the
            // worker made when the task has one.
            let existing = {
                let task = task.borrow();
                let hasher = self.task_data_by_path.hasher();
                let hash = match task.path_hash {
                    0 => hasher.hash_one(&task.path),
                    hash => hash,
                };
                debug_assert_eq!(hash, hasher.hash_one(&task.path), "{}", task.path.0);
                self.task_data_by_path
                    .raw_entry()
                    .from_key_hashed_nocheck(hash, &task.path)
                    .map(|(_, data)| data.clone())
            };
            let (data, loaded) = match existing {
                Some(data) => {
                    // No queued task of this path ran yet, so the loader
                    // loads the file later (a job to move up).
                    if prefetch.is_some() && data.borrow().lowest_depth == i32::MAX {
                        requests.extend(self.prefetch_request(loader, &task.borrow(), None, depth));
                    }
                    (data.clone(), true)
                }
                None => {
                    let candidate = get_parse_task_data(task);
                    let path = task.borrow().path.clone();
                    self.task_data_by_path
                        .insert(path.clone(), candidate.clone());
                    if prefetch.is_some() {
                        requests.extend(self.prefetch_request(
                            loader,
                            &task.borrow(),
                            Some(path),
                            depth,
                        ));
                    }
                    (candidate, false)
                }
            };
            task.borrow_mut().data = Rc::downgrade(&data);

            self.queue.push(QueuedParseTask {
                task: task.clone(),
                data,
                loaded,
                depth,
            });
        }
        if let Some(prefetch) = prefetch {
            prefetch.queue_batch(requests);
        }
    }

    /// The parse of a task's file that the parse workers can do ahead of
    /// the loader, when the queued run of the task will probably load it
    /// (`ParseTask::load`). `path`: the task is the first of its path, so
    /// the file may need a new job; `None`: an earlier task queued the
    /// path, so only a job that exists can move up. A wrong guess only
    /// costs worker time: the loader uses a worker parse only when it gives
    /// the same result (`take_prefetched`).
    fn prefetch_request(
        &self,
        loader: &FileLoader,
        task: &ParseTask,
        path: Option<Path>,
        depth: i32,
    ) -> Option<PrefetchRequest> {
        let current_depth = if task.increase_depth {
            depth + 1
        } else {
            depth
        };
        // tsgo#4712: the loader never parses a content mapper supplemental
        // file (it comes with its task).
        if task.is_for_automatic_type_directive
            || task.loaded
            || task.is_content_mapper_supplemental
            || task.elide_on_depth && current_depth > self.max_depth
        {
            return None;
        }
        let file_name = &task.normalized_file_path;
        // tsgo#4712: the host transforms a content-mapped file. PORT: a
        // worker sends its transform and parses the virtual text ahead,
        // once the loader's transform of a file of the mapper opened the
        // mapper project (`FileLoader::concurrent_content_mapper_transform`);
        // the host takes the result (`take_prefetched_mapped`).
        let mapped = if !loader.content_mapper_extensions.is_empty()
            && file_extension_is_one_of(
                file_name,
                &loader
                    .content_mapper_extensions
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            ) {
            Some(loader.concurrent_content_mapper_transform(file_name)?)
        } else {
            None
        };
        let Some(path) = path else {
            return Some(PrefetchRequest::Known(file_name.clone()));
        };
        if let Some(transform) = mapped {
            // PORT: the metadata (package.json scope) is not known yet, as
            // below; a parse that read other options is not used.
            let external_module_indicator_options = get_external_module_indicator_options(
                file_name,
                &loader.opts.config.compiler_options(),
                &SourceFileMetaData::default(),
            );
            return Some(PrefetchRequest::Mapped(
                SourceFileParseOptions {
                    file_name: file_name.clone(),
                    path,
                    external_module_indicator_options,
                },
                transform,
            ));
        }
        let script_kind = get_script_kind_from_file_name(file_name);
        if script_kind == ScriptKind::UNKNOWN
            || !has_extension(file_name)
            || !loader
                .opts
                .config
                .compiler_options()
                .allow_non_ts_extensions
                .is_true()
                && !loader.is_supported_extension(&get_canonical_file_name(
                    file_name,
                    loader.compare_paths_options.use_case_sensitive_file_names,
                ))
        {
            return None;
        }
        // PORT: the metadata (package.json scope) is not known yet. Most
        // parses do not read these options; a parse that read other options
        // than the loader passes is not used.
        let external_module_indicator_options = get_external_module_indicator_options(
            file_name,
            &loader.opts.config.compiler_options(),
            &SourceFileMetaData::default(),
        );
        Some(PrefetchRequest::New(
            SourceFileParseOptions {
                file_name: file_name.clone(),
                path,
                external_module_indicator_options,
            },
            script_kind,
        ))
    }

    /// The body of the closure that Go `start` queues.
    // Go: filesparser.go:273 (*filesParser).start (queued func)
    fn run_queued(&mut self, loader: &FileLoader, queued: QueuedParseTask) {
        let QueuedParseTask {
            task,
            data,
            loaded,
            depth,
        } = queued;

        let mut start_subtasks = false;
        if loaded {
            let existing_task = data
                .borrow()
                .tasks
                .get(&task.borrow().normalized_file_path)
                .cloned();
            if let Some(existing_task) = existing_task {
                // Go: tasks[i].loadedTask = existingTask (tasks[i] is task)
                // PORT: a restart of started subtasks (see below) queues the
                // same task again, so `existing_task` can be `task`. Go then
                // sets `task.loadedTask = task`, which `collectFiles` resolves
                // to the same task. Skip it here so no `Rc` cycle forms.
                if !Rc::ptr_eq(&existing_task, &task) {
                    task.borrow_mut().loaded_task = Some(existing_task);
                }
            } else {
                let name = task.borrow().normalized_file_path.clone();
                let mut d = data.borrow_mut();
                d.tasks.insert(name, task.clone());
                // This is new task for file name - so load subtasks if there was loading for any other casing
                start_subtasks = d.started_sub_tasks;
            }
        }

        {
            let mut d = data.borrow_mut();
            // Propagate packageId to data if we have one and data doesn't yet
            let t = task.borrow();
            if d.package_id.name.is_empty() && !t.package_id.name.is_empty() {
                d.package_id = t.package_id.clone();
            }
        }

        let current_depth = if task.borrow().increase_depth {
            depth + 1
        } else {
            depth
        };
        let lowered;
        {
            let mut d = data.borrow_mut();
            lowered = current_depth < d.lowest_depth;
            if lowered {
                // If we're seeing this task at a lower depth than before,
                // reprocess its subtasks to ensure they are loaded.
                d.lowest_depth = current_depth;
                start_subtasks = true;
                d.started_sub_tasks = true;
            }
        }

        if task.borrow().elide_on_depth && current_depth > self.max_depth {
            return;
        }

        // PORT: Go does not change `data.tasks` in this loop, so an index
        // walks the same tasks. PERF (parload1 step 4): no copy of the list.
        let count = data.borrow().tasks.len();
        for index in 0..count {
            let task_by_file_name = data.borrow().tasks[index].clone();
            let mut load_sub_tasks = start_subtasks;
            if !task_by_file_name.borrow().loaded {
                task_by_file_name.borrow_mut().load(loader);
                if task_by_file_name.borrow().redirected_parse_task.is_some() {
                    // Always load redirected task
                    load_sub_tasks = true;
                    data.borrow_mut().started_sub_tasks = true;
                }
            }
            // ts#64632: a later arrival that lowers the depth starts the
            // subtasks again at the new depth, so each file ends at its
            // lowest depth over all paths, in every run order.
            if load_sub_tasks && (lowered || !task_by_file_name.borrow().started_sub_tasks) {
                task_by_file_name.borrow_mut().started_sub_tasks = true;
                let lowest_depth = data.borrow().lowest_depth;
                // PERF (parload1 step 4): no copy of the list. `start`
                // borrows only the subtasks, which are tasks of their own.
                let parent = task_by_file_name.borrow();
                self.start(loader, &parent.sub_tasks, lowest_depth);
            }
        }
    }

    // Go: filesparser.go:337 (*filesParser).getProcessedFiles
    pub fn get_processed_files(&self, loader: &FileLoader) -> ProcessedFiles {
        let total_file_count = loader.total_file_count.get() as usize;
        let lib_file_count = loader.lib_file_count.get() as usize;

        let mut missing_files: Vec<String> = Vec::new();
        let mut duplicate_source_files: Vec<DuplicateSourceFile> = Vec::new();
        let mut files: Vec<Rc<ParsedSourceFile>> =
            Vec::with_capacity(total_file_count - lib_file_count);
        // totalFileCount here since we append files to it later to construct the final list
        let mut lib_files: Vec<Rc<ParsedSourceFile>> = Vec::with_capacity(total_file_count);

        // PERF: the maps that are only looked up are sized for every loaded
        // file up front, so the serial collect after the parse does not
        // grow them. The maps of the program keep their growth: a map's
        // iteration order depends on its capacity, and some readers
        // iterate them.
        let mut files_by_path: FxHashMap<Path, Rc<ParsedSourceFile>> = FxHashMap::default();
        // stores 'filename -> file association' ignoring case
        // used to track cases when two file names differ only in casing
        let mut tasks_seen_by_name_ignore_case: Option<FxHashMap<String, ParseTaskRef>> =
            if loader.compare_paths_options.use_case_sensitive_file_names {
                Some(FxHashMap::with_capacity_and_hasher(
                    total_file_count,
                    Default::default(),
                ))
            } else {
                None
            };

        // PERF (collect1): sized as Go sizes it (filesparser.go:355). Only
        // tests iterate this map, so its capacity changes no output (see
        // `FileIncludeReasons`).
        let mut include_processor = IncludeProcessor {
            file_include_reasons: Rc::new(FileIncludeReasons::with_capacity(total_file_count)),
            ..IncludeProcessor::default()
        };
        let mut output_file_to_project_reference_source: Option<FxHashMap<Path, String>> =
            if !loader.opts.can_use_project_reference_source() {
                Some(FxHashMap::default())
            } else {
                None
            };
        let mut type_resolutions_in_file: FxHashMap<
            Path,
            ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>,
        > = FxHashMap::default();
        let mut jsx_runtime_import_specifiers: Option<
            FxHashMap<Path, Rc<JsxRuntimeImportSpecifier>>,
        > = None;
        let mut import_helpers_import_specifiers: Option<FxHashMap<Path, Node>> = None;
        let mut lib_files_map: FxHashMap<Path, Rc<LibFile>> =
            FxHashMap::with_capacity_and_hasher(lib_file_count, Default::default());

        let mut redirect_targets_map: Option<FxHashMap<Path, Vec<String>>> = None;
        let mut redirect_files_by_path: Option<FxHashMap<Path, RedirectsFile>> = None;
        let mut package_id_to_source_file: Option<FxHashMap<PackageId, Rc<ParsedSourceFile>>> =
            None;
        if !loader
            .opts
            .config
            .compiler_options()
            .deduplicate_packages
            .is_false()
        {
            redirect_targets_map = Some(FxHashMap::default());
            package_id_to_source_file = Some(FxHashMap::default());
        }

        /// The walk's state of one task data (one path).
        // PERF (collect1): one slot holds Go's `seen[data]` and, until the
        // walk ends, the `fileIncludeReasons` entry of the data's path. So a
        // visit makes one lookup by pointer and none by path (Go makes one by
        // pointer and two by path).
        struct WalkSlot {
            /// Go `seen[data]`: the task whose name Go stores, not a copy of
            /// the name. A task gets its `normalized_file_path` when it is
            /// made and keeps it, so this task gives Go's `checkedName`.
            seen: Option<ParseTaskRef>,
            /// The first task that got a reason: its path is the key of
            /// `reasons` in `fileIncludeReasons`. Every task of one data has
            /// the data's path.
            owner: Option<ParseTaskRef>,
            /// Go `fileIncludeReasons[path]`, in the order of the walk.
            reasons: Vec<Rc<FileIncludeReason>>,
        }

        // PORT: the Go closure `collectFiles` is an explicit recursive
        // function over a struct that holds its captured variables.
        struct Collector<'a> {
            loader: &'a FileLoader,
            // PORT: Go `seen map[*parseTaskData]string` is keyed by pointer.
            // The key here is the `Rc` pointer of the task data, and the
            // value the index of its slot in `slots` (slots in the order
            // they are made).
            slot_index: FxHashMap<*const RefCell<ParseTaskData>, u32>,
            slots: Vec<WalkSlot>,
            missing_files: &'a mut Vec<String>,
            duplicate_source_files: &'a mut Vec<DuplicateSourceFile>,
            files: &'a mut Vec<Rc<ParsedSourceFile>>,
            lib_files: &'a mut Vec<Rc<ParsedSourceFile>>,
            files_by_path: &'a mut FxHashMap<Path, Rc<ParsedSourceFile>>,
            tasks_seen_by_name_ignore_case: &'a mut Option<FxHashMap<String, ParseTaskRef>>,
            include_processor: &'a mut IncludeProcessor,
            output_file_to_project_reference_source: &'a mut Option<FxHashMap<Path, String>>,
            file_maps: &'a mut FileMapSink,
            type_resolutions_in_file:
                &'a mut FxHashMap<Path, ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>>,
            jsx_runtime_import_specifiers:
                &'a mut Option<FxHashMap<Path, Rc<JsxRuntimeImportSpecifier>>>,
            import_helpers_import_specifiers: &'a mut Option<FxHashMap<Path, Node>>,
            lib_files_map: &'a mut FxHashMap<Path, Rc<LibFile>>,
            redirect_targets_map: &'a mut Option<FxHashMap<Path, Vec<String>>>,
            redirect_files_by_path: &'a mut Option<FxHashMap<Path, RedirectsFile>>,
            package_id_to_source_file: &'a mut Option<FxHashMap<PackageId, Rc<ParsedSourceFile>>>,
            // recordedDuplicates tracks, per task data, the set of file-name casings that
            // have already been recorded in duplicateSourceFiles. A file that is reached
            // from multiple import sites is walked once per site, but each distinct casing
            // is only parsed and acquired in the parse cache once. Recording the same casing
            // as a duplicate more than once would cause it to be released more times than it
            // was acquired when the snapshot is disposed, leaving a dangling cache entry that
            // panics the next time it is referenced.
            //
            // PORT: Go `map[*parseTaskData]*collections.Set[string]`, made on first use.
            // The key is the `Rc` pointer of the task data, as for `seen`.
            recorded_duplicates: FxHashMap<*const RefCell<ParseTaskData>, FxHashSet<String>>,
            total_file_count: usize,
        }

        impl Collector<'_> {
            /// The slot key of loaded task `t`: the pointer of its task
            /// data, Go's `seen` key. Only a task that `FilesParser::start`
            /// queued is loaded, and `start` sets its link
            /// (`ParseTask::data`) to the data of its path, which
            /// `task_data_by_path` keeps while the parser lives.
            // PERF (collect1): the link's pointer, with no upgrade and no
            // lookup by path (Go looks the data up by path).
            fn data_key(t: &ParseTask) -> *const RefCell<ParseTaskData> {
                t.data.as_ptr()
            }

            /// The index of the slot of task data `key`, made on first use.
            fn slot_of(&mut self, key: *const RefCell<ParseTaskData>) -> usize {
                let next = self.slots.len();
                let index = *self.slot_index.entry(key).or_insert(next as u32) as usize;
                if index == next {
                    self.slots.push(WalkSlot {
                        seen: None,
                        owner: None,
                        reasons: Vec::new(),
                    });
                }
                index
            }

            /// Adds `reason` to the reasons of slot `index`, whose data is
            /// the data of `task`.
            fn push_reason(
                &mut self,
                index: usize,
                task: &ParseTaskRef,
                reason: &Rc<FileIncludeReason>,
            ) {
                let slot = &mut self.slots[index];
                slot.owner.get_or_insert_with(|| task.clone());
                slot.reasons.push(reason.clone());
            }

            // Go: filesparser.go:594 (*filesParser).addIncludeReason
            // PORT: Go can append a nil reason. Only the automatic type
            // directive root task has no reason, and `collectFiles` never
            // passes it here, so a nil reason is skipped. The reason goes to
            // the slot of the task's data (`WalkSlot::reasons`).
            fn add_include_reason(
                &mut self,
                task: &ParseTaskRef,
                reason: Option<&Rc<FileIncludeReason>>,
            ) {
                let t = task.borrow();
                if let Some(redirected) = &t.redirected_parse_task {
                    self.add_include_reason(redirected, reason);
                } else if t.loaded
                    && let Some(reason) = reason
                {
                    let index = self.slot_of(Self::data_key(&t));
                    self.push_reason(index, task, reason);
                }
            }

            /// Puts the reasons of the slot of task data `key` into
            /// `fileIncludeReasons` now, for a reader during the walk (the
            /// casing check, includeprocessor.go:65). `finish` puts them
            /// again at the end.
            fn sync_reasons(&mut self, key: *const RefCell<ParseTaskData>) {
                let Some(&index) = self.slot_index.get(&key) else {
                    return;
                };
                let slot = &self.slots[index as usize];
                if let Some(owner) = &slot.owner {
                    Rc::make_mut(&mut self.include_processor.file_include_reasons)
                        .insert(owner.borrow().path.clone(), slot.reasons.clone());
                }
            }

            /// Go's `fileIncludeReasons` after the walk: the reasons of each
            /// slot under its data's path.
            fn finish(self) {
                // The map is not shared yet, so `make_mut` copies nothing.
                let reasons = Rc::make_mut(&mut self.include_processor.file_include_reasons);
                for slot in self.slots {
                    if let Some(owner) = slot.owner {
                        reasons.insert(owner.borrow().path.clone(), slot.reasons);
                    }
                }
            }

            // Go: filesparser.go:386 collectFiles
            fn collect_files(&mut self, tasks: &[ParseTaskRef]) {
                let loader = self.loader;
                // PERF (collect1): the task of the list, its loaded task and
                // its include reason are borrowed, not cloned (Go copies
                // pointers). The reason is cloned once, when it is stored.
                for listed in tasks {
                    // Exclude automatic type directive tasks from include reason processing,
                    // as these are internal implementation details and should not contribute
                    // to the reasons for including files.
                    let (loaded_task, add_reason) = {
                        let l = listed.borrow();
                        if l.redirected_parse_task.is_none() && !l.is_for_automatic_type_directive {
                            (l.loaded_task.clone(), true)
                        } else {
                            (None, false)
                        }
                    };
                    let task = loaded_task.as_ref().unwrap_or(listed);
                    let (loaded, redirected) = {
                        let t = task.borrow();
                        (t.loaded, t.redirected_parse_task.is_some())
                    };
                    // Go: `addIncludeReason` before the `loaded` check. A
                    // redirect gives the reason to its target here. Else
                    // only a loaded task gets it, below, through the slot
                    // that the visit looks up anyway.
                    if add_reason && redirected {
                        self.add_include_reason(task, listed.borrow().include_reason.as_ref());
                    }
                    if !loaded {
                        continue;
                    }
                    let data_key = Self::data_key(&task.borrow());
                    let index = self.slot_of(data_key);
                    if add_reason
                        && !redirected
                        && let Some(reason) = &listed.borrow().include_reason
                    {
                        self.push_reason(index, task, reason);
                    }

                    // ensure we only walk each task once
                    // PERF (collect1): a visit of the stored task (most
                    // visits: Go's `loadedTask`) has the stored name, so Go's
                    // name check below is false and is skipped.
                    let slot = &mut self.slots[index];
                    let checked = match &slot.seen {
                        Some(seen) if Rc::ptr_eq(seen, task) => continue,
                        Some(seen) => Some(seen.clone()),
                        None => {
                            slot.seen = Some(task.clone());
                            None
                        }
                    };
                    if let Some(checked) = checked {
                        let checked = checked.borrow();
                        let t = task.borrow();
                        let checked_name = &checked.normalized_file_path;
                        let normalized_file_path = &t.normalized_file_path;
                        if checked_name != normalized_file_path {
                            if let Some(file) = &t.file
                                && self
                                    .recorded_duplicates
                                    .entry(data_key)
                                    .or_default()
                                    .insert(normalized_file_path.clone())
                            {
                                self.duplicate_source_files.push(DuplicateSourceFile {
                                    parse_options: file.parse_options().clone(),
                                    content_mapper_parse_options: file
                                        .content_mapper_parse_options()
                                        .clone(),
                                    text: file.text.clone(),
                                    hash: file.hash.get(),
                                    script_kind: file.script_kind,
                                    content_mapper: file.content_mapper().to_string(),
                                    is_content_mapper_failure_stub: file
                                        .is_content_mapper_failure_stub(),
                                });
                            }
                            // PORT: equal names give equal absolute paths, so
                            // the check below only runs for different names.
                            if !loader
                                .opts
                                .config
                                .compiler_options()
                                .force_consistent_casing_in_file_names
                                .is_false()
                            {
                                // Check if it differs only in drive letters its ok to ignore that error:
                                let checked_absolute_path =
                                    get_normalized_absolute_path_without_root(
                                        checked_name,
                                        &loader.compare_paths_options.current_directory,
                                    );
                                let input_absolute_path = get_normalized_absolute_path_without_root(
                                    normalized_file_path,
                                    &loader.compare_paths_options.current_directory,
                                );
                                if checked_absolute_path != input_absolute_path {
                                    self.sync_reasons(data_key);
                                    self.include_processor
                                        .add_processing_diagnostics_for_file_casing(
                                            &t.path,
                                            checked_name,
                                            normalized_file_path,
                                            listed
                                                .borrow()
                                                .include_reason
                                                .clone()
                                                .expect("nil pointer dereference: includeReason"),
                                        );
                                }
                            }
                        }
                        continue;
                    }

                    // PERF (collect1): the first visit reads the task's name,
                    // path and package id in place; only the values that a
                    // map or a list keeps are copied.
                    let t = task.borrow();
                    if let Some(tasks_seen_by_name_ignore_case) =
                        self.tasks_seen_by_name_ignore_case.as_mut()
                    {
                        let path_lower_case = to_file_name_lower_case(&t.path);
                        // PERF (collect1): one lookup (Go: a get, then a set).
                        let first = match tasks_seen_by_name_ignore_case.entry(path_lower_case) {
                            std::collections::hash_map::Entry::Occupied(entry) => {
                                Some(entry.get().clone())
                            }
                            std::collections::hash_map::Entry::Vacant(entry) => {
                                entry.insert(task.clone());
                                None
                            }
                        };
                        if let Some(first) = first {
                            let other = first.borrow();
                            self.sync_reasons(Self::data_key(&other));
                            self.include_processor
                                .add_processing_diagnostics_for_file_casing(
                                    &other.path,
                                    &other.normalized_file_path,
                                    &t.normalized_file_path,
                                    listed
                                        .borrow()
                                        .include_reason
                                        .clone()
                                        .expect("nil pointer dereference: includeReason"),
                                );
                        }
                    }

                    for trace in &t.type_resolutions_trace {
                        loader.host.trace(trace.message, trace.args.clone());
                    }
                    for trace in &t.resolutions_trace {
                        loader.host.trace(trace.message, trace.args.clone());
                    }

                    let file = t.file.clone();
                    // PERF (parload1 step 4): the task's link to its data
                    // (`ParseTask::data`, see `data_key`), not a lookup by
                    // path.
                    let data = t
                        .data
                        .upgrade()
                        .expect("loaded parse task without task data");
                    let data = data.borrow();
                    let data_lowest_depth = data.lowest_depth;
                    if let Some(package_id_to_source_file) = self.package_id_to_source_file.as_mut()
                        && !data.package_id.name.is_empty()
                    {
                        if let Some(package_id_file) =
                            package_id_to_source_file.get(&data.package_id).cloned()
                        {
                            if let Some(file) = &file {
                                // Package deduplication keeps the first package instance in the
                                // program, but we still parsed this file and acquired it through
                                // the host, so snapshot disposal must release that extra owner.
                                self.duplicate_source_files.push(DuplicateSourceFile {
                                    parse_options: file.parse_options().clone(),
                                    content_mapper_parse_options: file
                                        .content_mapper_parse_options()
                                        .clone(),
                                    text: file.text.clone(),
                                    hash: file.hash.get(),
                                    script_kind: file.script_kind,
                                    content_mapper: file.content_mapper().to_string(),
                                    is_content_mapper_failure_stub: file
                                        .is_content_mapper_failure_stub(),
                                });
                            }
                            self.redirect_targets_map
                                .as_mut()
                                .expect("redirectTargetsMap is set with packageIdToSourceFile")
                                .entry(package_id_file.path().clone())
                                .or_default()
                                .push(t.normalized_file_path.clone());
                            let redirect_files_by_path =
                                self.redirect_files_by_path.get_or_insert_with(|| {
                                    FxHashMap::with_capacity_and_hasher(
                                        self.total_file_count,
                                        Default::default(),
                                    )
                                });
                            let index = (self.files.len() + redirect_files_by_path.len()) as i32;
                            redirect_files_by_path.insert(
                                t.path.clone(),
                                RedirectsFile {
                                    index,
                                    file_name: t.normalized_file_path.clone(),
                                    path: t.path.clone(),
                                    target: package_id_file.path().clone(),
                                },
                            );
                            self.files_by_path.insert(t.path.clone(), package_id_file);
                            if data_lowest_depth > 0 {
                                self.file_maps.add(FileMapEntry::Found(t.path.clone()));
                            }
                            continue;
                        } else if let Some(file) = &file {
                            package_id_to_source_file.insert(data.package_id.clone(), file.clone());
                        }
                    }
                    drop(data);

                    // PERF: walks the subtasks in place (no copy of the
                    // list). Collecting only reads tasks.
                    if !t.sub_tasks.is_empty() {
                        self.collect_files(&t.sub_tasks);
                    }
                    drop(t);

                    // PERF: the loader does not read a task after its file
                    // is collected, so the per-file maps move out of it
                    // instead of being copied (Go stores the map values).
                    // Each task data is walked once (`seen`), so no outer
                    // walk borrows this task now.
                    let mut t = task.borrow_mut();
                    // Exclude automatic type directive tasks from include reason processing,
                    // as these are internal implementation details and should not contribute
                    // to the reasons for including files.
                    if let Some(redirected) = &t.redirected_parse_task {
                        if !loader.opts.can_use_project_reference_source() {
                            self.output_file_to_project_reference_source
                                .as_mut()
                                .expect("outputFileToProjectReferenceSource is set when project reference source is not used")
                                .insert(redirected.borrow().path.clone(), t.file_name());
                        }
                        continue;
                    }

                    if t.is_for_automatic_type_directive {
                        let type_resolutions = std::mem::take(&mut t.type_resolutions_in_file);
                        self.type_resolutions_in_file
                            .insert(t.path.clone(), type_resolutions);
                        if !t.processing_diagnostics.is_empty() {
                            let diagnostics = std::mem::take(&mut t.processing_diagnostics);
                            self.include_processor
                                .processing_diagnostics
                                .extend(diagnostics);
                        }
                        continue;
                    }

                    let path = t.path.clone();

                    if !t.processing_diagnostics.is_empty() {
                        let diagnostics = std::mem::take(&mut t.processing_diagnostics);
                        self.include_processor
                            .processing_diagnostics
                            .extend(diagnostics);
                    }

                    let Some(file) = file else {
                        self.missing_files.push(t.normalized_file_path.clone());
                        continue;
                    };

                    if let Some(lib_file) = &t.lib_file {
                        self.lib_files.push(file.clone());
                        self.lib_files_map.insert(path.clone(), lib_file.clone());
                    } else {
                        self.files.push(file.clone());
                    }
                    self.files_by_path.insert(path.clone(), file);
                    let resolutions = std::mem::take(&mut t.resolutions_in_file);
                    self.type_resolutions_in_file.insert(
                        path.clone(),
                        std::mem::take(&mut t.type_resolutions_in_file),
                    );
                    let meta = std::mem::take(&mut t.metadata);

                    if let Some(jsx_runtime_import_specifier) = &t.jsx_runtime_import_specifier {
                        self.jsx_runtime_import_specifiers
                            .get_or_insert_with(FxHashMap::default)
                            .insert(path.clone(), jsx_runtime_import_specifier.clone());
                    }
                    if t.import_helpers_import_specifier.is_some() {
                        self.import_helpers_import_specifiers
                            .get_or_insert_with(FxHashMap::default)
                            .insert(path.clone(), t.import_helpers_import_specifier);
                    }
                    self.file_maps.add(FileMapEntry::File {
                        path,
                        resolutions,
                        meta,
                        found_searching_node_modules: data_lowest_depth > 0,
                    });
                }
            }
        }

        let FileMaps {
            mut resolved_modules,
            source_file_meta_datas,
            source_files_found_searching_node_modules,
        } = FileMaps::collect(self.single_threaded, |file_maps| {
            let mut collector = Collector {
                loader,
                slot_index: FxHashMap::with_capacity_and_hasher(
                    self.task_data_by_path.len(),
                    Default::default(),
                ),
                slots: Vec::with_capacity(self.task_data_by_path.len()),
                missing_files: &mut missing_files,
                duplicate_source_files: &mut duplicate_source_files,
                files: &mut files,
                lib_files: &mut lib_files,
                files_by_path: &mut files_by_path,
                tasks_seen_by_name_ignore_case: &mut tasks_seen_by_name_ignore_case,
                include_processor: &mut include_processor,
                output_file_to_project_reference_source:
                    &mut output_file_to_project_reference_source,
                file_maps,
                type_resolutions_in_file: &mut type_resolutions_in_file,
                jsx_runtime_import_specifiers: &mut jsx_runtime_import_specifiers,
                import_helpers_import_specifiers: &mut import_helpers_import_specifiers,
                lib_files_map: &mut lib_files_map,
                redirect_targets_map: &mut redirect_targets_map,
                redirect_files_by_path: &mut redirect_files_by_path,
                package_id_to_source_file: &mut package_id_to_source_file,
                recorded_duplicates: FxHashMap::default(),
                total_file_count,
            };
            collector.collect_files(&loader.root_tasks);
            collector.finish();
        });
        loader.sort_libs(&mut lib_files);

        let lib_files_len = lib_files.len() as i32;
        let mut all_files = lib_files;
        all_files.extend(files);
        if let Some(redirect_files_by_path) = redirect_files_by_path.as_mut() {
            for redirect_file in redirect_files_by_path.values_mut() {
                redirect_file.index += lib_files_len;
            }
        }

        let mut keys: Vec<Path> = loader
            .path_for_lib_file_resolutions
            .borrow()
            .keys()
            .cloned()
            .collect();
        // PORT: Go sorts by the bytes of the paths (see `compare_go_bytes`).
        stable_sort_by(&mut keys, |a, b| compare_go_bytes(a.as_str(), b.as_str()));
        for key in keys {
            let value = loader
                .path_for_lib_file_resolutions
                .borrow()
                .get(&key)
                .cloned()
                .expect("key from the map");
            let mut cache: ModeAwareCache<Arc<ResolvedModule>> = ModeAwareCache::default();
            cache.insert(
                ModeAwareCacheKey {
                    name: value.library_name.clone(),
                    mode: ModuleKind::COMMON_JS,
                },
                value.resolution.clone(),
            );
            resolved_modules.insert(key, cache);
            for trace in &value.trace {
                loader.host.trace(trace.message, trace.args.clone());
            }
        }

        ProcessedFiles {
            finished_processing: true,
            load_stats: LoadStatCache(Cell::new(self.load_stats.take())),
            resolver: loader.resolver.clone(),
            files: all_files,
            duplicate_source_files,
            files_by_path,
            project_reference_file_mapper: Some(loader.project_reference_file_mapper.clone()),
            resolved_modules: Arc::new(resolved_modules),
            type_resolutions_in_file: Rc::new(type_resolutions_in_file),
            source_file_meta_datas: Rc::new(source_file_meta_datas),
            jsx_runtime_import_specifiers: jsx_runtime_import_specifiers.map(Rc::new),
            import_helpers_import_specifiers: import_helpers_import_specifiers.map(Rc::new),
            source_files_found_searching_node_modules: Rc::new(
                source_files_found_searching_node_modules,
            ),
            lib_files: Rc::new(lib_files_map),
            missing_files,
            include_processor,
            output_file_to_project_reference_source: output_file_to_project_reference_source
                .map(Rc::new),
            redirect_targets_map: redirect_targets_map.map(Rc::new),
            redirect_files_by_path: redirect_files_by_path.map(Rc::new),
            // tsgo#4712
            content_mapper_diagnostics: loader.content_mapper_diagnostics.borrow().clone(),
        }
    }
}

/// What `collect_files` adds for one file to the maps of `FileMaps`.
enum FileMapEntry {
    /// A program file at `path`: its module resolutions and metadata, and
    /// whether the load found it searching node_modules.
    File {
        path: Path,
        resolutions: ModeAwareCache<Arc<ResolvedModule>>,
        meta: SourceFileMetaData,
        found_searching_node_modules: bool,
    },
    /// A package dedup redirect path that the load found searching
    /// node_modules.
    Found(Path),
}

/// The maps of `get_processed_files` that `collect_files` only adds to and
/// that another thread can own: Go `resolvedModules`, `sourceFileMetaDatas`
/// and `sourceFilesFoundSearchingNodeModules`.
#[derive(Default)]
struct FileMaps {
    resolved_modules: FxHashMap<Path, ModeAwareCache<Arc<ResolvedModule>>>,
    source_file_meta_datas: FxHashMap<Path, SourceFileMetaData>,
    source_files_found_searching_node_modules: FxHashSet<Path>,
}

impl FileMaps {
    /// Adds `entry` as Go `collectFiles` adds it (filesparser.go:455-548).
    fn add(&mut self, entry: FileMapEntry) {
        match entry {
            FileMapEntry::File {
                path,
                resolutions,
                meta,
                found_searching_node_modules,
            } => {
                self.resolved_modules.insert(path.clone(), resolutions);
                self.source_file_meta_datas.insert(path.clone(), meta);
                if found_searching_node_modules {
                    self.source_files_found_searching_node_modules.insert(path);
                }
            }
            FileMapEntry::Found(path) => {
                self.source_files_found_searching_node_modules.insert(path);
            }
        }
    }

    /// The maps that `walk` adds to through its sink. Each map gets its
    /// entries in the order of the walk, so it is the map that the walk
    /// would make itself. Unless `single_threaded`, another thread adds
    /// them while the walk goes on; when that thread cannot start, the
    /// walk's thread adds them.
    // PERF (serialgap1): Go's walk is serial (filesparser.go:337), and so is
    // this one; the map work of each file moves off it. VS Code: about 11k
    // files, two path copies and up to three inserts each.
    fn collect(single_threaded: bool, walk: impl FnOnce(&mut FileMapSink)) -> FileMaps {
        let mut sink = FileMapSink {
            maps: FileMaps::default(),
            batch: Vec::new(),
            send: None,
        };
        if single_threaded || cfg!(target_family = "wasm") {
            walk(&mut sink);
            return sink.maps;
        }
        let (send, receive) = std::sync::mpsc::channel::<Vec<FileMapEntry>>();
        std::thread::scope(|scope| {
            let Some(maps) = crate::program::helper_thread(scope, "goport-collect", move || {
                let mut maps = FileMaps::default();
                for batch in receive {
                    batch.into_iter().for_each(|entry| maps.add(entry));
                }
                maps
            }) else {
                walk(&mut sink);
                return sink.maps;
            };
            sink.batch = Vec::with_capacity(FileMapSink::BATCH);
            sink.send = Some(send);
            walk(&mut sink);
            // Sends the last batch. Dropping the sender ends the thread's
            // loop (also when the walk panics).
            sink.flush();
            drop(sink);
            maps.join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    }
}

/// Where `collect_files` puts the entries of `FileMaps`: `maps` on this
/// thread, or batches sent to the thread of `FileMaps::collect`.
struct FileMapSink {
    maps: FileMaps,
    batch: Vec<FileMapEntry>,
    send: Option<std::sync::mpsc::Sender<Vec<FileMapEntry>>>,
}

impl FileMapSink {
    /// Entries per batch: VS Code sends about 40.
    const BATCH: usize = 256;

    fn add(&mut self, entry: FileMapEntry) {
        if self.send.is_none() {
            self.maps.add(entry);
            return;
        }
        self.batch.push(entry);
        if self.batch.len() == Self::BATCH {
            self.flush();
        }
    }

    /// Sends the entries that are not sent yet.
    fn flush(&mut self) {
        if let Some(send) = &self.send
            && !self.batch.is_empty()
        {
            let full = std::mem::replace(&mut self.batch, Vec::with_capacity(Self::BATCH));
            // The thread receives until the sender drops.
            let _ = send.send(full);
        }
    }
}

#[cfg(test)]
mod file_maps_tests {
    use super::*;

    // followups58 (R208 reviewer): when the collect thread cannot start, the
    // walk's thread adds the entries itself, and the maps are the ones of a
    // serial walk. Paths come again with other metadata, so the order of the
    // inserts shows.
    #[test]
    fn collect_without_its_thread_makes_the_same_maps() {
        let walk = |threaded: bool| {
            move |sink: &mut FileMapSink| {
                assert_eq!(sink.send.is_some(), threaded);
                for i in 0..3 * FileMapSink::BATCH {
                    let path = Path(format!("/p/f{}.ts", i % 300));
                    sink.add(if i % 7 == 0 {
                        FileMapEntry::Found(path)
                    } else {
                        FileMapEntry::File {
                            path,
                            resolutions: ModeAwareCache::default(),
                            meta: SourceFileMetaData {
                                package_json_type: i.to_string(),
                                ..SourceFileMetaData::default()
                            },
                            found_searching_node_modules: i % 3 == 0,
                        }
                    });
                }
            }
        };
        let sorted = |maps: FileMaps| {
            let mut resolved: Vec<Path> = maps.resolved_modules.into_keys().collect();
            let mut metas: Vec<(Path, SourceFileMetaData)> =
                maps.source_file_meta_datas.into_iter().collect();
            let mut found: Vec<Path> = maps
                .source_files_found_searching_node_modules
                .into_iter()
                .collect();
            resolved.sort_by(|a, b| a.0.cmp(&b.0));
            metas.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            found.sort_by(|a, b| a.0.cmp(&b.0));
            (resolved, metas, found)
        };
        let serial = sorted(FileMaps::collect(true, walk(false)));
        let threaded = sorted(FileMaps::collect(false, walk(true)));
        let fallback = sorted(crate::program::refuse_helper_threads(|| {
            FileMaps::collect(false, walk(false))
        }));
        assert_eq!(threaded, serial);
        assert_eq!(fallback, serial);
    }
}

/// Go `strings.Join(core.Flatten(supportedExtensions), "', '")`.
pub(crate) fn join_flattened_extensions(extensions: &[Vec<String>]) -> String {
    extensions
        .iter()
        .flatten()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("', '")
}

/// Go `&processingDiagnostic{kind: processingDiagnosticKindExplainingFileInclude, data: &includeExplainingDiagnostic{...}}`.
pub(crate) fn new_explaining_processing_diagnostic(
    diagnostic_reason: Option<Rc<FileIncludeReason>>,
    message: &'static Message,
    args: Vec<String>,
) -> Rc<ProcessingDiagnostic> {
    Rc::new(ProcessingDiagnostic {
        kind: ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE,
        data: ProcessingDiagnosticData::IncludeExplaining(IncludeExplainingDiagnostic {
            file: Path::default(),
            diagnostic_reason,
            message,
            args,
        }),
    })
}

/// Go `&processingDiagnostic{kind: processingDiagnosticKindUnknownReference, data: includeReason}`.
pub(crate) fn new_unknown_reference_processing_diagnostic(
    include_reason: Rc<FileIncludeReason>,
) -> Rc<ProcessingDiagnostic> {
    Rc::new(ProcessingDiagnostic {
        kind: ProcessingDiagnosticKind::UNKNOWN_REFERENCE,
        data: ProcessingDiagnosticData::FileIncludeReason(include_reason),
    })
}

// ──────────────────────────────────────────────────────────────────────
// Parse prefetch
// ──────────────────────────────────────────────────────────────────────
//
// PORT: Go runs `parseTask.load` (read, parse, resolve, queue the
// subtasks) in one goroutine per task. The Rust loader keeps the Go
// single-threaded order, because store ids follow the load order. Parse
// workers do the rest of Go's parallel work ahead of it: they read and
// parse queued files, resolve their imports with their own resolvers, and
// queue the files they find. The loader checks every worker result
// (`take_prefetched`) and reads the answers the workers resolved
// (`SharedResolutionCache`), so the output is the same.

/// Number of parse workers next to the loading thread at the start of a
/// load: the parse threads of a program that is not large
/// (`ThreadBudget::parse_threads`), less the loading thread.
/// `GOPORT_PARSE_THREADS` sets it (0 turns prefetch off).
pub(crate) fn prefetch_worker_count() -> usize {
    if let Some(count) = parse_threads_from_env() {
        return count;
    }
    ThreadBudget::current().parse_threads(false) - 1
}

/// The workers for content-mapped jobs (`PrefetchPool::add_mapped_workers`):
/// as many as the other parse workers. `GOPORT_MAPPED_THREADS` sets it.
// PORT: not in Go (see `PrefetchJob::mapped`).
fn mapped_worker_count() -> usize {
    std::env::var("GOPORT_MAPPED_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(prefetch_worker_count)
}

/// The parse workers that `FilesParser::parse` adds for a large program
/// (`note_program_load`), up to `ThreadBudget::parse_large` parse threads.
/// None when `GOPORT_PARSE_THREADS` sets the count.
fn extra_worker_count(large: bool) -> usize {
    if parse_threads_from_env().is_some() {
        return 0;
    }
    let budget = ThreadBudget::current();
    budget
        .parse_threads(large)
        .saturating_sub(budget.parse_threads(false))
}

/// The parse thread count that `GOPORT_PARSE_THREADS` sets, if any.
fn parse_threads_from_env() -> Option<usize> {
    std::env::var("GOPORT_PARSE_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
}

/// True when a program load on this thread starts parse workers
/// (`FilesParser::parse`), unless the program is single threaded.
pub(crate) fn parse_workers_enabled() -> bool {
    prefetch_worker_count() > 0 && PREFETCH.with(|p| p.borrow().is_none())
}

/// How many of the largest root files per parse worker the workers parse
/// before the other jobs (`PrefetchShared::rank_roots`).
const ROOT_RANK_PER_WORKER: usize = 2;

/// The parse of one file by a parse worker.
struct PrefetchJob {
    /// Provisional store id offset (`new_detached_file_store`).
    job: usize,
    opts: SourceFileParseOptions,
    script_kind: ScriptKind,
    /// The references of a file that the loader's host gives from its
    /// cache (`CompilerHost::cached_source_file_refs`). The worker does not
    /// parse such a file: it only resolves and queues its references.
    cached: Option<Arc<FileRefs>>,
    /// True when the parse is the parse of a freeable file version
    /// (`PrefetchQueue::freeable`): the worker parses it as the loader
    /// would in `ast::enter_freeable_parse`, so its store owns its nodes and
    /// its text is not leaked.
    freeable: bool,
    state: Mutex<PrefetchState>,
    done: Condvar,
    /// Set with `state` locked when a loader waits on `done`
    /// (`PrefetchJob::wait_done`), so the worker that leaves the job wakes
    /// it (`PrefetchJob::settle`). A job that no loader waits for wakes
    /// none: a wake is a syscall even with no waiter.
    waited: AtomicBool,
    /// What the worker that parsed the file resolved for the loader after
    /// the parse (`FilePrep`), once it is there.
    prep: Mutex<Option<Box<FilePrep>>>,
    /// The transform of a content-mapped file: the worker parses the
    /// mapper's virtual text (`prefetch_mapped`).
    // PORT: not in Go, where the parse goroutines transform (tsgo#4712).
    mapped: Option<Arc<ConcurrentTransform>>,
    /// When the job was queued, for the debug counts of a claim
    /// (`GOPORT_PREFETCH_STATS`). `None` without them.
    queued_at: Option<std::time::Instant>,
    /// True when a loader batch put the job below the others
    /// (`queue_batch`), for the debug counts.
    under: AtomicBool,
    /// Tests: set when the worker that parsed the file has stored its prep
    /// or made none (`set_prep_wait`).
    #[cfg(test)]
    prep_settled: AtomicBool,
}

impl PrefetchJob {
    /// Waits on `done` with `state` locked, after it marks that a loader
    /// waits (`waited`).
    fn wait_done<'j>(
        &'j self,
        state: std::sync::MutexGuard<'j, PrefetchState>,
    ) -> std::sync::MutexGuard<'j, PrefetchState> {
        self.waited.store(true, AtomicOrdering::Relaxed);
        self.done
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sets the state of a job that its worker leaves, and wakes the
    /// loaders that wait for it (`wait_done`), if any.
    fn settle(&self, result: Option<PrefetchResult>) {
        let mut state = lock(&self.state);
        *state = PrefetchState::Done(result);
        // `waited` is set with `state` locked, so it is read here with the
        // same lock.
        let waited = self.waited.load(AtomicOrdering::Relaxed);
        drop(state);
        if waited {
            self.done.notify_all();
        }
    }
}

/// Sets `PrefetchJob::prep_settled` when the worker leaves the job.
#[cfg(test)]
struct PrepSettled<'j>(&'j PrefetchJob);

#[cfg(test)]
impl Drop for PrepSettled<'_> {
    fn drop(&mut self) {
        self.0.prep_settled.store(true, AtomicOrdering::Release);
    }
}

enum PrefetchState {
    Queued,
    Running,
    /// What the worker read and parsed. `None`: the file could not be read.
    Done(Option<PrefetchResult>),
    /// The loader took the job; a worker must not start it.
    Claimed,
}

/// A parse worker's read and parse of one file.
struct PrefetchResult {
    /// The file text as the worker's OS file system read it.
    text: FileText,
    /// The parse of `text`. `None`: the parse is not usable (it made
    /// thread-local state, or it panicked).
    parse: Option<DetachedParse>,
    /// The metadata that the worker found before the parse and parsed
    /// with, when the loader takes worker answers (`take_prefetched_meta`).
    meta: Option<WorkerMeta>,
    /// For a content-mapped file: the worker's transform result, whose
    /// virtual text `parse` parsed (`take_prefetched_mapped`).
    mapped: Option<MappedPrefetch>,
}

/// The transform result of a content-mapped job (`prefetch_mapped`).
// PORT: not in Go (see `PrefetchJob::mapped`).
struct MappedPrefetch {
    result: Result<crate::contentmapper::Result, crate::gostd::GoError>,
}

/// Go `loadSourceFileMetaData` of a file on its parse worker: the metadata,
/// and the lookup logs of the package scope walk of its directory
/// (`SharedResolutionCache::store_scope`), which the loader notes when it
/// takes the metadata, as for a shared answer.
// PORT: not in Go (loadpar1). Go finds the metadata in the parse task.
#[derive(Clone)]
struct WorkerMeta {
    meta: SourceFileMetaData,
    package_jsons: Arc<[PackageJsonLookup]>,
    lookups: Option<Arc<[StatLookup]>>,
}

/// What the parse worker of a file resolved for the loader, after the
/// parse, with the shared resolution cache: the answer of each synthetic
/// import and each import of the file (Go `resolveImportsAndModuleAugmentations`),
/// with the inputs that it resolved them with. The loader takes an answer
/// only when these inputs are its own (`fits`) and the name has the mode
/// that the worker used (`answer`); it still makes the synthetic nodes and
/// the subtasks itself, in the queue order. With a `commit`, the worker
/// made the module resolution map and the subtask data.
// PORT: not in Go (loadpar1). Go resolves in each parse task, which runs
// on its own goroutine; here the loader runs the tasks in order on one
// thread, so the resolutions run ahead on the workers.
pub(crate) struct FilePrep {
    meta: SourceFileMetaData,
    /// The config name of the project reference redirect, or empty.
    redirect: String,
    /// The containing file of the resolutions (Go `getRedirectForResolution`).
    containing_file: String,
    synthetic: SyntheticImports,
    /// The number of `file.imports`.
    imports: usize,
    /// The mode of each answer: Go `getModeForUsageLocation` of its name,
    /// from the usage that the worker read (`names`); without `names`, the
    /// mode of a plain `import .. from` in the file (`guess_import_mode`).
    modes: Vec<ResolutionMode>,
    /// The synthetic imports, then the imports, by module name index.
    /// `None` for an empty name or a source phase import.
    answers: Vec<Option<SharedResolution<Arc<ResolvedModule>>>>,
    /// The names, usages and flags of the file's imports and module
    /// augmentations, read while the nodes were on the worker thread
    /// (parload1). The loader reads them in place of the nodes. `None`
    /// with `GOPORT_LOAD_NAMES=0` (an A/B switch).
    pub(crate) names: Option<Box<ImportNames>>,
    /// The worker's commit of the answers (loadpar3b), when it read the
    /// names and every answer is one that the loader adds without its own
    /// state. `None` with `GOPORT_LOAD_COMMIT=0` (an A/B switch).
    pub(crate) commit: Option<Box<PrepCommit>>,
}

/// What the loader makes of the answers of the synthetic imports and the
/// imports of a file (Go `resolveImportsAndModuleAugmentations`, the part
/// after each resolution, fileloader.go:886 to :951), made by the file's
/// parse worker, which plays Go's parse task goroutine here. The loader
/// moves it in its queue order, and still makes the include reasons and
/// the subtasks (`Rc`), notes the answer logs and resolves the module
/// augmentations.
// PORT: not in Go (loadpar3b). Go runs all of it in the parse task.
pub(crate) struct PrepCommit {
    /// Go `resolutionsInFile` of the synthetic imports and imports: the
    /// loader's capacity and insert order, so the loader's later inserts
    /// (the module augmentations) give the same map.
    pub(crate) resolutions: ModeAwareCache<Arc<ResolvedModule>>,
    /// The files that the loader adds, in module name order.
    pub(crate) adds: Vec<PrepAdd>,
}

/// One file of a `PrepCommit` (Go `resolvedRef` of an import, fileloader.go:937).
pub(crate) struct PrepAdd {
    /// The module name index (the synthetic imports, then the imports).
    pub(crate) index: usize,
    /// The resolved file name, normalized.
    pub(crate) file_name: String,
    pub(crate) path: Path,
    /// The hash of `path` in `task_data_by_path` (`ParseTask::path_hash`).
    pub(crate) path_hash: u64,
    pub(crate) package_id: PackageId,
    pub(crate) increase_depth: bool,
}

impl PrepCommit {
    /// Adds the answer of module name `index` with `mode`. `adds` is Go's
    /// import index condition (a synthetic import, or `import_adds_file`).
    /// False when the loader must decide for the file: no answer, or a
    /// resolution whose diagnostic or JS file test needs more than the
    /// options (a JS file needs the loader's project reference mapper).
    fn push(
        &mut self,
        index: usize,
        name: &str,
        mode: ResolutionMode,
        answer: Option<&SharedResolution<Arc<ResolvedModule>>>,
        adds: bool,
        options: &CompilerOptions,
    ) -> bool {
        let Some(answer) = answer else {
            return false;
        };
        let resolved = &answer.value;
        self.resolutions.insert(
            ModeAwareCacheKey {
                name: name.to_string(),
                mode,
            },
            resolved.clone(),
        );
        if !resolved.is_resolved() {
            return true;
        }
        // Go `GetResolutionDiagnostic` (module/util.go:132) is nil for
        // these, and `isJsFile` (fileloader.go:919) is false for a TS or
        // JSON name before it asks the mapper. `noResolve` is off whenever
        // workers resolve (`workers_resolve_imports`).
        let no_diagnostic = match resolved.extension.as_str() {
            EXTENSION_TS | EXTENSION_DTS | EXTENSION_MTS | EXTENSION_DMTS | EXTENSION_CTS
            | EXTENSION_DCTS => true,
            EXTENSION_TSX => options.jsx != JsxEmit::NONE,
            EXTENSION_JSON => options.get_resolve_json_module(),
            _ => false,
        };
        if !no_diagnostic
            || resolved.resolved_using_extra_extensions
            || !file_extension_is_one_of(
                &resolved.resolved_file_name,
                SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
            )
        {
            return false;
        }
        if adds {
            let file_name = &resolved.resolved_file_name;
            self.adds.push(PrepAdd {
                index,
                file_name: if is_normalized_path(file_name) {
                    file_name.clone()
                } else {
                    normalize_path(file_name)
                },
                path: resolved.resolved_path.clone(),
                path_hash: FxBuildHasher.hash_one(&resolved.resolved_path),
                package_id: resolved.package_id.clone(),
                increase_depth: resolved.is_external_library_import,
            });
        }
        true
    }
}

impl FilePrep {
    /// The answers by module name index (`answers`).
    pub(crate) fn answers(&self) -> &[Option<SharedResolution<Arc<ResolvedModule>>>] {
        &self.answers
    }

    /// The mode of module name `index` (`modes`), for the loader's checks.
    #[cfg(debug_assertions)]
    pub(crate) fn mode(&self, index: usize) -> Option<ResolutionMode> {
        self.modes.get(index).copied()
    }

    /// True when the worker resolved with the loader's inputs: the
    /// metadata, the redirect and the containing file, the same synthetic
    /// imports and import count.
    pub(crate) fn fits(
        &self,
        meta: &SourceFileMetaData,
        redirect: &str,
        containing_file: &str,
        synthetic: &SyntheticImports,
        imports: usize,
    ) -> bool {
        self.meta == *meta
            && self.redirect == redirect
            && self.containing_file == containing_file
            && self.synthetic == *synthetic
            && self.imports == imports
            && self.answers.len() == synthetic.names().count() + imports
    }

    /// The worker answer of module name `index` (the synthetic imports,
    /// then the imports), when the loader resolves it with `mode`.
    pub(crate) fn answer(
        &self,
        index: usize,
        mode: ResolutionMode,
    ) -> Option<&SharedResolution<Arc<ResolvedModule>>> {
        if self.modes.get(index) != Some(&mode) {
            return None;
        }
        self.answers.get(index)?.as_ref()
    }
}

/// The job of the worker parse that the loader took for the file that it
/// loads now (`take_prefetched`, `ParseTask::load`).
pub(crate) struct TakenParse(Arc<PrefetchJob>);

impl TakenParse {
    /// The job whose parse `take_prefetched` gave last, when `file` is
    /// that parse (the host gave it as it is).
    fn of(file: &ParsedSourceFile) -> Option<Self> {
        TAKEN
            .with(|taken| taken.borrow_mut().take())
            .filter(|(_, root)| *root == file.root)
            .map(|(job, _)| TakenParse(job))
    }

    /// The prep of the file, when its worker has published it. The loader
    /// does not wait for a worker that still resolves (or makes no prep):
    /// it resolves the names itself, and reads each answer that the worker
    /// has stored in the shared cache by then, so it is not later than the
    /// worker.
    pub(crate) fn take_prep(self) -> Option<Box<FilePrep>> {
        // A test can make the loader wait (up to 60 s) until the worker has
        // stored the prep or made none (`set_prep_wait`).
        #[cfg(test)]
        if PREP_WAIT.with(Cell::get) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
            while !self.0.prep_settled.load(AtomicOrdering::Acquire)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        let prep = lock(&self.0.prep).take();
        if prep.is_none() {
            count_prep(|c| c.preps_missing += 1);
        }
        prep
    }
}

/// The preps that the loader took (`TakenParse::take_prep`), with their
/// names and commit, once it is done with them. Freeing them on the loader
/// put the reads and frees of memory that a worker wrote on the serial
/// path (loadstamp1 C5).
// PERF (loadfree1): they free on the free thread (`drop_in_background`),
// in batches, as Go's GC frees them beside the load. Not in Go.
type WorkerGarbage = Vec<Box<FilePrep>>;

/// The preps in one batch of `WorkerGarbage`.
const WORKER_GARBAGE_BATCH: usize = 256;

thread_local! {
    /// The `WorkerGarbage` of the load on this thread.
    static WORKER_GARBAGE: RefCell<WorkerGarbage> = const { RefCell::new(Vec::new()) };
}

/// Frees a prep that the loader took off the loader: it goes in the
/// loader's `WorkerGarbage`, and a full batch goes to the free thread.
pub(crate) fn free_prep_off_loader(prep: Box<FilePrep>) {
    WORKER_GARBAGE.with(|garbage| {
        let mut garbage = garbage.borrow_mut();
        garbage.push(prep);
        if garbage.len() >= WORKER_GARBAGE_BATCH {
            let full = std::mem::replace(&mut *garbage, Vec::with_capacity(WORKER_GARBAGE_BATCH));
            crate::execute::build::build_task::drop_in_background(full);
        }
    });
}

/// The `WorkerGarbage` of this thread's load, at the end of the load.
fn take_worker_garbage() -> WorkerGarbage {
    WORKER_GARBAGE.with(|garbage| std::mem::take(&mut *garbage.borrow_mut()))
}

/// The metadata of `file_name` that its parse worker found, when the
/// loader takes worker answers (`WorkerMeta`). Waits for a running worker
/// parse, as `take_prefetched` does. `None`: the loader finds it.
// PORT: not in Go (loadpar1).
fn take_prefetched_meta(loader: &FileLoader, file_name: &str) -> Option<SourceFileMetaData> {
    if loader.shared_resolution.is_none() || loader.opts.skip_module_resolution {
        return None;
    }
    let shared = PREFETCH.with(|p| p.borrow().clone())?;
    let job = shared.job(file_name)?;
    // A content-mapped job finds no metadata, so the loader does not wait
    // for its transform here.
    if job.mapped.is_some() {
        return None;
    }
    let mut state = lock(&job.state);
    let mut waited: Option<std::time::Instant> = None;
    let meta = loop {
        match &*state {
            PrefetchState::Running => {
                waited.get_or_insert_with(std::time::Instant::now);
                state = job.wait_done(state);
            }
            // A test can make the loader wait for a job that no worker has
            // started yet (`set_meta_wait`), so the worker finds the
            // metadata whatever the timing.
            #[cfg(test)]
            PrefetchState::Queued if META_WAIT.with(Cell::get) => {
                state = job.wait_done(state);
            }
            PrefetchState::Done(Some(result)) => break result.meta.clone(),
            _ => break None,
        }
    };
    drop(state);
    shared.count(|c| {
        if let Some(start) = waited {
            c.waited += 1;
            c.wait += start.elapsed();
        }
        if meta.is_some() {
            c.meta_taken += 1;
        }
    });
    let meta = meta?;
    loader.note_worker_logs(&meta.package_jsons, &meta.lookups);
    loader.adopt_worker_package_jsons(&meta.package_jsons);
    Some(meta.meta)
}

thread_local! {
    /// A test's choice of `load_prep_enabled` for its loads on this thread.
    static LOAD_PREP: Cell<Option<bool>> = const { Cell::new(None) };
    /// The preps that loads on this thread took (`FilePrep`), for tests.
    static PREPS_TAKEN: Cell<usize> = const { Cell::new(0) };
    /// A test's choice of `load_names_enabled` for its loads on this thread.
    static LOAD_NAMES: Cell<Option<bool>> = const { Cell::new(None) };
    /// The names that loads on this thread resolved themselves, for tests.
    #[cfg(test)]
    static NAMES_OWN: Cell<usize> = const { Cell::new(0) };
    /// A test's choice of `load_commit_enabled` for its loads on this thread.
    static LOAD_COMMIT: Cell<Option<bool>> = const { Cell::new(None) };
}

/// True when the parse workers make the commit of their answers for the
/// loader (`FilePrep::commit`): unless `GOPORT_LOAD_COMMIT` is `0` (an A/B
/// switch), or a test chose (`set_load_commit`).
fn load_commit_enabled() -> bool {
    LOAD_COMMIT
        .with(Cell::get)
        .unwrap_or_else(|| std::env::var_os("GOPORT_LOAD_COMMIT").is_none_or(|value| value != "0"))
}

/// Sets `load_commit_enabled` for the loads on this thread (tests).
#[cfg(test)]
pub(crate) fn set_load_commit(on: Option<bool>) {
    LOAD_COMMIT.with(|commit| commit.set(on));
}

/// Counts whether the loader took the commit of a named prep
/// (`FilePrep::commit`).
pub(crate) fn note_commit(taken: bool) {
    count_prep(|c| {
        c.commits_taken += usize::from(taken);
        c.commits_missing += usize::from(!taken);
    });
}

/// True when the parse workers of a load on this thread make preps for
/// the loader (`FilePrep`): unless `GOPORT_LOAD_PREP` is `0` (an A/B
/// switch), or a test chose (`set_load_prep`).
fn load_prep_enabled() -> bool {
    LOAD_PREP
        .with(Cell::get)
        .unwrap_or_else(|| std::env::var_os("GOPORT_LOAD_PREP").is_none_or(|value| value != "0"))
}

/// True when the parse workers read the import names of their parses for
/// the loader (`FilePrep::names`): unless `GOPORT_LOAD_NAMES` is `0` (an
/// A/B switch), or a test chose (`set_load_names`).
fn load_names_enabled() -> bool {
    LOAD_NAMES
        .with(Cell::get)
        .unwrap_or_else(|| std::env::var_os("GOPORT_LOAD_NAMES").is_none_or(|value| value != "0"))
}

/// Sets `load_names_enabled` for the loads on this thread (tests).
#[cfg(test)]
pub(crate) fn set_load_names(on: Option<bool>) {
    LOAD_NAMES.with(|names| names.set(on));
}

/// The module names that loads on this thread resolved themselves so far
/// (tests): no prep answer had the name's mode, or no prep fit.
#[cfg(test)]
pub(crate) fn names_own() -> usize {
    NAMES_OWN.with(Cell::get)
}

/// Notes module names that the loader resolved itself.
pub(crate) fn note_names_own(count: usize) {
    #[cfg(test)]
    NAMES_OWN.with(|own| own.set(own.get() + count));
    let _ = count;
}

/// Sets `load_prep_enabled` for the loads on this thread (tests).
#[cfg(test)]
pub(crate) fn set_load_prep(on: Option<bool>) {
    LOAD_PREP.with(|prep| prep.set(on));
}

#[cfg(test)]
thread_local! {
    /// A test's choice to make the loads on this thread wait in
    /// `take_prefetched_meta` for a job that is still queued.
    static META_WAIT: Cell<bool> = const { Cell::new(false) };
    /// A test's choice to make the loads on this thread wait in
    /// `TakenParse::take_prep` for the worker's prep.
    static PREP_WAIT: Cell<bool> = const { Cell::new(false) };
}

/// Makes the loads on this thread wait in `TakenParse::take_prep` until the
/// worker that parsed the file has stored its prep or made none, so the
/// loader takes each prep whatever the timing (tests). A worker stores the
/// prep after the parse that the loader waits for.
#[cfg(test)]
pub(crate) fn set_prep_wait(on: bool) {
    PREP_WAIT.with(|wait| wait.set(on));
}

/// Makes the loads on this thread wait for the parse worker of each file
/// in `take_prefetched_meta`, also when no worker has started it yet, so
/// the loader always takes the worker's metadata (tests). Only for loads
/// with parse workers (`parse_workers_enabled`): with none, a queued job
/// never starts.
#[cfg(test)]
pub(crate) fn set_meta_wait(on: bool) {
    META_WAIT.with(|wait| wait.set(on));
}

/// The preps that loads on this thread took so far (tests).
#[cfg(test)]
pub(crate) fn preps_taken() -> usize {
    PREPS_TAKEN.with(Cell::get)
}

/// Notes a prep that the loader took.
pub(crate) fn note_prep_taken() {
    PREPS_TAKEN.with(|count| count.set(count.get() + 1));
}

/// Updates the debug counts of the prefetch of this load
/// (`GOPORT_PREFETCH_STATS`), when parse workers run.
pub(crate) fn count_prep(update: impl FnOnce(&mut PrefetchStats)) {
    if let Some(shared) = PREFETCH.with(|p| p.borrow().clone()) {
        shared.count(update);
    }
}

/// What the loader can take from the parse workers for one file
/// (`take_prefetched`).
pub enum Prefetched {
    /// A worker parse of the file, adopted into the stores of this thread.
    /// It equals what `parse_source_file` would make here now.
    Parse(ParsedSourceFile),
    /// The text that a worker read, when its parse is not usable.
    Text(FileText),
    /// No worker read the file, the read failed, or the worker text is not
    /// the text that the loader read.
    Nothing,
}

/// A parse that `FilesParser::start` asks the parse workers for
/// (`FilesParser::prefetch_request`).
enum PrefetchRequest {
    /// The file of the first task of a path: a new job, unless the file
    /// has one.
    New(SourceFileParseOptions, ScriptKind),
    /// The file of a later task of a path that the loader has not reached:
    /// the file name of a job to move up, when one exists.
    Known(String),
    /// The first task of a content-mapped file: a job that transforms it.
    // PORT: not in Go (see `PrefetchJob::mapped`).
    Mapped(SourceFileParseOptions, Arc<ConcurrentTransform>),
}

/// The parse jobs by file name (`PrefetchQueue::by_name`), in 64 shards by
/// the name's hash, so the loader finds a job without the queue lock
/// (`PrefetchShared::job`). Every insert holds the queue lock too
/// (`PrefetchQueue::add`), so a file name still gets one job, and no job
/// is removed.
// PORT: port-only, as the prefetch queue is. Go has no parse prefetch and
// no central queue. The lock-free lookup is like Go's `taskDataByPath`
// (a `collections.SyncMap` keyed by path, filesparser.go:235), but this map
// holds the prefetch jobs and is keyed by file name (loadpar3b step A).
struct JobMap {
    shards: Box<[Mutex<FxHashMap<String, Arc<PrefetchJob>>>]>,
}

impl Default for JobMap {
    fn default() -> Self {
        JobMap {
            shards: (0..64).map(|_| Mutex::default()).collect(),
        }
    }
}

impl JobMap {
    fn shard(
        &self,
        file_name: &str,
    ) -> std::sync::MutexGuard<'_, FxHashMap<String, Arc<PrefetchJob>>> {
        use std::hash::BuildHasher;
        let hash = rustc_hash::FxBuildHasher.hash_one(file_name);
        lock(&self.shards[crate::frontend::module::shard_index(hash)])
    }

    fn get(&self, file_name: &str) -> Option<Arc<PrefetchJob>> {
        self.shard(file_name).get(file_name).cloned()
    }

    fn contains(&self, file_name: &str) -> bool {
        self.shard(file_name).contains_key(file_name)
    }

    /// Adds `job` by its file name. Call it with the queue lock held.
    fn insert(&self, job: Arc<PrefetchJob>) {
        self.shard(&job.opts.file_name)
            .insert(job.opts.file_name.clone(), job);
    }

    fn for_each(&self, mut f: impl FnMut(&Arc<PrefetchJob>)) {
        for shard in &self.shards {
            lock(shard).values().for_each(&mut f);
        }
    }
}

/// False when `GOPORT_JOB_MAP=0`: the loader finds jobs under the queue
/// lock (`PrefetchShared::job`) and `queue_batch` locks for every batch, as
/// at R224. An A/B switch, read once, or a test's choice for the pools that
/// start on its thread (`set_job_map`).
fn job_map_enabled() -> bool {
    #[cfg(test)]
    if let Some(on) = JOB_MAP.with(Cell::get) {
        return on;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GOPORT_JOB_MAP").is_none_or(|value| value != "0"))
}

#[cfg(test)]
thread_local! {
    /// A test's choice of `job_map_enabled` for the pools of this thread.
    static JOB_MAP: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Sets `job_map_enabled` for the pools that start on this thread (tests).
#[cfg(test)]
pub(crate) fn set_job_map(on: Option<bool>) {
    JOB_MAP.with(|job_map| job_map.set(on));
}

#[derive(Default)]
struct PrefetchQueue {
    /// Jobs no worker has taken yet. Workers take the newest first, like
    /// the loader's queue, but take `lib.dom.d.ts` before all others. A job
    /// that moves up is pushed again, so it can be in the list more than
    /// once (`rank_largest`, `queue_batch`, and the move-up of a worker's
    /// `PrefetchShared::queue`); a worker skips a job that is no longer
    /// queued.
    pending: Vec<Arc<PrefetchJob>>,
    /// The content-mapped jobs no worker has taken yet, newest first too.
    /// The workers of `PrefetchPool::add_mapped_workers` take only these;
    /// the other workers take them when `pending` is empty.
    // PORT: not in Go (see `PrefetchJob::mapped`).
    mapped: Vec<Arc<PrefetchJob>>,
    /// The queued `lib.dom.d.ts` job. It is the largest file of most
    /// programs, so its parse starts first to end before the loader needs it.
    first: Option<Arc<PrefetchJob>>,
    /// The root file jobs for the next free worker to rank by file size,
    /// and how many of the largest to move up (`PrefetchShared::rank_roots`).
    rank: Option<(Vec<Arc<PrefetchJob>>, usize)>,
    /// The root file names of an early pool for the next free worker to
    /// queue below the other jobs (`PrefetchShared::queue_roots`).
    roots: Option<Vec<String>>,
    /// Every job by file name. A file name is queued once. The same map
    /// as `PrefetchShared::jobs`.
    by_name: Arc<JobMap>,
    /// The files that the loader's host gives from its cache, with their
    /// references (`CompilerHost::cached_source_file_refs`). Their jobs
    /// parse nothing (`PrefetchJob::cached`), unless the cached parse does
    /// not fit the job's options (`FileRefs::fits`).
    cached: FxHashMap<String, Arc<FileRefs>>,
    /// The paths whose new parse is a freeable file version
    /// (`ast::freeable_path`, which reads the loading thread's state), when
    /// the host asks for it (`CompilerHost::freeable_worker_parses`). Empty
    /// otherwise: every worker parse is static.
    freeable: Arc<FxHashSet<String>>,
    /// The source files of the project references whose output `.d.ts`
    /// file the loader loads in their place, by source file name, and the
    /// output file name and path (`get_parse_file_redirect`, a program
    /// that does not use the sources of its references). A request for
    /// such a source queues its output.
    redirects: FxHashMap<String, (String, Path)>,
    next_job: usize,
    closed: bool,
    /// The workers that wait on `PrefetchShared::ready` and no wake has
    /// reached yet (`PrefetchShared::wake`).
    sleepers: usize,
    /// True once the workers of `PrefetchPool::add_mapped_workers` start.
    /// They wait on `ready` too (`PrefetchShared::wake`).
    // PORT: not in Go (see `PrefetchJob::mapped`).
    mapped_workers: bool,
    /// True when the debug counts are kept (`GOPORT_PREFETCH_STATS`): jobs
    /// note when they are queued.
    timed: bool,
    /// `move_up_enabled` of the thread that started the pool.
    move_up: bool,
}

impl PrefetchQueue {
    /// Makes the job of a file that has none and records it by name. A
    /// `lib.dom.d.ts` job that parses goes to `first`; the caller pushes
    /// other jobs. `None` when the queue is closed or full.
    fn add(
        &mut self,
        opts: SourceFileParseOptions,
        script_kind: ScriptKind,
        mapped: Option<Arc<ConcurrentTransform>>,
    ) -> Option<Arc<PrefetchJob>> {
        if self.closed || self.next_job >= DETACHED_STORE_LIMIT {
            return None;
        }
        let (opts, script_kind) = self.redirected(opts, script_kind);
        if let Some(job) = self.by_name.get(&opts.file_name) {
            return Some(job);
        }
        let cached = self
            .cached
            .get(&opts.file_name)
            .filter(|refs| mapped.is_none() && refs.fits(|| opts.external_module_indicator_options))
            .cloned();
        let freeable = !self.freeable.is_empty() && self.freeable.contains(&opts.path.0);
        let job = Arc::new(PrefetchJob {
            job: self.next_job,
            opts,
            script_kind,
            cached,
            freeable,
            state: Mutex::new(PrefetchState::Queued),
            done: Condvar::new(),
            waited: AtomicBool::new(false),
            prep: Mutex::new(None),
            mapped,
            queued_at: self.timed.then(std::time::Instant::now),
            under: AtomicBool::new(false),
            #[cfg(test)]
            prep_settled: AtomicBool::new(false),
        });
        self.next_job += 1;
        self.by_name.insert(job.clone());
        if job.cached.is_none() && job.opts.file_name.ends_with("/lib.dom.d.ts") {
            self.first = Some(job.clone());
        }
        Some(job)
    }

    /// Puts a job that no worker has taken on its stack.
    fn push_pending(&mut self, job: Arc<PrefetchJob>) {
        if job.mapped.is_some() {
            self.mapped.push(job);
        } else {
            self.pending.push(job);
        }
    }

    /// The job of the file of `request`: the job that exists, or else for
    /// a `New` request a new one (`add`).
    fn job_of(&mut self, request: PrefetchRequest) -> Option<Arc<PrefetchJob>> {
        let (opts, script_kind, mapped) = match request {
            PrefetchRequest::New(opts, script_kind) => (opts, script_kind, None),
            // The script kind is the one of the virtual text, which the
            // worker finds.
            // The loader loads the output `.d.ts` file of a redirected
            // source (`redirects`), and sends no transform for it.
            PrefetchRequest::Mapped(opts, _) if self.redirects.contains_key(&opts.file_name) => {
                return None;
            }
            PrefetchRequest::Mapped(opts, transform) => {
                (opts, ScriptKind::UNKNOWN, Some(transform))
            }
            PrefetchRequest::Known(file_name) => {
                let file_name = match self.redirects.get(&file_name) {
                    Some((output, _)) => output,
                    None => &file_name,
                };
                return self.by_name.get(file_name);
            }
        };
        match self.by_name.get(&opts.file_name) {
            Some(job) => Some(job),
            None => self.add(opts, script_kind, mapped),
        }
    }

    /// The parse that the loader makes for a request of `opts`: of the
    /// output `.d.ts` file of a redirected source (`redirects`), with the
    /// guessed options of `queue_names`, else of `opts`.
    fn redirected(
        &self,
        opts: SourceFileParseOptions,
        script_kind: ScriptKind,
    ) -> (SourceFileParseOptions, ScriptKind) {
        match self.redirects.get(&opts.file_name) {
            Some((file_name, path)) => (
                SourceFileParseOptions {
                    file_name: file_name.clone(),
                    path: path.clone(),
                    external_module_indicator_options: ExternalModuleIndicatorOptions::default(),
                },
                get_script_kind_from_file_name(file_name),
            ),
            None => (opts, script_kind),
        }
    }

    /// True for the `first` job while no worker has taken it.
    fn is_first(&self, job: &Arc<PrefetchJob>) -> bool {
        self.first
            .as_ref()
            .is_some_and(|first| Arc::ptr_eq(first, job))
    }
}

/// What a parse worker needs to guess the files that a parsed file
/// references (`queue_references`). The loader takes workers only with its
/// own config (`FilesParser::parse`).
#[derive(Clone, PartialEq, Eq)]
struct PrefetchConfig {
    current_directory: String,
    use_case_sensitive_file_names: bool,
    /// Go `defaultLibraryPath`. Empty when `libReplacement` can move lib
    /// files, so lib references are not guessed.
    default_library_path: String,
}

impl PrefetchConfig {
    /// The config of the program that `loader` loads.
    fn of_loader(loader: &FileLoader) -> Self {
        let lib_replacement = loader
            .opts
            .config
            .compiler_options()
            .lib_replacement
            .is_true();
        PrefetchConfig {
            current_directory: loader.host.get_current_directory(),
            use_case_sensitive_file_names: loader.host.fs().use_case_sensitive_file_names(),
            default_library_path: if lib_replacement {
                String::new()
            } else {
                loader.default_library_path.clone()
            },
        }
    }
}

/// What a parse worker needs to resolve the imports and type reference
/// directives of the files it parses, as each Go parse task does with the
/// program resolver.
struct WorkerResolveConfig {
    options: CompilerOptions,
    typings_location: String,
    project_name: String,
    /// Go `opts.Config.ContentMapperExtensions()`, the resolver's extra
    /// extensions (tsgo#4712).
    extra_extensions: Vec<String>,
    /// The cache that the loader's resolver reads (`FileLoader::shared_resolution`).
    /// `None`: the worker answers are only hints for the parse queue.
    shared: Option<Arc<SharedResolutionCache>>,
    /// The project references of the files in `redirects`: config name
    /// and options.
    references: Vec<(String, CompilerOptions)>,
    /// The source and output `.d.ts` files of the project references, by
    /// path: the source file name and the index in `references`. Go
    /// resolves the references of such a file with the reference's
    /// options, from its source file (`getRedirectForResolution`).
    // PORT: Go's third rule, the real path of a `.d.ts` file under
    // node_modules with `preserveSymlinks`, is not here: the worker
    // resolves such a file without a redirect, and the loader, which
    // resolves it with one, takes none of those answers (`FilePrep::fits`
    // and the cache key).
    redirects: FxHashMap<Path, (String, usize)>,
    /// True when the workers find the metadata of each file and resolve
    /// its names for the loader (`FilePrep`): the loader takes worker
    /// answers, and `GOPORT_LOAD_PREP` is not `0`.
    prep: bool,
    /// True when, with `prep`, the workers read the import names of each
    /// parse for the loader and resolve each name with its own mode
    /// (`FilePrep::names`): `GOPORT_LOAD_NAMES` is not `0`.
    names: bool,
    /// True when, with `names`, the workers make the commit of the
    /// answers (`FilePrep::commit`): `GOPORT_LOAD_COMMIT` is not `0`.
    commit: bool,
}

impl WorkerResolveConfig {
    /// The config of `loader`. `None` when workers do not resolve.
    fn of_loader(loader: &FileLoader) -> Option<Self> {
        Self::of_loader_with(loader, loader.shared_resolution.clone())
    }

    /// `of_loader` with `shared` as the loader's shared resolution cache,
    /// before the loader keeps it (`early_shared_resolution`).
    fn of_loader_with(
        loader: &FileLoader,
        shared: Option<Arc<SharedResolutionCache>>,
    ) -> Option<Self> {
        let options = loader.opts.config.compiler_options();
        // PORT: see `skip_module_resolution` and `create_module_resolver` in
        // `process_all_program_files`.
        if !super::file_loader::workers_resolve_imports(options)
            || loader.opts.skip_module_resolution
            || loader.custom_module_resolver
        {
            return None;
        }
        // Only a loader that takes worker answers needs the redirects: the
        // answer of a redirected file is another cache key.
        let mut references = Vec::new();
        let mut redirects = FxHashMap::default();
        if shared.is_some() {
            let mapper = loader.project_reference_file_mapper.borrow();
            let mut index_of: FxHashMap<*const ParsedCommandLine, usize> = FxHashMap::default();
            // Go `getRedirectForResolution` tries the sources first, so a
            // source entry replaces an output entry of the same path.
            let outputs = mapper.output_dts_to_project_reference.iter();
            let sources = mapper.source_to_project_reference.iter();
            for (path, reference) in outputs.chain(sources) {
                let Some(resolved) = reference.resolved.upgrade() else {
                    continue;
                };
                let index = *index_of.entry(Rc::as_ptr(&resolved)).or_insert_with(|| {
                    references.push((
                        resolved.config_name().to_string(),
                        (**resolved.compiler_options()).clone(),
                    ));
                    references.len() - 1
                });
                redirects.insert(path.clone(), (reference.source.clone(), index));
            }
        }
        Some(WorkerResolveConfig {
            options: (**options).clone(),
            typings_location: loader.opts.typings_location.clone(),
            project_name: loader.opts.project_name.clone(),
            extra_extensions: loader.content_mapper_extensions.clone(),
            prep: shared.is_some() && load_prep_enabled(),
            shared,
            references,
            redirects,
            names: load_names_enabled(),
            commit: load_commit_enabled(),
        })
    }

    /// The config of the early pool of `config` (`start_default_lib_prefetch`):
    /// `of_loader` of a tsc loader with no project references, no content
    /// mappers and a new shared resolution cache. `None` when such a loader
    /// shares no resolution cache (`process_all_program_files`).
    fn of_config(config: &ParsedCommandLine) -> Option<Self> {
        let options = config.compiler_options();
        if !super::file_loader::workers_resolve_imports(options)
            || options.trace_resolution == Tristate::True
            || config.has_project_references()
            || !config.content_mappers().is_empty()
        {
            return None;
        }
        Some(WorkerResolveConfig {
            options: (**options).clone(),
            typings_location: String::new(),
            project_name: String::new(),
            extra_extensions: Vec::new(),
            shared: Some(Arc::default()),
            references: Vec::new(),
            redirects: FxHashMap::default(),
            prep: load_prep_enabled(),
            names: load_names_enabled(),
            commit: load_commit_enabled(),
        })
    }

    /// True when the workers resolve the same with both configs: equal
    /// fields and the same shared resolution cache.
    fn same(&self, other: &Self) -> bool {
        let shared = match (&self.shared, &other.shared) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        shared
            && self.options == other.options
            && self.typings_location == other.typings_location
            && self.project_name == other.project_name
            && self.extra_extensions == other.extra_extensions
            && self.references == other.references
            && self.redirects == other.redirects
            && self.prep == other.prep
            && self.names == other.names
            && self.commit == other.commit
    }
}

/// The jobs that the loading thread and the parse workers share.
struct PrefetchShared {
    queue: Mutex<PrefetchQueue>,
    /// `PrefetchQueue::by_name`, which the loader reads without the queue
    /// lock (`job`) unless `GOPORT_JOB_MAP=0`.
    jobs: Arc<JobMap>,
    /// `job_map_enabled` when the pool started.
    job_map: bool,
    /// True when `PrefetchQueue::redirects` has an entry.
    has_redirects: AtomicBool,
    ready: Condvar,
    /// `queue.closed`, which a worker reads between resolutions.
    closed: AtomicBool,
    config: PrefetchConfig,
    /// The stat cache of the workers' file systems (`WorkerFs`).
    stats: Arc<SharedStatCache>,
    /// Set when the loader takes the workers (`FilesParser::parse`), or
    /// before the loader exists by an early pool with root jobs
    /// (`start_default_lib_prefetch`), which the loader takes only with an
    /// equal config (`early_shared_resolution`, `FilesParser::parse`).
    /// Until it is set, and with `None` inside, the workers do not resolve.
    resolve: OnceLock<Option<WorkerResolveConfig>>,
    /// Debug counts of `take_prefetched`, kept when `GOPORT_PREFETCH_STATS`
    /// is set and printed to stderr when the pool stops.
    counts: Option<Mutex<PrefetchStats>>,
}

/// What `take_prefetched` gave the loader during one program load.
#[derive(Default)]
pub(crate) struct PrefetchStats {
    /// Worker parses that the loader took.
    taken: usize,
    /// The files that the loader parsed itself, and why: no worker had
    /// started the job (the loader claims it), no job was queued, or the
    /// worker parse was not usable.
    claimed: Vec<String>,
    not_queued: Vec<String>,
    unusable: Vec<String>,
    /// Running worker parses that the loader waited for, and the time.
    waited: usize,
    wait: std::time::Duration,
    /// Worker reads and parses that the loader did not take, and their
    /// text bytes (`PrefetchShared::untaken`).
    untaken: usize,
    untaken_bytes: u64,
    /// Worker metadata that the loader took (`take_prefetched_meta`).
    meta_taken: usize,
    /// Preps that the loader took, and that did not fit (`FilePrep::fits`).
    pub(crate) preps_taken: usize,
    pub(crate) preps_unfit: usize,
    /// Taken preps with the worker's import names (`FilePrep::names`).
    pub(crate) preps_named: usize,
    /// Named preps whose commit the loader took, and with none
    /// (`FilePrep::commit`).
    commits_taken: usize,
    commits_missing: usize,
    /// Worker parses with no prep when the loader needed it: the worker
    /// still resolved, or it made none.
    preps_missing: usize,
    /// Module names of taken preps with a worker answer, and with none
    /// (another mode than the worker's).
    pub(crate) names_taken: usize,
    pub(crate) names_own: usize,
    /// Per claimed job: how long it was queued, the jobs on the worker
    /// stack then, and whether a loader batch put it below the others.
    claims: Vec<(std::time::Duration, usize, bool)>,
    /// When the pool started, the workers, and their time on jobs.
    started: Option<std::time::Instant>,
    workers: usize,
    busy: std::time::Duration,
    /// parload2: when the loader took the pool (`FilesParser::parse`), the
    /// root jobs that an early pool queued (`queue_roots`), and the claims
    /// in the first `EARLY_CLAIMS` of the loader: the time and the file.
    loader_started: Option<std::time::Instant>,
    early_roots: usize,
    early_claims: Vec<(std::time::Duration, String)>,
}

/// The start of the load whose claims the debug counts list
/// (`PrefetchStats::early_claims`).
const EARLY_CLAIMS: std::time::Duration = std::time::Duration::from_millis(50);

impl PrefetchStats {
    /// Prints the counts to stderr. The sizes are read here, so only a
    /// debug run pays for them.
    fn print(&self) {
        let size = |name: &String| {
            crate::frontend::bundled::bundled_text(name).map_or_else(
                || std::fs::metadata(name).map_or(0, |m| m.len()),
                |text| text.len() as u64,
            )
        };
        let mut own: Vec<(u64, &String)> = self
            .claimed
            .iter()
            .chain(&self.not_queued)
            .chain(&self.unusable)
            .map(|name| (size(name), name))
            .collect();
        // Largest first.
        stable_sort_by(&mut own, |a, b| b.0.cmp(&a.0));
        let bytes: u64 = own.iter().map(|(size, _)| size).sum();
        eprintln!(
            "goport prefetch: loader parsed {} files ({} KB): claimed {}, not queued {}, unusable {}; took {} worker parses; untaken {} ({} KB); waited {} times ({:.1} ms); largest: {}",
            own.len(),
            bytes / 1024,
            self.claimed.len(),
            self.not_queued.len(),
            self.unusable.len(),
            self.taken,
            self.untaken,
            self.untaken_bytes / 1024,
            self.waited,
            self.wait.as_secs_f64() * 1e3,
            own.iter()
                .take(5)
                .map(|(size, name)| format!("{name} {} KB", size / 1024))
                .collect::<Vec<_>>()
                .join(", "),
        );
        eprintln!(
            "goport prefetch: took {} worker metadata; preps: took {} (named {}, committed {}, not committed {}; {} names, {} resolved here), unfit {}, missing {}",
            self.meta_taken,
            self.preps_taken,
            self.preps_named,
            self.commits_taken,
            self.commits_missing,
            self.names_taken,
            self.names_own,
            self.preps_unfit,
            self.preps_missing,
        );
        // parload1: why jobs were still queued when the loader came.
        let bucket = |age: std::time::Duration| match age.as_micros() {
            0..100 => 0,
            100..1000 => 1,
            1000..10000 => 2,
            _ => 3,
        };
        let mut ages = [[0usize; 4]; 2];
        let mut pending = [0usize; 2];
        for &(age, stack, under) in &self.claims {
            let side = usize::from(!under);
            ages[side][bucket(age)] += 1;
            pending[side] += stack;
        }
        for (side, label) in [(0, "under"), (1, "other")] {
            let count: usize = ages[side].iter().sum();
            eprintln!(
                "goport prefetch: claims {label} {count}: age <0.1ms {} <1ms {} <10ms {} >=10ms {}; worker stack mean {:.0}",
                ages[side][0],
                ages[side][1],
                ages[side][2],
                ages[side][3],
                pending[side] as f64 / count.max(1) as f64,
            );
        }
        if let (Some(started), Some(loader)) = (self.started, self.loader_started) {
            let mut early: Vec<(u64, std::time::Duration, &String)> = self
                .early_claims
                .iter()
                .map(|(at, name)| (size(name), *at, name))
                .collect();
            stable_sort_by(&mut early, |a, b| b.0.cmp(&a.0));
            eprintln!(
                "goport prefetch: pool started {:.1} ms before the loader with {} root jobs; {} claims in the first {} ms ({} KB): {}",
                loader.duration_since(started).as_secs_f64() * 1e3,
                self.early_roots,
                early.len(),
                EARLY_CLAIMS.as_millis(),
                early.iter().map(|(size, ..)| size).sum::<u64>() / 1024,
                early
                    .iter()
                    .take(5)
                    .map(|(size, at, name)| format!(
                        "{name} {} KB at {:.1} ms",
                        size / 1024,
                        at.as_secs_f64() * 1e3
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if let Some(started) = self.started {
            let wall = started.elapsed();
            eprintln!(
                "goport prefetch: {} workers busy {:.1} ms of {:.1} ms ({:.0}%)",
                self.workers,
                self.busy.as_secs_f64() * 1e3,
                wall.as_secs_f64() * 1e3,
                100.0 * self.busy.as_secs_f64() / (wall.as_secs_f64() * self.workers.max(1) as f64),
            );
        }
    }
}

/// Program loads of this process that started parse workers, and that
/// started none because the host gave every root file from its cache
/// (`FilesParser::parse`). Worker reads (`run_prefetch_worker`), and the
/// reads and their text bytes that no loader took (`PrefetchPool::drop`).
static POOL_LOADS: AtomicUsize = AtomicUsize::new(0);
static CACHED_LOADS: AtomicUsize = AtomicUsize::new(0);
static WORKER_READS: AtomicUsize = AtomicUsize::new(0);
static UNTAKEN_READS: AtomicUsize = AtomicUsize::new(0);
static UNTAKEN_BYTES: AtomicU64 = AtomicU64::new(0);

/// What the parse workers of this process read and parsed, and what of it
/// no program load took. An untaken parse keeps its nodes in the worker's
/// AST arena and its text, so `untaken` should stay 0 for loads that get
/// their files from a cache (`CompilerHost::cached_source_file_refs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefetchCounts {
    /// Program loads that started parse workers.
    pub pool_loads: usize,
    /// Program loads that started none because the host gave every root
    /// file from its cache.
    pub cached_loads: usize,
    /// Files that a worker read (and parsed, when it could).
    pub reads: usize,
    /// Reads that the loader did not take when its pool stopped.
    pub untaken: usize,
    /// The text bytes of the untaken reads.
    pub untaken_bytes: u64,
}

/// The counts of `PrefetchCounts` so far. The workers of a discarded early
/// pool (`start_default_lib_prefetch`) are not in `untaken`.
// PORT: not in Go (parse workers). For tests and debug runs.
pub fn prefetch_counts() -> PrefetchCounts {
    PrefetchCounts {
        pool_loads: POOL_LOADS.load(AtomicOrdering::Relaxed),
        cached_loads: CACHED_LOADS.load(AtomicOrdering::Relaxed),
        reads: WORKER_READS.load(AtomicOrdering::Relaxed),
        untaken: UNTAKEN_READS.load(AtomicOrdering::Relaxed),
        untaken_bytes: UNTAKEN_BYTES.load(AtomicOrdering::Relaxed),
    }
}

thread_local! {
    /// Set on the loading thread while parse workers run.
    static PREFETCH: RefCell<Option<Arc<PrefetchShared>>> = const { RefCell::new(None) };

    /// Parse workers that `start_default_lib_prefetch` started before the
    /// loader exists. The next `FilesParser::parse` on this thread takes
    /// them.
    static EARLY_POOL: RefCell<Option<PrefetchPool>> = const { RefCell::new(None) };

    /// The `LoaderStatFs` made on this thread so far: the next one gets this
    /// number (`PrefetchPool::host`).
    static LOADER_HOSTS: Cell<u64> = const { Cell::new(0) };

    /// The job of the worker parse that `take_prefetched` gave last, and
    /// the root node of the adopted parse (`TakenParse::of`).
    static TAKEN: RefCell<Option<(Arc<PrefetchJob>, Node)>> = const { RefCell::new(None) };
}

impl PrefetchShared {
    fn new(config: PrefetchConfig) -> Self {
        let jobs = Arc::new(JobMap::default());
        Self {
            queue: Mutex::new(PrefetchQueue {
                timed: std::env::var_os("GOPORT_PREFETCH_STATS").is_some(),
                move_up: move_up_enabled(),
                by_name: jobs.clone(),
                ..PrefetchQueue::default()
            }),
            jobs,
            job_map: job_map_enabled(),
            has_redirects: AtomicBool::new(false),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
            config,
            stats: Arc::new(SharedStatCache::default()),
            resolve: OnceLock::new(),
            counts: std::env::var_os("GOPORT_PREFETCH_STATS").map(|_| {
                Mutex::new(PrefetchStats {
                    started: Some(std::time::Instant::now()),
                    ..PrefetchStats::default()
                })
            }),
        }
    }

    /// Closes the queue: the workers stop after their current job.
    fn close(&self) {
        let mut queue = lock(&self.queue);
        queue.closed = true;
        queue.sleepers = 0;
        self.closed.store(true, AtomicOrdering::Relaxed);
        self.ready.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(AtomicOrdering::Relaxed)
    }

    /// Updates the debug counts, when they are kept (`GOPORT_PREFETCH_STATS`).
    fn count(&self, update: impl FnOnce(&mut PrefetchStats)) {
        if let Some(counts) = &self.counts {
            update(&mut lock(counts));
        }
    }

    /// The jobs that a worker read (and parsed) and the loader did not
    /// take, and their text bytes. Call it after the workers ended.
    fn untaken(&self) -> (usize, u64) {
        let mut untaken = (0, 0);
        self.jobs.for_each(|job| {
            if let PrefetchState::Done(Some(result)) = &*lock(&job.state) {
                untaken.0 += 1;
                untaken.1 += result.text.len() as u64;
            }
        });
        untaken
    }

    /// Asks the next free worker to move the `count` largest root files
    /// that no worker has started to the top of the queue
    /// (`rank_largest`). Call it after `FilesParser::start` queued the
    /// root tasks.
    // PERF: the loader loads imports depth first, so it needs root files
    // such as effect's `Effect.ts` (460 KB) early, while the workers take
    // the root jobs in root order and reach them last. The loader then
    // parses them itself, at 16 threads as a loader-only tail after the
    // workers are idle. The largest roots go first, so the long parses run
    // on workers in parallel. The loader order does not change, and it
    // still checks every worker parse.
    fn rank_roots(&self, tasks: &[ParseTaskRef], count: usize) {
        let mut queue = lock(&self.queue);
        let jobs: Vec<_> = tasks
            .iter()
            .filter_map(|task| queue.by_name.get(&task.borrow().normalized_file_path))
            .collect();
        if jobs.len() < 2 || count == 0 {
            return;
        }
        queue.rank = Some((jobs, count));
        self.wake(&mut queue, 1);
    }

    /// Pushes the `count` largest of `jobs` that are still queued on top of
    /// the queue, the largest last so it is taken first. A worker runs it,
    /// so the loader does not wait for the `stat` calls.
    fn rank_largest(&self, fs: &dyn Fs, jobs: Vec<Arc<PrefetchJob>>, count: usize) {
        let queued = |job: &Arc<PrefetchJob>| matches!(*lock(&job.state), PrefetchState::Queued);
        let mut sized: Vec<(i64, Arc<PrefetchJob>)> = jobs
            .into_iter()
            .filter(|job| queued(job))
            .filter_map(|job| {
                let (_, size) = self.stats.node(fs, &job.opts.file_name)?;
                Some((size, job))
            })
            .collect();
        // Stable: files of one size keep the root order.
        sized.sort_by_key(|(size, _)| *size);
        let largest = sized.split_off(sized.len().saturating_sub(count));
        let mut queue = lock(&self.queue);
        let mut pushed = 0;
        for (_, job) in largest {
            if queued(&job) {
                queue.push_pending(job);
                pushed += 1;
            }
        }
        self.wake(&mut queue, pushed);
    }

    /// `queue_locked` with its own lock of the queue, and the wake of a
    /// worker for a new job.
    #[cfg(test)]
    fn queue(&self, opts: SourceFileParseOptions, script_kind: ScriptKind) {
        let mut queue = lock(&self.queue);
        let new = Self::queue_locked(&mut queue, opts, script_kind);
        self.wake(&mut queue, usize::from(new));
    }

    /// Queues a parse of `opts.file_name`, unless one is queued already,
    /// with the queue lock held. A job that no worker has taken moves up.
    /// True when it made a new job: the caller wakes a worker for it.
    // PERF (parload2): a worker queues the files that a parse references.
    // The loader loads them right after that file (depth first), so a job
    // that a root file or an earlier parse queued, deep in the list, moves
    // up, as the loader's own batches move up the jobs it reaches
    // (`queue_batch`). Else the loader reaches it first and parses it
    // itself. `GOPORT_PREFETCH_MOVE_UP=0` turns it off.
    fn queue_locked(
        queue: &mut PrefetchQueue,
        opts: SourceFileParseOptions,
        script_kind: ScriptKind,
    ) -> bool {
        if let Some(job) = queue.by_name.get(&opts.file_name) {
            if queue.move_up
                && !queue.is_first(&job)
                && matches!(*lock(&job.state), PrefetchState::Queued)
            {
                queue.push_pending(job);
            }
            return false;
        }
        let Some(job) = queue.add(opts, script_kind, None) else {
            return false;
        };
        if !queue.is_first(&job) {
            queue.pending.push(job);
        }
        true
    }

    /// Queues the parses of one `FilesParser::start` batch: new jobs, and
    /// the jobs of files that the loader has not reached yet (`Known`),
    /// which move up. Jobs that a worker started or that the loader took
    /// stay where they are.
    // PERF: effect R3-E2. The loader loads the batch newest first, and each
    // file's imports before the next file of the batch, so it needs the
    // newest file at once. It gets there before a worker wakes, so it
    // parses that file itself anyway, and a worker that takes it only
    // makes the loader wait. So the newest job goes below the others and
    // wakes no worker; the workers take the file the loader needs next
    // first. The jobs of files that an earlier batch or a worker queued
    // move up too, so the loader does not reach them while they are deep
    // in the list. Before, the loader parsed 117 to 182 effect files itself
    // (`GOPORT_PREFETCH_STATS` "claimed"), among them root files such as
    // `Layer.ts` and `@types/node` files that were queued long before.
    fn queue_batch(&self, mut requests: Vec<PrefetchRequest>) {
        // loadpar3b step A: a request whose job a worker started or the
        // loader took needs nothing, and the loader finds that without the
        // queue lock. Port-only: Go has no prefetch queue (its
        // `taskDataByPath` `SyncMap`, filesparser.go:235, is a lock-free map
        // of a different kind, keyed by path). The lock is for the rest.
        if self.job_map {
            requests.retain(|request| !self.settled(request));
        }
        if requests.is_empty() {
            return;
        }
        let mut queue = lock(&self.queue);
        if queue.closed {
            return;
        }
        let mut jobs = Vec::with_capacity(requests.len());
        let mut woken = 0;
        for request in requests {
            let Some(job) = queue.job_of(request) else {
                continue;
            };
            if queue.is_first(&job) {
                // A worker takes it before all others.
                woken += 1;
            } else if matches!(*lock(&job.state), PrefetchState::Queued) {
                jobs.push(job);
            }
        }
        if let Some(newest) = jobs.pop() {
            woken += jobs.len();
            if queue.timed {
                newest.under.store(true, AtomicOrdering::Relaxed);
            }
            queue.push_pending(newest);
            for job in jobs {
                queue.push_pending(job);
            }
        }
        self.wake(&mut queue, woken);
    }

    /// True when the job of `request` exists, is not queued and is not the
    /// `lib.dom.d.ts` job (`PrefetchQueue::first`): `queue_batch` does
    /// nothing for it. A job never goes back to `Queued`.
    fn settled(&self, request: &PrefetchRequest) -> bool {
        let file_name = match request {
            PrefetchRequest::New(opts, _) => &opts.file_name,
            // Without the queue lock the redirects are not known.
            PrefetchRequest::Known(file_name)
                if !self.has_redirects.load(AtomicOrdering::Relaxed) =>
            {
                file_name
            }
            _ => return false,
        };
        !file_name.ends_with("/lib.dom.d.ts")
            && self
                .jobs
                .get(file_name)
                .is_some_and(|job| !matches!(*lock(&job.state), PrefetchState::Queued))
    }

    /// The job of `file_name`, if any: without the queue lock unless
    /// `GOPORT_JOB_MAP=0`.
    fn job(&self, file_name: &str) -> Option<Arc<PrefetchJob>> {
        if self.job_map {
            self.jobs.get(file_name)
        } else {
            let _queue = lock(&self.queue);
            self.jobs.get(file_name)
        }
    }

    /// Wakes the workers for `jobs` new jobs, with the queue lock held: a
    /// job wakes one sleeping worker, and no wake is made when no worker
    /// sleeps (`PrefetchQueue::sleepers`), as a wake is a syscall even with
    /// no waiter. Go runs each queued task on a goroutine of its own
    /// (core/workgroup.go:34 parallelWorkGroup.Queue), and its scheduler
    /// wakes an idle thread for a new goroutine only when a thread is idle
    /// (runtime/proc.go `wakep`). Once the workers of content-mapped jobs
    /// started, they wait on `ready` too, and one wake could reach a worker
    /// that cannot take the job, so all wake. The lock keeps a worker from
    /// starting to wait between the check and the wake.
    // PORT: not in Go (see `PrefetchJob::mapped`).
    // PERF (syscut1): a VS Code load made about 10,000 wakes with no
    // worker asleep, most of them the loader's (`queue_batch`).
    fn wake(&self, queue: &mut PrefetchQueue, jobs: usize) {
        let woken = jobs.min(queue.sleepers);
        if woken == 0 {
            return;
        }
        if queue.mapped_workers {
            queue.sleepers = 0;
            self.ready.notify_all();
        } else {
            queue.sleepers -= woken;
            for _ in 0..woken {
                self.ready.notify_one();
            }
        }
    }

    /// Queues a parse of each file in `names` that the parser can take
    /// (`parse_request`), with one lock of the queue.
    // PERF (syscut1): with a lock per name, the queue lock took 15,000 to
    // 24,000 futex calls in a VS Code load (testbox, 4 runs), and 7,000 to
    // 12,000 with one lock per call.
    fn queue_names(&self, names: Vec<String>) {
        let requests: Vec<_> = names
            .into_iter()
            .filter_map(|file_name| self.parse_request(file_name))
            .collect();
        if requests.is_empty() {
            return;
        }
        let mut queue = lock(&self.queue);
        let mut new = 0;
        for (opts, script_kind) in requests {
            new += usize::from(Self::queue_locked(&mut queue, opts, script_kind));
        }
        self.wake(&mut queue, new);
    }

    /// The parse options and script kind of a job for `file_name`, when
    /// the parser can take it: a normalized absolute name with a known
    /// extension, not a network path (ts#64832: the loader never reads one,
    /// so a worker does not either).
    fn parse_request(&self, file_name: String) -> Option<(SourceFileParseOptions, ScriptKind)> {
        let config = &self.config;
        let script_kind = get_script_kind_from_file_name(&file_name);
        if script_kind == ScriptKind::UNKNOWN
            || !has_extension(&file_name)
            || get_encoded_root_length(&file_name) == 0
            || file_name != normalize_path(&file_name)
            || crate::frontend::nativepath::is_network_path(&file_name)
        {
            return None;
        }
        // PERF (perffu1): with the program options the guess is the one of
        // `FilesParser::prefetch_request`. With the default options, a
        // file with no import or export that the loader parses with other
        // options (a `.cjs`, `.mjs`, `.cts` or `.mts` file, or any file with
        // `moduleDetection: force` or a `react-jsx` emit) was parsed again.
        // Without a worker resolver the options are not known here.
        let options = match self.resolve.get() {
            Some(Some(resolve)) => Some(&resolve.options),
            _ => None,
        };
        let path = to_path(
            &file_name,
            &config.current_directory,
            config.use_case_sensitive_file_names,
        );
        // PORT: the options are a guess (see `FilesParser::prefetch_request`).
        let external_module_indicator_options = options.map_or_else(Default::default, |options| {
            get_external_module_indicator_options(
                &file_name,
                options,
                &SourceFileMetaData::default(),
            )
        });
        Some((
            SourceFileParseOptions {
                file_name,
                path,
                external_module_indicator_options,
            },
            script_kind,
        ))
    }

    /// Queues the root files of an early pool (`start_default_lib_prefetch`)
    /// below the other jobs, the first root lowest. The loader runs its root
    /// tasks newest first, after the lib tasks, so the workers take the
    /// jobs in the order in which the loader needs them. A worker runs it,
    /// so the loading thread does not make the jobs.
    // PERF (parload2): the loader of a large program reached root files
    // such as VS Code's `vscode.d.ts` (724 KB) before any worker had
    // started them, and parsed them itself.
    fn queue_roots(&self, names: Vec<String>) {
        let requests: Vec<_> = names
            .into_iter()
            .filter_map(|name| self.parse_request(name))
            .collect();
        let mut queue = lock(&self.queue);
        let mut jobs = Vec::with_capacity(requests.len());
        for (opts, script_kind) in requests {
            // A loader that came first queued the job already.
            if queue.by_name.contains(&opts.file_name) {
                continue;
            }
            let Some(job) = queue.add(opts, script_kind, None) else {
                break;
            };
            if !queue.is_first(&job) {
                jobs.push(job);
            }
        }
        let count = jobs.len();
        queue.pending.splice(0..0, jobs);
        self.wake(&mut queue, count);
        drop(queue);
        self.count(|c| c.early_roots = count);
    }

    /// Queues the files that a parsed file references, so their parses do
    /// not wait until the loader loads that file: `/// <reference path>`
    /// and `/// <reference lib>` files, then the files that the worker
    /// resolver finds for the imports and type reference directives, or
    /// without a resolver the relative imports that name an existing TS
    /// file. This follows `ParseTask::load` (`resolve_tripleslash_path_reference`,
    /// `path_for_lib_file`, module resolution); the loader checks every
    /// guess.
    fn queue_references(
        &self,
        fs: &dyn Fs,
        refs: &FileRefs,
        resolver: Option<&WorkerResolver>,
        prep: Option<(PrepInput, &PrefetchJob)>,
    ) {
        let config = &self.config;
        let mut names = Vec::new();
        for reference in &refs.referenced_files {
            // The name that `resolve_tripleslash_path_reference` reads first
            // (ts#64159: no trailing separator).
            names.push(resolve_path_without_trailing_directory_separator(
                &get_directory_path(&refs.file_name),
                &[&reference.file_name],
            ));
        }
        if !config.default_library_path.is_empty() {
            for lib in &refs.lib_reference_directives {
                let (name, ok) = get_lib_file_name(&lib.file_name);
                if ok {
                    names.push(normalize_path(&combine_paths(
                        &config.default_library_path,
                        &[&name],
                    )));
                }
            }
        }
        match resolver {
            Some(resolver) => {
                // The references need no resolution, so they queue first.
                self.queue_names(std::mem::take(&mut names));
                let (input, job) = prep.unzip();
                // The loader may need the prep now, so it goes first.
                if let Some(prep) = resolver.resolve(self, refs, input, &mut names)
                    && let Some(job) = job
                {
                    *lock(&job.prep) = Some(Box::new(prep));
                }
            }
            None => {
                for specifier in &refs.import_specifiers {
                    if let Some(name) = guess_relative_import(fs, &refs.file_name, specifier) {
                        names.push(name);
                    }
                }
            }
        }
        self.queue_names(names);
    }
}

/// The parse workers of one program load and their jobs. Dropping it
/// closes the queue and waits for the workers.
struct PrefetchPool {
    shared: Arc<PrefetchShared>,
    threads: Vec<std::thread::JoinHandle<()>>,
    /// True once the workers of content-mapped jobs started
    /// (`FilesParser::queue_mapped_prefetch`).
    mapped_workers: bool,
    /// True when a loader took the shared resolution cache of this early
    /// pool (`early_shared_resolution`). Its workers write that cache, so
    /// `discard` waits for them.
    handed: bool,
    /// The number of the next `LoaderStatFs` on this thread when the pool
    /// started (`LOADER_HOSTS`). An early pool goes to that host
    /// (`worker_stats`).
    host: u64,
}

impl PrefetchPool {
    fn start(config: PrefetchConfig, workers: usize) -> Self {
        let mut pool = Self {
            shared: Arc::new(PrefetchShared::new(config)),
            threads: Vec::new(),
            mapped_workers: false,
            handed: false,
            host: LOADER_HOSTS.with(Cell::get),
        };
        pool.add_workers(workers);
        pool
    }

    /// Starts `workers` more parse workers on the queue of this pool.
    fn add_workers(&mut self, workers: usize) {
        self.spawn_workers(workers, false);
    }

    /// Starts `workers` workers that only take content-mapped jobs. Such a
    /// job mostly waits for the mapper, so these do not take CPU from the
    /// parses of the other workers, and the mapper gets as many requests at
    /// once as the parse goroutines of Go send.
    // PORT: not in Go (see `PrefetchJob::mapped`).
    fn add_mapped_workers(&mut self, workers: usize) {
        lock(&self.shared.queue).mapped_workers = true;
        self.spawn_workers(workers, true);
    }

    fn spawn_workers(&mut self, workers: usize, mapped_only: bool) {
        // wasm32-wasip1 has no threads, so no worker can start. Saying so
        // leaves the worker thread out of the wasm module.
        if cfg!(target_family = "wasm") {
            return;
        }
        for _ in 0..workers {
            let shared = self.shared.clone();
            // A worker that cannot start only makes the parse less parallel.
            let spawned = std::thread::Builder::new()
                .name("goport-parse".to_string())
                .stack_size(crate::gostd::stack::max_stack_size())
                .spawn(move || run_prefetch_worker(&shared, mapped_only));
            self.threads.extend(spawned.ok());
        }
    }

    /// Stops an unused pool: each worker ends after its current job. It
    /// waits for the workers only when a loader reads their resolution
    /// cache (`handed`), so that no worker writes that cache after the
    /// load, as Go waits for every parse task of a load (filesparser.go:270
    /// `w.wg.RunAndWait()`).
    fn discard(mut self) {
        self.shared.close();
        if self.handed {
            for thread in self.threads.drain(..) {
                let _ = thread.join();
            }
        }
        // Dropping a join handle detaches its thread.
        self.threads.clear();
    }
}

impl Drop for PrefetchPool {
    fn drop(&mut self) {
        self.shared.close();
        // A discarded pool has no threads here and prints no counts.
        let used = !self.threads.is_empty();
        let workers = self.threads.len();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        if !used {
            return;
        }
        let (untaken, untaken_bytes) = self.shared.untaken();
        UNTAKEN_READS.fetch_add(untaken, AtomicOrdering::Relaxed);
        UNTAKEN_BYTES.fetch_add(untaken_bytes, AtomicOrdering::Relaxed);
        self.shared.count(|c| {
            c.untaken = untaken;
            c.untaken_bytes = untaken_bytes;
            c.workers = workers;
            c.print();
        });
    }
}

/// Starts the parse workers of the next program load on this thread and
/// queues the parses of the default lib files of `config` (the root lib
/// files and the libs they reference, `lib.dom.d.ts` first), before the
/// compiler host, the build info read and the loader setup. Call it right
/// after the config parse, on the thread that loads the program. The loader
/// takes the workers when its config matches (`FilesParser::parse`) and
/// checks each parse, as for every worker parse; else they stop unused.
///
/// With `roots`, for a tsc program whose loader shares a resolution cache
/// with its workers (`WorkerResolveConfig::of_config`), the workers also
/// parse the root files and resolve their imports with that cache, which
/// the loader takes (`early_shared_resolution`). Pass it only for a host
/// with no stat cache of its own (not `tsc -b`). `GOPORT_EARLY_ROOTS=0`
/// turns it off.
// PORT: not in Go. Go starts the lib parses when `NewProgram` queues its
// root tasks; here the lib.dom parse is the longest path of small
// programs, so it starts earlier. PERF (parload2): the root parses start
// before the loader setup too.
pub fn start_default_lib_prefetch(
    config: &ParsedCommandLine,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
    default_library_path: &str,
    roots: bool,
) {
    let options = config.compiler_options();
    // Go `processAllProgramFiles` loads lib files only for a program with
    // root files and without noLib.
    if options.single_threaded.is_true()
        || options.lib_replacement.is_true()
        || !options.no_lib.is_false_or_unknown()
        || config.file_names().is_empty()
        || !parse_workers_enabled()
    {
        return;
    }
    let default_library_path =
        get_normalized_absolute_path(default_library_path, current_directory);
    // Go: fileloader.go:213 (root lib tasks)
    let mut names: Vec<String> = match &options.lib {
        Some(libs) => libs
            .iter()
            .filter_map(|lib| {
                let (name, ok) = get_lib_file_name(lib);
                ok.then_some(name)
            })
            .collect(),
        None => vec![get_default_lib_file_name(options)],
    };
    // The libs that bundled lib texts reference, so lib.dom is queued now
    // and not after the parse of the lib that references it.
    let mut index = 0;
    while index < names.len() {
        let file_name = combine_paths(&default_library_path, &[&names[index]]);
        if let Some(text) = bundled_text(&file_name) {
            for lib in bundled_lib_references(text) {
                let (name, ok) = get_lib_file_name(lib);
                if ok && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        index += 1;
    }
    let pool = PrefetchPool::start(
        PrefetchConfig {
            current_directory: current_directory.to_string(),
            use_case_sensitive_file_names,
            default_library_path: default_library_path.clone(),
        },
        prefetch_worker_count(),
    );
    // Set before the root jobs, so the workers resolve and parse them with
    // the program options.
    if roots
        && early_roots_enabled()
        && let Some(resolve) = WorkerResolveConfig::of_config(config)
    {
        let _ = pool.shared.resolve.set(Some(resolve));
        let mut queue = lock(&pool.shared.queue);
        queue.roots = Some(config.file_names().to_vec());
        pool.shared.wake(&mut queue, 1);
    }
    pool.shared.queue_names(
        names
            .iter()
            .map(|name| normalize_path(&combine_paths(&default_library_path, &[name])))
            .collect(),
    );
    // A pool that no load took stops here.
    if let Some(old) = EARLY_POOL.with(|early| early.borrow_mut().replace(pool)) {
        old.discard();
    }
}

/// False when `GOPORT_EARLY_ROOTS=0`: an early pool parses no root files
/// (`start_default_lib_prefetch`). An A/B switch.
fn early_roots_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GOPORT_EARLY_ROOTS").is_none_or(|value| value != "0"))
}

/// False when `GOPORT_PREFETCH_MOVE_UP=0`: a worker does not move up the
/// queued jobs of the files that a parse references (`PrefetchShared::queue`).
/// An A/B switch, read once, or a test's choice for the pools that start on
/// its thread (`set_move_up`).
fn move_up_enabled() -> bool {
    #[cfg(test)]
    if let Some(on) = MOVE_UP.with(Cell::get) {
        return on;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("GOPORT_PREFETCH_MOVE_UP").is_none_or(|value| value != "0"))
}

#[cfg(test)]
thread_local! {
    /// A test's choice of `move_up_enabled` for the pools of this thread.
    static MOVE_UP: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Sets `move_up_enabled` for the pools that start on this thread (tests).
#[cfg(test)]
pub(crate) fn set_move_up(on: Option<bool>) {
    MOVE_UP.with(|move_up| move_up.set(on));
}

/// The shared resolution cache of the early pool of this thread, for the
/// loader that makes its shared cache now (`process_all_program_files`),
/// when the early workers run with this loader's config: the same
/// `PrefetchConfig` and `WorkerResolveConfig::same`, as
/// `FilesParser::parse` checks before it takes the pool. Else the early
/// pool stops, so that no worker answers with another config, and the
/// loader makes a new cache. When `parse` stops a pool that gave its cache
/// here, it waits for the workers (`PrefetchPool::discard`).
// PORT: not in Go (see `start_default_lib_prefetch`).
pub(crate) fn early_shared_resolution(loader: &FileLoader) -> Option<Arc<SharedResolutionCache>> {
    EARLY_POOL.with(|early| {
        let mut early = early.borrow_mut();
        let pool = early.as_ref()?;
        let fits = match pool.shared.resolve.get() {
            Some(Some(config)) => config.shared.clone().filter(|shared| {
                loader.host.stat_cache().is_none()
                    && pool.shared.config == PrefetchConfig::of_loader(loader)
                    && WorkerResolveConfig::of_loader_with(loader, Some(shared.clone()))
                        .is_some_and(|loader_config| loader_config.same(config))
            }),
            // An early pool with no root jobs: the loader sets its config.
            _ => return None,
        };
        if fits.is_none() {
            if let Some(pool) = early.take() {
                pool.discard();
            }
        } else if let Some(pool) = early.as_mut() {
            pool.handed = true;
        }
        #[cfg(test)]
        if fits.is_some() {
            EARLY_POOLS_TAKEN.with(|taken| taken.set(taken.get() + 1));
        }
        fits
    })
}

#[cfg(test)]
thread_local! {
    /// The early shared caches that loads on this thread took, for tests.
    static EARLY_POOLS_TAKEN: Cell<usize> = const { Cell::new(0) };
}

/// The early shared caches that loads on this thread took
/// (`early_shared_resolution`), for tests.
#[cfg(test)]
pub(crate) fn early_pools_taken() -> usize {
    EARLY_POOLS_TAKEN.with(Cell::get)
}

/// The `/// <reference lib="..." />` names at the top of a bundled lib
/// text, after its license comment. A guess for `start_default_lib_prefetch`.
fn bundled_lib_references(text: &'static str) -> Vec<&'static str> {
    const LIB: &str = "<reference lib=\"";
    let rest = if text.starts_with("/*") {
        text.find("*/").map_or("", |end| &text[end + 2..])
    } else {
        text
    };
    let mut names = Vec::new();
    for line in rest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(directive) = line.strip_prefix("///") else {
            break;
        };
        if let Some(start) = directive.find(LIB) {
            let value = &directive[start + LIB.len()..];
            if let Some(end) = value.find('"') {
                names.push(&value[..end]);
            }
        }
    }
    names
}

/// Installs `PREFETCH` for the loading thread. Dropping it closes the
/// queue, so the workers stop (also when the loader panics).
struct PrefetchGuard(Arc<PrefetchShared>);

impl PrefetchGuard {
    fn install(shared: Arc<PrefetchShared>) -> Self {
        PREFETCH.with(|p| *p.borrow_mut() = Some(shared.clone()));
        Self(shared)
    }
}

impl Drop for PrefetchGuard {
    fn drop(&mut self) {
        PREFETCH.with(|p| p.borrow_mut().take());
        self.0.close();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What `queue_references` reads from a parse. It is taken before the
/// parse goes to the loader, so the loader does not wait for the
/// resolution. The workers only guess with it: the loader checks every
/// file and resolution it takes from them.
#[derive(Debug)]
pub struct FileRefs {
    file_name: String,
    referenced_files: Vec<FileReference>,
    lib_reference_directives: Vec<FileReference>,
    type_reference_directives: Vec<FileReference>,
    import_specifiers: Vec<String>,
    /// The ambient modules that the file declares: the loader does not
    /// resolve an import of one of them (ts#64491), so the worker does not.
    ambient_module_names: Vec<String>,
    /// The module indicator options that the cached parse read
    /// (`FileRefs::of_kept_parse`): the host gives the parse only for these
    /// options (`fits`). `None`: any options.
    module_indicator_options: Option<ExternalModuleIndicatorOptions>,
}

impl FileRefs {
    /// The references of a parsed file of this thread (the caches of the
    /// `tsc -b` host and the language server's project host,
    /// `CompilerHost::cached_source_file_refs`).
    pub fn of_file(file: &ParsedSourceFile) -> Self {
        FileRefs {
            file_name: file.file_name().to_string(),
            referenced_files: file.referenced_files.clone(),
            lib_reference_directives: file.lib_reference_directives.clone(),
            type_reference_directives: file.type_reference_directives.clone(),
            import_specifiers: file.imports.iter().map(|n| n.text().to_string()).collect(),
            ambient_module_names: file.ambient_module_names.clone(),
            module_indicator_options: None,
        }
    }

    /// `of_file` for a parse that a watch host keeps across a config change
    /// (`tsc --watch` and `tsc -b --watch`): the host gives it only for the
    /// module indicator options that it read, if it read them
    /// (`parse_with_options`), so a job with other options parses the file.
    pub fn of_kept_parse(file: &ParsedSourceFile) -> Self {
        FileRefs {
            module_indicator_options: crate::frontend::parser::reads_module_indicator_options(file)
                .then(|| file.parse_options().external_module_indicator_options),
            ..Self::of_file(file)
        }
    }

    /// True when the host probably gives the cached parse for a parse with
    /// module indicator options `options()`. A wrong guess only costs time:
    /// the loader checks each parse.
    fn fits(&self, options: impl FnOnce() -> ExternalModuleIndicatorOptions) -> bool {
        self.module_indicator_options
            .is_none_or(|read| read == options())
    }

    fn take_from(parse: &mut DetachedParse) -> Self {
        // The loader does not read `import_specifiers`.
        let import_specifiers = std::mem::take(&mut parse.import_specifiers);
        let file = &parse.file;
        FileRefs {
            file_name: file.file_name().to_string(),
            referenced_files: file.referenced_files.clone(),
            lib_reference_directives: file.lib_reference_directives.clone(),
            type_reference_directives: file.type_reference_directives.clone(),
            import_specifiers,
            ambient_module_names: file.ambient_module_names.clone(),
            module_indicator_options: None,
        }
    }
}

/// A job that a parse worker started (`PrefetchState::Running`). If the
/// worker panics before `finish`, the drop ends the job as `Done(None)`, so a
/// loader that waits for it (`take_prefetched`, `take_prefetched_meta`)
/// reads, parses and finds the metadata of the file itself, and meets the
/// same panic there when it is a Go panic. The worker thread ends.
// PORT: not in Go (loadpar1). Go has no parse worker: its parse task does
// this work, and a panic there is the program's panic. Here the loader does
// the work again, so a Go panic comes from the loader.
struct RunningJob<'a>(&'a PrefetchJob);

impl RunningJob<'_> {
    /// Publishes the worker's result and wakes the loaders that wait.
    fn finish(self, result: Option<PrefetchResult>) {
        let job = self.0;
        std::mem::forget(self);
        job.settle(result);
    }
}

impl Drop for RunningJob<'_> {
    fn drop(&mut self) {
        self.0.settle(None);
    }
}

/// The file whose worker job panics after it starts, for the tests of
/// `RunningJob`. Any thread's load sees it, so a test names a file of its
/// own temp dir.
#[cfg(test)]
pub(crate) static PANIC_IN_JOB: Mutex<Option<String>> = Mutex::new(None);

/// The worker transforms that loads took (`take_prefetched_mapped`), for
/// the tests: the file name, and whether a worker parse of the virtual text
/// came with the transform.
#[cfg(test)]
pub(crate) static MAPPED_TAKEN: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

/// Content-mapped files whose worker job the loader does not claim, for
/// the tests: `take_prefetched_mapped` waits up to 60 s for a worker to
/// start it, so a worker sends its transform. Any thread's load sees it,
/// so a test names files of its own temp dir.
#[cfg(test)]
pub(crate) static MAPPED_WAIT_FOR_WORKER: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A parse worker: parses queued files, newest first (the loader's queue
/// is a stack too), until the queue closes. After each parse it queues the
/// files that the parse references. The first free worker after the root
/// tasks are queued ranks them (`PrefetchShared::rank_roots`).
fn run_prefetch_worker(shared: &PrefetchShared, mapped_only: bool) {
    // Go: sys.FS() is bundled.WrapFS(osvfs.FS()). The workers share one
    // stat cache, like the Go parse tasks share the host's cachedvfs.
    let os_fs = crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs());
    let fs: Rc<dyn Fs> = Rc::new(WorkerFs {
        fs: os_fs.clone(),
        stats: shared.stats.clone(),
        keep_package_jsons: None,
    });
    let mut resolver: Option<WorkerResolver> = None;
    let mut resolver_failed = false;
    let timed = shared.counts.is_some();
    loop {
        let job = {
            let mut queue = lock(&shared.queue);
            loop {
                if queue.closed {
                    return;
                }
                if !mapped_only {
                    if let Some(job) = queue.first.take() {
                        break job;
                    }
                    if let Some(names) = queue.roots.take() {
                        drop(queue);
                        shared.queue_roots(names);
                        queue = lock(&shared.queue);
                        continue;
                    }
                    if let Some((jobs, count)) = queue.rank.take() {
                        drop(queue);
                        shared.rank_largest(&*fs, jobs, count);
                        queue = lock(&shared.queue);
                        continue;
                    }
                    if let Some(job) = queue.pending.pop() {
                        break job;
                    }
                }
                if let Some(job) = queue.mapped.pop() {
                    break job;
                }
                // A wake takes this worker off the count (`wake`).
                queue.sleepers += 1;
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        {
            let mut state = lock(&job.state);
            if !matches!(*state, PrefetchState::Queued) {
                continue;
            }
            *state = PrefetchState::Running;
        }
        let started = timed.then(std::time::Instant::now);
        let running = RunningJob(&job);
        #[cfg(test)]
        if lock(&PANIC_IN_JOB).as_deref() == Some(job.opts.file_name.as_str()) {
            panic!("test panic in the worker job of {}", job.opts.file_name);
        }
        if let Some(transform) = &job.mapped {
            running.finish(prefetch_mapped(&*fs, &job, transform));
            continue;
        }
        if resolver.is_none()
            && !resolver_failed
            && let Some(Some(config)) = shared.resolve.get()
        {
            // The resolver's file system keeps the package.json files that
            // it reads first in the load, for the loader's package.json cache
            // (`WorkerResolver::new`).
            let resolver_fs: Rc<dyn Fs> = Rc::new(WorkerFs {
                fs: os_fs.clone(),
                stats: shared.stats.clone(),
                keep_package_jsons: config.shared.clone(),
            });
            resolver = Some(WorkerResolver::new(
                config,
                resolver_fs,
                &shared.config.current_directory,
                shared.stats.host.get().is_some(),
            ));
        }
        // PERF (loadpar1): for a loader that takes worker answers, the
        // worker finds the file's metadata first, as Go `load` does, and
        // parses with the options of that metadata. A bundled lib (a lib
        // task's metadata is a constant) and a cached file get none.
        let prep_config = match (shared.resolve.get(), &resolver) {
            (Some(Some(config)), Some(_))
                if config.prep
                    && job.cached.is_none()
                    && bundled_text(&job.opts.file_name).is_none() =>
            {
                Some(config)
            }
            _ => None,
        };
        let meta = match (prep_config, &resolver) {
            (Some(config), Some(worker)) => {
                // A lookup that panics (a Go panic) panics on the loader
                // too. The worker only stops resolving.
                let meta = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.meta(config, &job.opts.file_name)
                }));
                meta.unwrap_or_else(|_| {
                    resolver = None;
                    resolver_failed = true;
                    None
                })
            }
            _ => None,
        };
        // A cached file's job reads nothing: `Done(None)` makes the loader
        // parse the file itself if it asks for it (a cache miss).
        let (result, refs, synthetic) = match &job.cached {
            Some(refs) => (None, Some(refs.clone()), None),
            None => {
                let opts =
                    meta.as_ref()
                        .zip(prep_config)
                        .map(|(meta, config)| SourceFileParseOptions {
                            external_module_indicator_options:
                                get_external_module_indicator_options(
                                    &job.opts.file_name,
                                    &config.options,
                                    &meta.meta,
                                ),
                            ..job.opts.clone()
                        });
                let read_names = prep_config.is_some_and(|config| config.names) && meta.is_some();
                let (mut result, names) = match prefetch_parse(
                    &*fs,
                    &job,
                    opts.as_ref().unwrap_or(&job.opts),
                    read_names,
                ) {
                    Some((result, names)) => (Some(result), names),
                    None => (None, None),
                };
                if let Some(result) = &mut result {
                    WORKER_READS.fetch_add(1, AtomicOrdering::Relaxed);
                    result.meta = meta.clone();
                }
                let (refs, synthetic) = match result
                    .as_mut()
                    .and_then(|result| result.parse.as_mut())
                {
                    Some(parse) => {
                        // The synthetic imports read the parse, which
                        // the loader takes.
                        let synthetic = meta.as_ref().and(resolver.as_ref()).zip(prep_config).map(
                            |(resolver, config)| {
                                resolver.synthetic_imports(config, &job.opts.path, &parse.file)
                            },
                        );
                        (
                            Some(Arc::new(FileRefs::take_from(parse))),
                            synthetic.map(|synthetic| (synthetic, names)),
                        )
                    }
                    None => (None, None),
                };
                (result, refs, synthetic)
            }
        };
        running.finish(result);
        #[cfg(test)]
        let _settled = PrepSettled(&job);
        if let Some(refs) = refs {
            let prep = meta.zip(synthetic).map(|(meta, (synthetic, names))| {
                (
                    PrepInput {
                        meta: meta.meta,
                        synthetic,
                        names,
                    },
                    &*job,
                )
            });
            // A resolution that panics (a Go panic) panics on the loader
            // too when it resolves the same import. The worker only stops
            // resolving.
            let queued = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                shared.queue_references(&*fs, &refs, resolver.as_ref(), prep);
            }));
            if queued.is_err() {
                resolver = None;
                resolver_failed = true;
            }
        }
        // A job with no references (an unread file) is busy time too.
        if let Some(started) = started {
            let busy = started.elapsed();
            shared.count(|c| c.busy += busy);
        }
    }
}

/// A parse worker's resolver: the program resolver of a Go parse task,
/// on the worker's file system.
struct WorkerResolver {
    resolver: DefaultResolver,
    options: Rc<CompilerOptions>,
    /// The project reference redirects of `WorkerResolveConfig::references`,
    /// made on first use.
    redirects: RefCell<Vec<Option<Rc<WorkerRedirect>>>>,
}

/// Go `module.ResolvedProjectReference` of a project reference, for a
/// parse worker: the reference's config name and options.
struct WorkerRedirect {
    config_name: String,
    options: Rc<CompilerOptions>,
}

impl ModuleResolvedProjectReference for WorkerRedirect {
    fn config_name(&self) -> &str {
        &self.config_name
    }

    fn compiler_options(&self) -> Option<Rc<CompilerOptions>> {
        Some(self.options.clone())
    }
}

/// Go `module.ResolutionHost` of a worker resolver.
struct WorkerResolutionHost {
    fs: Rc<dyn Fs>,
    current_directory: String,
}

impl ResolutionHost for WorkerResolutionHost {
    fn fs(&self) -> &dyn Fs {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

/// `options` for a worker resolver: the loader's resolver writes the
/// traces; a worker makes none.
fn worker_options(options: &CompilerOptions) -> Rc<CompilerOptions> {
    let mut options = options.clone();
    options.trace_resolution = Tristate::Unknown;
    Rc::new(options)
}

impl WorkerResolver {
    fn new(
        config: &WorkerResolveConfig,
        fs: Rc<dyn Fs>,
        current_directory: &str,
        log_lookups: bool,
    ) -> Self {
        let options = worker_options(&config.options);
        let host: Rc<dyn ResolutionHost> = Rc::new(WorkerResolutionHost {
            fs,
            current_directory: current_directory.to_string(),
        });
        let mut resolver = new_resolver(ResolverOptions {
            host: Some(host),
            compiler_options: Some(options.clone()),
            typings_location: config.typings_location.clone(),
            project_name: config.project_name.clone(),
            extra_extensions: config.extra_extensions.clone(),
            package_json_cache: None,
        });
        resolver.caches.shared = config.shared.clone().map(|cache| SharedResolutionLink {
            cache,
            publish: true,
        });
        // The worker keeps what it reads for each package.json entry (`fs`
        // keeps the texts, `WorkerFs::keep_package_jsons`), and the lookups
        // of the package scope walk for a file's metadata carry it. When the
        // loader takes the metadata, it keeps those reads in its own cache as
        // texts that the cache parses on the first lookup (`InfoCache::get`),
        // as the Go loader finds the metadata with the program's resolver
        // (`Caches::adopt_worker_package_jsons`). The other reads of the load
        // go into that cache at its end
        // (`SharedResolutionCache::end_package_json_reads`).
        resolver.caches.worker_package_json_reads = Some(WorkerPackageJsonReads::default());
        set_worker_lookup_log(log_lookups);
        WorkerResolver {
            resolver,
            options,
            redirects: RefCell::new(vec![None; config.references.len()]),
        }
    }

    /// The redirect of reference `index` of `config`.
    fn redirect(&self, config: &WorkerResolveConfig, index: usize) -> Rc<WorkerRedirect> {
        self.redirects.borrow_mut()[index]
            .get_or_insert_with(|| {
                let (config_name, options) = &config.references[index];
                Rc::new(WorkerRedirect {
                    config_name: config_name.clone(),
                    options: worker_options(options),
                })
            })
            .clone()
    }

    /// The project reference redirect of the file of `path`: the source
    /// file and the redirect. Go resolves the references of a source file
    /// or an output `.d.ts` file of a project reference with the
    /// reference's options, from its source file
    /// (`getRedirectForResolution`).
    fn redirect_of<'c>(
        &self,
        config: &'c WorkerResolveConfig,
        path: &Path,
    ) -> Option<(&'c str, Rc<WorkerRedirect>)> {
        config
            .redirects
            .get(path)
            .map(|(source, index)| (source.as_str(), self.redirect(config, *index)))
    }

    /// Go `loadSourceFileMetaData` of `file_name` with the program's
    /// options: the scope of its directory comes from the shared cache, or
    /// the worker finds it and stores it there with its lookup logs.
    fn meta(&self, config: &WorkerResolveConfig, file_name: &str) -> Option<WorkerMeta> {
        let cache = config.shared.as_ref()?;
        let directory = get_directory_path(file_name);
        let scope = match cache.get_scope(&directory) {
            Some(scope) => scope,
            None => {
                let caches = &self.resolver.caches;
                caches.start_package_json_log();
                let attach = caches
                    .worker_package_json_reads
                    .as_ref()
                    .map(|reads| &reads.attach);
                attach.inspect(|attach| attach.set(true));
                let value = PackageScope::of(
                    self.resolver
                        .get_package_scope_for_path(&directory)
                        .as_deref(),
                );
                attach.inspect(|attach| attach.set(false));
                let package_jsons = caches.take_package_json_log();
                let lookups = caches.take_worker_lookup_log();
                cache.store_scope(
                    &directory,
                    SharedResolution {
                        value,
                        package_jsons,
                        lookups,
                        ahead: None,
                    },
                )
            }
        };
        Some(WorkerMeta {
            meta: super::file_loader::meta_of_package_scope(scope.value, &self.options, file_name),
            package_jsons: scope.package_jsons,
            lookups: scope.lookups,
        })
    }

    /// The synthetic imports of the parse `file` of `path`, with the
    /// options of its redirect.
    fn synthetic_imports(
        &self,
        config: &WorkerResolveConfig,
        path: &Path,
        file: &ParsedSourceFile,
    ) -> SyntheticImports {
        match self.redirect_of(config, path) {
            Some((_, redirect)) => SyntheticImports::of(file, &redirect.options),
            None => SyntheticImports::of(file, &self.options),
        }
    }

    /// Resolves the type reference directives and imports of `refs` as
    /// `ParseTask::load` does (`resolve_type_reference_directives`,
    /// `resolve_imports_and_module_augmentations`), and adds to `names`
    /// the files that the loader would add. A source file or an output
    /// `.d.ts` file of a project reference resolves with the reference's
    /// options, from its source file (`redirect_of`). Stops when the queue
    /// closes.
    ///
    /// With `prep` (the file's metadata and synthetic imports), the
    /// synthetic imports resolve too, and the answers go to the loader
    /// (`FilePrep`). Each import resolves with the mode of the usage that
    /// the worker read with the parse (`PrepInput::names`, parload1). A
    /// synthetic import, or an import whose usage the worker did not read,
    /// resolves with the mode of a plain import (`guess_import_mode`).
    fn resolve(
        &self,
        shared: &PrefetchShared,
        refs: &FileRefs,
        prep: Option<PrepInput>,
        names: &mut Vec<String>,
    ) -> Option<FilePrep> {
        if prep.is_none()
            && refs.type_reference_directives.is_empty()
            && refs.import_specifiers.is_empty()
        {
            return None;
        }
        let Some(Some(config)) = shared.resolve.get() else {
            return None;
        };
        let file_name = refs.file_name.as_str();
        let redirect = if config.redirects.is_empty() {
            None
        } else {
            let path = to_path(
                file_name,
                &shared.config.current_directory,
                shared.config.use_case_sensitive_file_names,
            );
            self.redirect_of(config, &path)
        };
        let (containing_file, options) = match &redirect {
            Some((source, redirect)) => (*source, redirect.options.clone()),
            None => (file_name, self.options.clone()),
        };
        // Another module resolution kind panics in the resolver.
        if !super::file_loader::workers_resolve_imports(&options) {
            return None;
        }
        let redirect = redirect
            .as_ref()
            .map(|(_, redirect)| &**redirect as &dyn ModuleResolvedProjectReference);
        let (meta, synthetic, import_names) = match prep {
            Some(PrepInput {
                meta,
                synthetic,
                names,
            }) => (meta, Some(synthetic), names),
            // Go `loadSourceFileMetaData` reads the program's options.
            None => (
                super::file_loader::source_file_meta_data(&self.resolver, &self.options, file_name),
                None,
                None,
            ),
        };
        // parload1: with the import names, each import resolves with the
        // mode of its usage, as on the loader; the names must be those of
        // `refs` (the same parse).
        let import_names = import_names.filter(|names| {
            names.len() >= refs.import_specifiers.len()
                && refs
                    .import_specifiers
                    .iter()
                    .enumerate()
                    .all(|(index, specifier)| names.get(index).0 == specifier)
        });
        for reference in &refs.type_reference_directives {
            if shared.is_closed() {
                return None;
            }
            // Go: fileloader.go:1038 getModeForTypeReferenceDirectiveInFile
            let mode = if reference.resolution_mode != RESOLUTION_MODE_NONE {
                reference.resolution_mode
            } else {
                super::file_loader::get_default_resolution_mode_for_file(file_name, &meta, &options)
            };
            let (resolved, _) = self.resolver.resolve_type_reference_directive(
                &reference.file_name,
                containing_file,
                mode,
                redirect,
            );
            if resolved.is_resolved() {
                names.push(normalize_path(&resolved.resolved_file_name));
            }
        }
        let guessed_mode = super::file_loader::guess_import_mode(file_name, &meta, &options);
        // The shared cache keeps the first answer of each key with its
        // lookup logs: the prep takes that entry.
        let answers_from = synthetic
            .as_ref()
            .and(config.shared.as_deref())
            .map(|cache| {
                (
                    cache,
                    get_directory_path(containing_file),
                    get_redirect_config_name(redirect),
                )
            });
        let synthetic_names = synthetic
            .as_ref()
            .map_or(0, |synthetic| synthetic.names().count());
        let mut answers = Vec::with_capacity(synthetic_names + refs.import_specifiers.len());
        let mut modes = Vec::with_capacity(synthetic_names + refs.import_specifiers.len());
        // loadpar3b: the commit of the answers (`PrepCommit`), with the
        // loader's map capacity: its module names are the synthetic imports
        // and the names that the worker read.
        let mut commit = import_names
            .as_ref()
            .filter(|_| config.commit && answers_from.is_some())
            .map(|names| {
                let mut resolutions = ModeAwareCache::default();
                resolutions.reserve(synthetic_names + names.len());
                Box::new(PrepCommit {
                    resolutions,
                    adds: Vec::new(),
                })
            });
        let mut file_emit_mode = None;
        let specifiers = synthetic
            .iter()
            .flat_map(SyntheticImports::names)
            .chain(refs.import_specifiers.iter().map(String::as_str));
        for (index, specifier) in specifiers.enumerate() {
            if shared.is_closed() {
                return None;
            }
            // A synthetic import is a plain `import .. from` (its usage is
            // `ImportUsage::Other`), so the guessed mode is its mode.
            let usage = index
                .checked_sub(synthetic_names)
                .zip(import_names.as_ref())
                .map(|(import, names)| names.get(import));
            let mode = match usage {
                Some((_, usage, _, _)) => super::file_loader::mode_for_import_usage(
                    usage,
                    file_name,
                    &meta,
                    Some(&options),
                    &mut file_emit_mode,
                ),
                None => guessed_mode,
            };
            modes.push(mode);
            // ts#63915: the loader does not resolve a source phase import.
            if specifier.is_empty()
                || usage.is_some_and(|(_, _, _, source_phase)| source_phase == Some(true))
            {
                answers.push(None);
                continue;
            }
            // ts#64491: nor an ambient module that the file declares; it
            // maps the name to an empty resolution (fileloader.go:887).
            if refs
                .ambient_module_names
                .iter()
                .any(|name| name == specifier)
            {
                if let Some(commit) = &mut commit {
                    commit.resolutions.insert(
                        ModeAwareCacheKey {
                            name: specifier.to_string(),
                            mode,
                        },
                        Arc::new(ResolvedModule::default()),
                    );
                }
                answers.push(None);
                continue;
            }
            // The worker's resolver is the default resolver, which never
            // fails (Go resolver.go:469 gives a nil error). So no answer
            // has an error for the loader to note (Go
            // `moduleResolutionError`, fileloader.go:901).
            let (resolved, _, err) =
                self.resolver
                    .resolve_module_name(specifier, containing_file, mode, redirect);
            debug_assert!(err.is_none(), "the default resolver failed on {specifier}");
            if let Some((cache, directory, redirect_name)) = &answers_from {
                let answer = cache.get_module(&(
                    directory.as_str(),
                    specifier,
                    mode,
                    redirect_name.as_str(),
                ));
                // The commit reads the stored answer, which the loader
                // takes, not `resolved`.
                if let Some(committed) = &mut commit {
                    let adds = usage.is_none_or(|(_, _, adds, _)| adds != Some(false));
                    if !committed.push(index, specifier, mode, answer.as_ref(), adds, &options) {
                        commit = None;
                    }
                }
                answers.push(answer);
            }
            if !resolved.is_resolved() {
                continue;
            }
            // The loader adds a JS file only with allowJs, and not from
            // node_modules at the default depth (`should_add_file`).
            let resolved_file_name = &resolved.resolved_file_name;
            if !file_extension_is_one_of(resolved_file_name, SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT)
                && (!options.get_allow_js() || resolved.is_external_library_import)
            {
                continue;
            }
            names.push(normalize_path(resolved_file_name));
        }
        let (_, _, redirect) = answers_from?;
        Some(FilePrep {
            meta,
            redirect,
            containing_file: containing_file.to_string(),
            synthetic: synthetic?,
            imports: refs.import_specifiers.len(),
            modes,
            answers,
            names: import_names,
            commit,
        })
    }
}

/// What a parse worker knows of its file for the prep (`FilePrep`) before
/// it resolves: the metadata and the synthetic imports.
struct PrepInput {
    meta: SourceFileMetaData,
    synthetic: SyntheticImports,
    /// The import names of the parse (`FilePrep::names`), when the worker
    /// read them.
    names: Option<Box<ImportNames>>,
}

/// The lookups that Go `cachedvfs` caches (all but `Stat`), in maps that
/// other threads can read. The parse workers of one load share one
/// (`SharedStatCache`). The `tsc -b` host's cached file system keeps its
/// cache in them too (`BuildStatCache`).
#[derive(Default)]
pub struct StatCache {
    file_exists: StatMap<bool>,
    directory_exists: StatMap<bool>,
    realpath: StatMap<String>,
    entries: StatMap<Entries>,
}

/// The shards of a `StatMap`.
const STAT_MAP_SHARDS: usize = 64;

/// How long a lookup waits for the thread that loads the same path
/// (`StatMap::get_or_load`) before it loads the path itself.
const STAT_MAP_WAIT: std::time::Duration = std::time::Duration::from_micros(20);

/// Go `collections.SyncMap` of one `cachedvfs` lookup, for threads: one map
/// per shard of the path hash. The parse workers of a directory ask for
/// the same package.json scope and `node_modules` lookups together, so a
/// miss marks its path as loading (`StatSlot::Loading`): a thread that asks
/// for that path meanwhile waits for the answer (`STAT_MAP_WAIT`, spinning:
/// a lock wait costs more than the call) and makes no call of its own.
// PORT: Go `cachedvfs` loads outside the map (cachedvfs.go:56, in
// `DirectoryExists` at :49), so two goroutines can both call the file
// system for one path. The answer is the same.
struct StatMap<V> {
    shards: Box<[Mutex<FxHashMap<String, StatSlot<V>>>]>,
}

enum StatSlot<V> {
    Ready(V),
    /// A thread loads the value now. It sets the cell, then makes the slot
    /// `Ready`.
    Loading(Arc<OnceLock<V>>),
}

impl<V: Clone> StatSlot<V> {
    fn value(&self) -> Option<V> {
        match self {
            StatSlot::Ready(value) => Some(value.clone()),
            StatSlot::Loading(cell) => cell.get().cloned(),
        }
    }
}

impl<V> Default for StatMap<V> {
    fn default() -> Self {
        StatMap {
            shards: (0..STAT_MAP_SHARDS).map(|_| Mutex::default()).collect(),
        }
    }
}

impl<V: Clone> StatMap<V> {
    fn shard(&self, path: &str) -> &Mutex<FxHashMap<String, StatSlot<V>>> {
        use std::hash::BuildHasher;
        const _: () = assert!(STAT_MAP_SHARDS == 64);
        let hash = rustc_hash::FxBuildHasher.hash_one(path);
        &self.shards[crate::frontend::module::shard_index(hash)]
    }

    /// The cached value of `path`.
    fn get(&self, path: &str) -> Option<V> {
        lock(self.shard(path)).get(path).and_then(StatSlot::value)
    }

    /// The cached value of `path`, or `load()` stored as it.
    fn get_or_load(&self, path: &str, load: impl FnOnce() -> V) -> V {
        let loading = {
            let mut shard = lock(self.shard(path));
            match shard.get(path) {
                Some(StatSlot::Ready(value)) => return value.clone(),
                Some(StatSlot::Loading(cell)) => Some(cell.clone()),
                None => {
                    shard.insert(path.to_string(), StatSlot::Loading(Arc::default()));
                    None
                }
            }
        };
        if let Some(cell) = loading {
            let start = std::time::Instant::now();
            loop {
                if let Some(value) = cell.get() {
                    return value.clone();
                }
                for _ in 0..32 {
                    std::hint::spin_loop();
                }
                if start.elapsed() > STAT_MAP_WAIT {
                    // The loading thread is slow (or descheduled): the
                    // answer is the same.
                    return load();
                }
            }
        }
        let value = load();
        self.insert_if_absent(path, value)
    }

    /// Stores `value` as the value of `path` when there is none (or only
    /// a load of it). Returns the value that the map has.
    fn insert_if_absent(&self, path: &str, value: V) -> V {
        let mut shard = lock(self.shard(path));
        match shard.get_mut(path) {
            Some(StatSlot::Ready(old)) => old.clone(),
            Some(slot) => {
                if let StatSlot::Loading(cell) = slot {
                    let _ = cell.set(value.clone());
                }
                *slot = StatSlot::Ready(value.clone());
                value
            }
            None => {
                shard.insert(path.to_string(), StatSlot::Ready(value.clone()));
                value
            }
        }
    }

    fn clear(&self) {
        for shard in &self.shards {
            lock(shard).clear();
        }
    }
}

/// Stores the value of `path` in `from` in `to`, when `to` has none.
fn copy_stat<V: Clone>(from: &StatMap<V>, to: &StatMap<V>, path: &str) {
    if let Some(value) = from.get(path) {
        to.insert_if_absent(path, value);
    }
}

/// Stores each value of `from` in `to`, when `to` has none for its path.
/// `from` is empty after.
fn merge_stats<V: Clone>(from: &StatMap<V>, to: &StatMap<V>) {
    for shard in &from.shards {
        let slots = std::mem::take(&mut *lock(shard));
        for (path, slot) in slots {
            if let Some(value) = slot.value() {
                to.insert_if_absent(&path, value);
            }
        }
    }
}

impl StatCache {
    /// The cached `FileExists(path)`, or `load()` stored as it.
    pub fn file_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        self.file_exists.get_or_load(path, load)
    }

    /// The cached `DirectoryExists(path)`, or `load()` stored as it.
    pub fn directory_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        self.directory_exists.get_or_load(path, load)
    }

    /// The cached `Realpath(path)`, or `load()` stored as it.
    pub fn realpath(&self, path: &str, load: impl FnOnce() -> String) -> String {
        self.realpath.get_or_load(path, load)
    }

    /// The cached `GetAccessibleEntries(path)`, or `load()` stored as it.
    pub fn entries(&self, path: &str, load: impl FnOnce() -> Entries) -> Entries {
        self.entries.get_or_load(path, load)
    }

    /// Stores the value of `lookup` in `from` here, when this cache has
    /// none.
    fn copy_from(&self, from: &StatCache, lookup: &StatLookup) {
        let path = lookup.path.as_str();
        match lookup.kind {
            StatKind::FileExists => copy_stat(&from.file_exists, &self.file_exists, path),
            StatKind::DirectoryExists => {
                copy_stat(&from.directory_exists, &self.directory_exists, path);
            }
            StatKind::Realpath => copy_stat(&from.realpath, &self.realpath, path),
            StatKind::Entries => copy_stat(&from.entries, &self.entries, path),
        }
    }

    /// Go `cachedvfs.FS.ClearCache` for these lookups.
    pub fn clear(&self) {
        self.file_exists.clear();
        self.directory_exists.clear();
        self.realpath.clear();
        self.entries.clear();
    }
}

/// The lookup cache of the `tsc -b` host's file system
/// (`CompilerHost::stat_cache`), shared with the parse workers of its
/// program loads.
///
/// PORT: Go parse tasks share the host's cachedvfs, which lasts for the
/// whole build, and a write does not update it (cachedvfs.go:145). So each
/// lookup that a program makes before the build writes that path stays
/// for the later programs. The parse workers here also resolve guesses:
/// imports with a guessed resolution mode, files that the loader may not
/// load. A lookup of a guess must not stay, because Go does not make it.
/// So the workers read `cached` and keep their own lookups in `load`,
/// which the load clears at its end (`end_load`). What stays in `cached`
/// is what Go caches:
/// - the lookups of the host's file system (the loader and the
///   orchestrator). A lookup that a worker of the load made already is
///   taken from `load` (the build writes nothing during a load);
/// - the lookups of the worker answers that the loader takes
///   (`SharedResolution::lookups`), which the load adds at its end;
/// - the lookups of the config matches that config threads made for the
///   orchestrator (`add`).
#[derive(Default)]
pub struct BuildStatCache {
    /// Go `cachedvfs`: kept for the whole build.
    cached: StatCache,
    /// The lookups of the parse workers of the load that runs now, which
    /// `cached` did not have.
    load: StatCache,
}

impl BuildStatCache {
    /// A lookup of the host's file system: the value in `cached`, else the
    /// value that a worker of this load found, else `load()`, stored in
    /// `cached`, as Go `cachedvfs` stores each lookup.
    fn host_lookup<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &StatMap<V>,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        let cached = map(&self.cached);
        if let Some(value) = cached.get(path) {
            return value;
        }
        let value = map(&self.load).get(path).unwrap_or_else(load);
        cached.insert_if_absent(path, value)
    }

    /// A parse worker's lookup: the value in `cached`, else in `load`, else
    /// `load()` stored in `load`. A lookup that `cached` does not have goes
    /// into the log of the resolution that runs (`note_worker_lookup`).
    fn worker_lookup<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &StatMap<V>,
        kind: StatKind,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        if let Some(value) = map(&self.cached).get(path) {
            return value;
        }
        note_worker_lookup(kind, path);
        map(&self.load).get_or_load(path, load)
    }

    /// The host's `FileExists(path)` (see `host_lookup`).
    pub fn file_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        self.host_lookup(|c| &c.file_exists, path, load)
    }

    /// The host's `DirectoryExists(path)` (see `host_lookup`).
    pub fn directory_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        self.host_lookup(|c| &c.directory_exists, path, load)
    }

    /// The host's `Realpath(path)` (see `host_lookup`).
    pub fn realpath(&self, path: &str, load: impl FnOnce() -> String) -> String {
        self.host_lookup(|c| &c.realpath, path, load)
    }

    /// The host's `GetAccessibleEntries(path)` (see `host_lookup`).
    pub fn entries(&self, path: &str, load: impl FnOnce() -> Entries) -> Entries {
        self.host_lookup(|c| &c.entries, path, load)
    }

    /// Moves the lookups in `lookups` that `cached` does not have into
    /// `cached`: the lookups that another thread made for the host's file
    /// system, when the host takes that thread's result
    /// (config_prefetch.rs). `lookups` is empty after.
    pub fn add(&self, lookups: &StatCache) {
        merge_stats(&lookups.file_exists, &self.cached.file_exists);
        merge_stats(&lookups.directory_exists, &self.cached.directory_exists);
        merge_stats(&lookups.realpath, &self.cached.realpath);
        merge_stats(&lookups.entries, &self.cached.entries);
    }

    /// Starts a program load whose parse workers use this cache.
    fn start_load(&self) {
        self.load.clear();
    }

    /// Ends a program load, after its parse workers ended: adds to `cached`
    /// the lookups of the worker answers that the loader took
    /// (`DefaultResolver::take_worker_lookups`), and drops the other worker
    /// lookups.
    pub fn end_load(&self, taken: &[Arc<[StatLookup]>]) {
        for lookup in taken.iter().flat_map(|lookups| lookups.iter()) {
            self.cached.copy_from(&self.load, lookup);
        }
        self.load.clear();
    }

    /// Go `cachedvfs.FS.ClearCache` for these lookups.
    pub fn clear(&self) {
        self.cached.clear();
        self.load.clear();
    }
}

/// Go `cachedvfs` for the parse workers: the lookups that `CachedFs`
/// caches, shared by the workers of one load. Module resolution and import
/// guesses ask for the same paths many times.
///
/// PORT: Go parse tasks share the host's cachedvfs. When the loader's host
/// keeps its cache in a `BuildStatCache` (`CompilerHost::stat_cache`, the
/// `tsc -b` host), the workers use that cache (`host`) in place of `own`:
/// they read what the build has cached, and the host's file system reads
/// what they found.
#[derive(Default)]
pub(crate) struct SharedStatCache {
    own: StatCache,
    host: OnceLock<Arc<BuildStatCache>>,
    /// The package.json texts that the workers' resolvers read
    /// (`WorkerFs::read_file`), so that each is read once in the load. Go
    /// reads each once into the resolver's package.json cache, which all
    /// parse tasks share; each worker resolver here has its own cache.
    package_jsons: StatMap<(Arc<str>, bool)>,
    /// The OS stat of each path that a `FileExists`, `DirectoryExists` or
    /// root file size (`PrefetchShared::rank_largest`) asked for in the load,
    /// as (is a directory, size), when there is no host cache. Go
    /// `vfs.Common` answers each of the two lookups with a stat
    /// (internal.go:59, :64), so here one stat answers all three. Not for
    /// the lookups of a bundled path (`exists`).
    // PORT: Go `cachedvfs` caches `DirectoryExists` and `FileExists` in two
    // maps (cachedvfs.go:49, :65), so Go makes one stat per lookup kind: 2
    // stats for a path that it asks both of. Here the first lookup of a
    // path fixes both answers for the load, and for the program's symlink
    // cache, which reads them after the load (`HostStatCache::Load`,
    // `HostStatFs`). So the port makes fewer stats than Go. The answers
    // are Go's unless the file system changes between Go's two stats of
    // one path. The load writes no file, so the output of the load is the
    // same.
    nodes: StatMap<Option<(bool, i64)>>,
}

impl SharedStatCache {
    /// The cached value of `path` in the host's cache (when there is one,
    /// `BuildStatCache::worker_lookup`) or else in the workers' own cache,
    /// or `load()` stored there.
    fn lookup<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &StatMap<V>,
        kind: StatKind,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        match self.host.get() {
            Some(host) => host.worker_lookup(map, kind, path, load),
            None => map(&self.own).get_or_load(path, load),
        }
    }

    /// The OS stat of `path` on `fs` (see `nodes`).
    fn node(&self, fs: &dyn Fs, path: &str) -> Option<(bool, i64)> {
        self.nodes.get_or_load(path, || {
            fs.stat(path).map(|info| (info.is_dir(), info.size()))
        })
    }

    /// `FileExists` (or with `directory`, `DirectoryExists`) of `path` on
    /// `fs`, the bundled wrapper of the OS file system. For an OS path it is
    /// Go `vfs.Common.FileExists` or `DirectoryExists`, each a stat, so one
    /// stat in `nodes` answers both (see the PORT note there). A bundled
    /// path is answered by `fs` itself (no syscall): there the stat and the
    /// lookups differ (Go embed.go:66 and :85: `Stat("bundled:///")` is a
    /// directory, but `DirectoryExists("bundled:///")` is false). `None`
    /// with a host cache: its lookups of each kind stay for later programs,
    /// so they must not answer each other (`BuildStatCache`).
    fn exists(&self, fs: &dyn Fs, path: &str, directory: bool) -> Option<bool> {
        if self.host.get().is_some() {
            return None;
        }
        if crate::frontend::bundled::is_bundled(path) {
            return Some(if directory {
                fs.directory_exists(path)
            } else {
                fs.file_exists(path)
            });
        }
        Some(
            self.node(fs, path)
                .is_some_and(|(is_dir, _)| is_dir == directory),
        )
    }
}

/// A parse worker's file system: the OS file system of its thread with the
/// shared stat cache.
struct WorkerFs {
    fs: Rc<dyn Fs>,
    stats: Arc<SharedStatCache>,
    /// The file system of the worker's resolver: it keeps the text of each
    /// package.json that it reads and the load has no read of yet
    /// (`note_worker_package_json_read`, `SharedResolutionCache::has_package_json_read`).
    keep_package_jsons: Option<Arc<SharedResolutionCache>>,
}

impl Fs for WorkerFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        if let Some(exists) = self.stats.exists(&*self.fs, path, false) {
            return exists;
        }
        self.stats.lookup(
            |c| &c.file_exists,
            StatKind::FileExists,
            path,
            || self.fs.file_exists(path),
        )
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        if !path.ends_with("/package.json") {
            return self.fs.read_file(path);
        }
        let (text, ok) = self.stats.package_jsons.get_or_load(path, || {
            let (text, ok) = self.fs.read_file(path);
            (Arc::from(text), ok)
        });
        let read = (text.to_string(), ok);
        if let Some(shared) = &self.keep_package_jsons
            && !shared.has_package_json_read(path)
        {
            note_worker_package_json_read(path, &read.0);
        }
        read
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<std::time::SystemTime>,
        m_time: Option<std::time::SystemTime>,
    ) -> Result<(), FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        if let Some(exists) = self.stats.exists(&*self.fs, path, true) {
            return exists;
        }
        self.stats.lookup(
            |c| &c.directory_exists,
            StatKind::DirectoryExists,
            path,
            || self.fs.directory_exists(path),
        )
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.stats.lookup(
            |c| &c.entries,
            StatKind::Entries,
            path,
            || self.fs.get_accessible_entries(path),
        )
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.fs.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        self.stats.lookup(
            |c| &c.realpath,
            StatKind::Realpath,
            path,
            || self.fs.realpath(path),
        )
    }
}

/// The base file system of a compiler host on the plain OS file system
/// (`new_cached_fs_compiler_host`, under its `cachedvfs`): its
/// `FileExists`, `DirectoryExists`, `Realpath` and `GetAccessibleEntries`
/// calls on this thread go through the stat cache of the parse workers of
/// this thread's load or of its own load's early pool (`worker_stats`,
/// `SharedStatCache::own`), as Go parse tasks and the loader share the
/// host's cachedvfs. The loader then makes no call that a worker made
/// already, and a worker makes none that the loader made (for example the
/// root file checks of `addRootFileTask`, which the workers' import
/// lookups ask for again). Every other call goes to `fs`.
// PORT: not in Go (see `SharedStatCache`). The workers read the same OS
// file system (`CompilerHost::is_plain_os_fs`), so a worker's answer is
// the answer of `fs`.
pub(crate) struct LoaderStatFs {
    fs: Rc<dyn Fs>,
    /// The number of this host on its thread (`LOADER_HOSTS`).
    host: u64,
}

impl LoaderStatFs {
    pub(crate) fn new(fs: Rc<dyn Fs>) -> Self {
        let host = LOADER_HOSTS.with(|hosts| {
            let host = hosts.get();
            hosts.set(host + 1);
            host
        });
        LoaderStatFs { fs, host }
    }

    /// `load()`, or the value of `path` in the workers' cache of this
    /// thread's load (else of its early pool), where `load()` is stored
    /// when the cache has none. Only a cache of the workers' own (no
    /// `tsc -b` host cache).
    fn shared<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &StatMap<V>,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        match worker_stats(self.host) {
            Some(stats) => map(&stats.own).get_or_load(path, load),
            None => load(),
        }
    }

    /// `FileExists` (or with `directory`, `DirectoryExists`) of `path`
    /// (see `SharedStatCache::exists`), else the call on `fs`.
    fn exists(&self, path: &str, directory: bool) -> bool {
        if let Some(exists) =
            worker_stats(self.host).and_then(|stats| stats.exists(&*self.fs, path, directory))
        {
            return exists;
        }
        if directory {
            self.fs.directory_exists(path)
        } else {
            self.fs.file_exists(path)
        }
    }
}

/// The stat cache of the parse workers of this thread's load, else of the
/// early pool of `host`'s load (`host` is a `LoaderStatFs` number), when
/// the cache is their own (no `tsc -b` host cache).
///
/// An early pool's cache keeps the OS answers from when its workers ran.
/// Each flow that starts an early pool makes its host next and loads with
/// that host, and the load takes the pool (`FilesParser::parse`), except
/// `tsc -b`: it starts an early pool but makes no `LoaderStatFs` (its host
/// is `new_compiler_host_over`), and its load gives the pool the build
/// host's stat cache, which this does not give. So the pool's answers are
/// those of the host's own load. An early pool goes
/// only to the first host made after it (`PrefetchPool::host`): a later
/// host does not read the pool of a host that never loaded. A flow that
/// makes no host after it starts a pool leaves the pool to the next host
/// on the thread, until a load takes the pool or a new pool replaces it.
fn worker_stats(host: u64) -> Option<Arc<SharedStatCache>> {
    let shared = PREFETCH.with(|p| p.borrow().clone()).or_else(|| {
        EARLY_POOL.with(|early| {
            early
                .borrow()
                .as_ref()
                .filter(|pool| pool.host == host)
                .map(|pool| pool.shared.clone())
        })
    })?;
    shared
        .stats
        .host
        .get()
        .is_none()
        .then(|| shared.stats.clone())
}

impl Fs for LoaderStatFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.exists(path, false)
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        self.fs.read_file(path)
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<std::time::SystemTime>,
        m_time: Option<std::time::SystemTime>,
    ) -> Result<(), FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.exists(path, true)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.shared(
            |c| &c.entries,
            path,
            || self.fs.get_accessible_entries(path),
        )
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.fs.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        self.shared(|c| &c.realpath, path, || self.fs.realpath(path))
    }
}

/// The thread-safe lookup cache of a program host's file system, which a
/// thread other than the loading one can read (`HostStatFs`).
// PORT: not in Go, where each goroutine reads the host's `cachedvfs`
// (`p.Host().FS()`). The port's `tsc` host caches in `CachedFs`, which
// only the loading thread reads. These are the caches that it shares.
#[derive(Clone)]
pub(crate) enum HostStatCache {
    /// The lookups that the loader (`LoaderStatFs`) and the parse workers
    /// of a load on the plain OS file system made (`SharedStatCache`).
    /// They answer for the host: every answer is the OS answer, and the
    /// load writes no file. Empty for a load that made none that another
    /// thread can read (`empty`).
    Load(Arc<SharedStatCache>),
    /// The cache of the `tsc -b` host (`CompilerHost::stat_cache`).
    Build(Arc<BuildStatCache>),
}

impl HostStatCache {
    /// No lookups, for a load on the plain OS file system with no parse
    /// workers (as with `--singleThreaded`): its lookups are in the host's
    /// `CachedFs`, which only the loading thread reads. A `HostStatFs` on
    /// it makes each lookup that it needs, once.
    pub(crate) fn empty() -> Self {
        HostStatCache::Load(Arc::default())
    }

    /// Lets go of these lookups. A load's lookups go to the free thread
    /// (`drop_in_background`, which the load started), so this thread does
    /// not free them; the `tsc -b` host keeps its own.
    pub(crate) fn free(self) {
        if let HostStatCache::Load(stats) = self {
            crate::execute::build::build_task::drop_in_background(stats);
        }
    }
}

/// The lookups of a program's load (`HostStatCache::Load`), until the
/// thread that finishes the program's symlink cache takes them
/// (program/go_frontend.rs `SymlinkSeed`). A copy of the files (Go
/// `UpdateProgram`) has none: the file system can change after the load.
#[derive(Default)]
pub struct LoadStatCache(Cell<Option<HostStatCache>>);

impl LoadStatCache {
    pub(crate) fn take(&self) -> Option<HostStatCache> {
        self.0.take()
    }
}

impl Clone for LoadStatCache {
    fn clone(&self) -> Self {
        LoadStatCache::default()
    }
}

/// The OS file system of this thread with the `FileExists`,
/// `DirectoryExists`, `Realpath` and `GetAccessibleEntries` lookups of a
/// program's host (`HostStatCache`): a lookup that the host made is not
/// made again, and a new one is stored there, as Go `cachedvfs` does for
/// each goroutine (cachedvfs.go:49, :65). Every other call goes to the OS
/// file system.
pub(crate) struct HostStatFs {
    fs: Rc<dyn Fs>,
    cache: HostStatCache,
}

impl HostStatFs {
    pub(crate) fn new(cache: HostStatCache) -> Self {
        HostStatFs {
            fs: crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs()),
            cache,
        }
    }

    /// `FileExists` (or with `directory`, `DirectoryExists`) of `path`.
    fn exists(&self, path: &str, directory: bool) -> bool {
        let load = || {
            if directory {
                self.fs.directory_exists(path)
            } else {
                self.fs.file_exists(path)
            }
        };
        match &self.cache {
            HostStatCache::Load(stats) => stats
                .exists(&*self.fs, path, directory)
                .unwrap_or_else(load),
            HostStatCache::Build(stats) if directory => stats.directory_exists(path, load),
            HostStatCache::Build(stats) => stats.file_exists(path, load),
        }
    }
}

impl Fs for HostStatFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.exists(path, false)
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        self.fs.read_file(path)
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<std::time::SystemTime>,
        m_time: Option<std::time::SystemTime>,
    ) -> Result<(), FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.exists(path, true)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let load = || self.fs.get_accessible_entries(path);
        match &self.cache {
            HostStatCache::Load(stats) => stats.own.entries(path, load),
            HostStatCache::Build(stats) => stats.entries(path, load),
        }
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.fs.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        let load = || self.fs.realpath(path);
        match &self.cache {
            HostStatCache::Load(stats) => stats.own.realpath(path, load),
            HostStatCache::Build(stats) => stats.realpath(path, load),
        }
    }
}

#[cfg(test)]
mod host_stat_fs_tests {
    use super::*;

    // syscut1 round 2: the symlink cache's resolver on another thread
    // reads the lookups of the program's load and makes no call that the
    // load made (Go: the host's `cachedvfs`), and stores its own there.
    #[test]
    fn host_stat_fs_answers_from_the_load() {
        let dir = std::env::temp_dir().join(format!("goport-host-stat-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.to_string_lossy().replace('\\', "/");
        let gone = format!("{dir}/node_modules/gone");
        let stats = Arc::new(SharedStatCache::default());
        // The load found `gone`, which does not exist now: an answer from
        // the cache shows that no call was made.
        stats.nodes.insert_if_absent(&gone, Some((true, 0)));
        stats
            .own
            .realpath
            .insert_if_absent(&gone, format!("{dir}/real/gone"));
        let fs = HostStatFs::new(HostStatCache::Load(Arc::clone(&stats)));
        assert!(fs.directory_exists(&gone));
        assert!(!fs.file_exists(&gone));
        assert_eq!(fs.realpath(&gone), format!("{dir}/real/gone"));
        // A new lookup is stored for the next one.
        let here = format!("{dir}/node_modules");
        assert!(!fs.directory_exists(&here));
        assert_eq!(stats.nodes.get(&here), Some(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The TS file that a relative import (`./x`, `../x.js`) of `containing`
/// most likely resolves to: the first existing file in module resolution's
/// extension order. `None` for other specifiers.
fn guess_relative_import(fs: &dyn Fs, containing: &str, specifier: &str) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }
    let base = normalize_path(&combine_paths(
        &get_directory_path(containing),
        &[specifier],
    ));
    let with = |stem: &str, extensions: &[&str]| -> Vec<String> {
        extensions
            .iter()
            .map(|e| normalize_path(&format!("{stem}{e}")))
            .collect()
    };
    let candidates = if let Some(stem) = base.strip_suffix(EXTENSION_JS) {
        with(stem, &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_DTS])
    } else if let Some(stem) = base.strip_suffix(EXTENSION_MJS) {
        with(stem, &[EXTENSION_MTS, EXTENSION_DMTS])
    } else if let Some(stem) = base.strip_suffix(EXTENSION_CJS) {
        with(stem, &[EXTENSION_CTS, EXTENSION_DCTS])
    } else if [EXTENSION_TS, EXTENSION_TSX, EXTENSION_MTS, EXTENSION_CTS]
        .iter()
        .any(|e| base.ends_with(e))
    {
        vec![base]
    } else if get_base_file_name(&base).contains('.') {
        return None;
    } else {
        let index = combine_paths(&base, &["index"]);
        let mut candidates = with(&base, &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_DTS]);
        candidates.extend(with(&index, &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_DTS]));
        candidates
    };
    candidates.into_iter().find(|name| fs.file_exists(name))
}

/// Reads and parses the file of `job` with `opts` (the job's file name and
/// path) into a detached store. The parse is usable only when it made no
/// thread-local state that the loading thread would need (synthetic nodes,
/// node ids) and did not panic.
fn prefetch_parse(
    fs: &dyn Fs,
    job: &PrefetchJob,
    opts: &SourceFileParseOptions,
    read_names: bool,
) -> Option<(PrefetchResult, Option<Box<ImportNames>>)> {
    // A bundled lib text is embedded, so it needs no copy. Another text is
    // leaked, as the loader's is, except the text of a freeable file
    // version (`FileText::new`).
    let text: FileText = match crate::frontend::bundled::bundled_text(&job.opts.file_name) {
        Some(text) => text.into(),
        None => {
            let (text, ok) = fs.read_file(&job.opts.file_name);
            if !ok {
                return None;
            }
            FileText::new(text, job.freeable)
        }
    };
    let before = (synthetic_slot_count(), next_ids());
    let parse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The parse of a freeable file version owns its nodes, as the
        // loader's parse of it does (`CompilerHostImpl::get_source_file`), so
        // the version frees them, and a parse that the loader does not take
        // is freed with the pool.
        let _owned_nodes = job.freeable.then(crate::ast::enter_freeable_parse);
        // parload1: the import names are read while the nodes are on this
        // thread.
        parse_source_file_detached_reading(job.job, opts, text.clone(), job.script_kind, |file| {
            read_names.then(|| Box::new(ImportNames::of(file)))
        })
    }));
    let (parse, names) = match parse {
        Ok((parse, names)) => {
            let usable =
                parse.store.is_self_contained() && (synthetic_slot_count(), next_ids()) == before;
            (usable.then_some(parse), names.filter(|_| usable))
        }
        Err(_) => {
            let _ = take_detached_file_store();
            (None, None)
        }
    };
    let result = PrefetchResult {
        text,
        parse,
        meta: None,
        mapped: None,
    };
    Some((result, names))
}

/// A parse worker's transform of a content-mapped file: the transform
/// request that `transform_locked` sends, and the parse of the virtual text
/// that `contentmapper::parse_result` would make. As in Go, the parse runs
/// only for a result whose mappings and virtual extension pass the checks
/// before it (`contentmapper::parses_canonical_output`). The loader takes both
/// (`take_prefetched_mapped`): it reports an error and attaches the other
/// outputs, so each file gets one request, as in Go. `None` when the file
/// cannot be read (Go sends no request then either), or when the loader
/// must send the request itself (`ConcurrentTransform::transform`).
// PORT: not in Go (see `PrefetchJob::mapped`).
fn prefetch_mapped(
    fs: &dyn Fs,
    job: &PrefetchJob,
    transform: &ConcurrentTransform,
) -> Option<PrefetchResult> {
    let (content, ok) = fs.read_file(&job.opts.file_name);
    if !ok {
        return None;
    }
    let result = transform.transform(&job.opts.file_name, &content)?;
    let parse = match &result {
        Ok(result) if crate::contentmapper::parses_canonical_output(result, &content) => {
            let mut opts = job.opts.clone();
            if crate::contentmapper::is_module_virtual_extension(&result.virtual_extension) {
                opts.external_module_indicator_options.force = true;
            }
            let script_kind = get_script_kind_from_file_name(&format!(
                "{}{}",
                job.opts.file_name, result.virtual_extension
            ));
            // The virtual text is leaked, as the loader leaks it
            // (`contentmapper::parse_result`).
            let virtual_text = FileText::new(result.text.clone(), false);
            let before = (synthetic_slot_count(), next_ids());
            let parse = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                parse_source_file_detached(job.job, &opts, virtual_text, script_kind)
            }));
            match parse {
                Ok(parse) => (parse.store.is_self_contained()
                    && (synthetic_slot_count(), next_ids()) == before)
                    .then_some(parse),
                Err(_) => {
                    let _ = take_detached_file_store();
                    None
                }
            }
        }
        _ => None,
    };
    Some(PrefetchResult {
        text: FileText::Shared(Arc::from(content.as_str())),
        parse,
        meta: None,
        mapped: Some(MappedPrefetch { result }),
    })
}

/// What the loader takes from the parse workers for the content-mapped
/// file of `opts`, whose text it read as `content`: the worker's transform
/// result, and its parse of the virtual text, adopted into the stores of
/// this thread, when that parse equals what `contentmapper::parse_result`
/// would make here now. Waits for a running worker. `None`, so that the
/// loader sends the transform itself: no worker started the job (the
/// loader takes it), the worker read other text, or it sent nothing.
// PORT: not in Go (see `PrefetchJob::mapped`).
pub(crate) fn take_prefetched_mapped(
    opts: &SourceFileParseOptions,
    content: &str,
) -> Option<PrefetchedTransform> {
    let shared = PREFETCH.with(|p| p.borrow().clone())?;
    let job = shared.job(&opts.file_name)?;
    job.mapped.as_ref()?;
    let mut state = lock(&job.state);
    #[cfg(test)]
    if lock(&MAPPED_WAIT_FOR_WORKER).contains(&opts.file_name) {
        job.waited.store(true, AtomicOrdering::Relaxed);
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
        while matches!(*state, PrefetchState::Queued) && std::time::Instant::now() < deadline {
            // A worker sets `Running` without a wake, so this polls.
            state = job
                .done
                .wait_timeout(state, std::time::Duration::from_millis(5))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
    let mut waited: Option<std::time::Instant> = None;
    let result = loop {
        match std::mem::replace(&mut *state, PrefetchState::Claimed) {
            PrefetchState::Queued | PrefetchState::Claimed => {
                shared.count(|c| c.claimed.push(opts.file_name.clone()));
                return None;
            }
            PrefetchState::Running => {
                *state = PrefetchState::Running;
                waited.get_or_insert_with(std::time::Instant::now);
                state = job.wait_done(state);
            }
            PrefetchState::Done(result) => break result,
        }
    };
    drop(state);
    if let Some(start) = waited {
        shared.count(|c| {
            c.waited += 1;
            c.wait += start.elapsed();
        });
    }
    let unusable = || shared.count(|c| c.unusable.push(opts.file_name.clone()));
    let Some(PrefetchResult {
        text,
        parse,
        mapped: Some(MappedPrefetch { result }),
        ..
    }) = result
    else {
        unusable();
        return None;
    };
    if &*text != content {
        unusable();
        return None;
    }
    let parse = parse.zip(result.as_ref().ok()).and_then(|(parse, result)| {
        let mut want = opts.clone();
        if crate::contentmapper::is_module_virtual_extension(&result.virtual_extension) {
            want.external_module_indicator_options.force = true;
        }
        let file_opts = &parse.file.parse_options;
        let same = file_opts.file_name == want.file_name
            && file_opts.path == want.path
            && (!parse.read_module_indicator_options
                || file_opts.external_module_indicator_options
                    == want.external_module_indicator_options);
        if !same {
            unusable();
            return None;
        }
        shared.count(|c| c.taken += 1);
        Some(adopt_detached_parse(parse, &want))
    });
    #[cfg(test)]
    lock(&MAPPED_TAKEN).push((opts.file_name.clone(), parse.is_some()));
    Some(PrefetchedTransform { result, parse })
}

/// What the loader can take from the parse workers for the file of
/// `opts`: the worker parse, adopted into the stores of this thread, when
/// it equals what `parse_source_file(opts, text, script_kind)` would make
/// now, or else the text the worker read. Waits for a running worker
/// parse. `text` is the loader's own read of the file, which the worker's
/// text must equal; `None` takes the worker's read as the file text (the
/// host shows the plain OS file system, `CompilerHost::is_plain_os_fs`).
pub fn take_prefetched(
    opts: &SourceFileParseOptions,
    script_kind: ScriptKind,
    text: Option<&str>,
) -> Prefetched {
    let Some(shared) = PREFETCH.with(|p| p.borrow().clone()) else {
        return Prefetched::Nothing;
    };
    let Some(job) = shared.job(&opts.file_name) else {
        shared.count(|c| c.not_queued.push(opts.file_name.clone()));
        return Prefetched::Nothing;
    };
    // The jobs on the worker stack, for the debug counts only.
    let stack = if shared.counts.is_some() {
        lock(&shared.queue).pending.len()
    } else {
        0
    };
    let mut state = lock(&job.state);
    let mut waited: Option<std::time::Instant> = None;
    let result = loop {
        match std::mem::replace(&mut *state, PrefetchState::Claimed) {
            PrefetchState::Queued | PrefetchState::Claimed => {
                shared.count(|c| {
                    c.claimed.push(opts.file_name.clone());
                    if let Some(at) = c.loader_started.map(|started| started.elapsed())
                        && at < EARLY_CLAIMS
                    {
                        c.early_claims.push((at, opts.file_name.clone()));
                    }
                    if let Some(queued_at) = job.queued_at {
                        c.claims.push((
                            queued_at.elapsed(),
                            stack,
                            job.under.load(AtomicOrdering::Relaxed),
                        ));
                    }
                });
                return Prefetched::Nothing;
            }
            PrefetchState::Running => {
                *state = PrefetchState::Running;
                waited.get_or_insert_with(std::time::Instant::now);
                state = job.wait_done(state);
            }
            PrefetchState::Done(result) => break result,
        }
    };
    drop(state);
    if let Some(start) = waited {
        shared.count(|c| {
            c.waited += 1;
            c.wait += start.elapsed();
        });
    }
    let Some(PrefetchResult {
        text: worker_text,
        parse,
        ..
    }) = result
    else {
        return Prefetched::Nothing;
    };
    let unusable = || shared.count(|c| c.unusable.push(opts.file_name.clone()));
    if text.is_some_and(|text| text != &*worker_text) {
        unusable();
        return Prefetched::Nothing;
    }
    let Some(parse) = parse else {
        unusable();
        return Prefetched::Text(worker_text);
    };
    let file_opts = &parse.file.parse_options;
    let same = job.script_kind == script_kind
        && file_opts.file_name == opts.file_name
        && file_opts.path == opts.path
        && (!parse.read_module_indicator_options
            || file_opts.external_module_indicator_options
                == opts.external_module_indicator_options);
    if same {
        shared.count(|c| c.taken += 1);
        let file = adopt_detached_parse(parse, opts);
        TAKEN.with(|taken| *taken.borrow_mut() = Some((job, file.root)));
        Prefetched::Parse(file)
    } else {
        unusable();
        Prefetched::Text(worker_text)
    }
}

#[cfg(test)]
mod move_up_tests {
    use super::*;

    // `GOPORT_PREFETCH_MOVE_UP` is read once per process, so a test chooses
    // the switch per pool (`set_move_up`), and both run here. A job that no
    // worker took moves up when a parse references its file again: it is
    // pushed again, on top. With the switch off, it stays where it is.
    #[test]
    fn a_queued_job_moves_up_only_with_the_switch() {
        let opts = |name: &str| SourceFileParseOptions {
            file_name: format!("/p/{name}"),
            path: Path(format!("/p/{name}")),
            external_module_indicator_options: ExternalModuleIndicatorOptions::default(),
        };
        for (on, pending) in [
            (false, ["/p/a.ts", "/p/b.ts"].as_slice()),
            (true, ["/p/a.ts", "/p/b.ts", "/p/a.ts"].as_slice()),
        ] {
            set_move_up(Some(on));
            let shared = PrefetchShared::new(PrefetchConfig {
                current_directory: "/p".to_string(),
                use_case_sensitive_file_names: true,
                default_library_path: String::new(),
            });
            set_move_up(None);
            for name in ["a.ts", "b.ts", "a.ts"] {
                shared.queue(opts(name), ScriptKind::TS);
            }
            let queue = lock(&shared.queue);
            let names: Vec<&str> = queue
                .pending
                .iter()
                .map(|job| job.opts.file_name.as_str())
                .collect();
            assert_eq!(names, pending, "move-up {on}");
        }
    }
}

#[cfg(test)]
mod stat_cache_tests {
    use super::*;

    // statcut1 round 2 (skeptic): one stat answers `FileExists` and
    // `DirectoryExists` only where the file system answers them from that
    // stat. The bundled wrapper does not for `bundled:///`: its stat is a
    // directory, but `DirectoryExists` is false (Go embed.go:66, :85). Each
    // answer of the workers' cache must be the file system's own.
    #[test]
    fn one_stat_answers_as_the_file_system() {
        let fs = crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs());
        let dir = std::env::temp_dir().join(format!("ts_goport_statcut_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), "").unwrap();
        let root = normalize_slashes(&dir.to_string_lossy());
        let lib = format!("{}/lib.es5.d.ts", crate::frontend::bundled::lib_path());
        let paths = [
            "bundled:///".to_string(),
            "bundled:///libs".to_string(),
            "bundled:///libs/".to_string(),
            "bundled:///nope".to_string(),
            lib,
            root.clone(),
            format!("{root}/"),
            format!("{root}/a.ts"),
            format!("{root}/b.ts"),
        ];
        let stats = SharedStatCache::default();
        for path in &paths {
            // The stat of a root file size comes first, as in a load
            // (`PrefetchShared::rank_largest`).
            let _ = stats.node(&*fs, path);
            assert_eq!(
                stats.exists(&*fs, path, false),
                Some(fs.file_exists(path)),
                "FileExists({path})"
            );
            assert_eq!(
                stats.exists(&*fs, path, true),
                Some(fs.directory_exists(path)),
                "DirectoryExists({path})"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lookup of a path that another thread loads waits at most
    /// `STAT_MAP_WAIT`, then loads the path itself and stores nothing: the
    /// value of the path stays the loader's.
    #[test]
    fn stat_map_waiter_loads_itself() {
        use std::sync::mpsc;
        let map = &StatMap::<u32>::default();
        std::thread::scope(|scope| {
            let (started, on_start) = mpsc::channel();
            let (finish, on_finish) = mpsc::channel::<()>();
            let loader = scope.spawn(move || {
                map.get_or_load("p", || {
                    started.send(()).unwrap();
                    // Until `finish` is dropped.
                    let _ = on_finish.recv();
                    1
                })
            });
            on_start.recv().unwrap();
            let (waited, on_wait) = mpsc::channel();
            scope.spawn(move || waited.send(map.get_or_load("p", || 2)));
            // A waiter that waited for the loader would get 1, after
            // `finish`.
            let wait = on_wait.recv_timeout(std::time::Duration::from_secs(10));
            let stored = map.get("p");
            drop(finish);
            assert_eq!(wait, Ok(2));
            assert_eq!(stored, None);
            assert_eq!(loader.join().unwrap(), 1);
        });
        assert_eq!(map.get("p"), Some(1));
    }

    /// A load that never finishes (it panics) leaves its slot `Loading`.
    /// Each lookup of the path then loads it itself, and an insert stores
    /// its value there.
    #[test]
    fn stat_map_load_that_never_finishes() {
        let map = Arc::new(StatMap::<u32>::default());
        let load = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            map.get_or_load("p", || panic!("the load fails"))
        }));
        assert!(load.is_err());
        assert_eq!(map.get("p"), None);
        // On another thread, so that a lookup that waits for the slot
        // fails the test and does not stop it.
        let (looked, on_look) = std::sync::mpsc::channel();
        std::thread::spawn({
            let map = map.clone();
            move || looked.send((map.get_or_load("p", || 1), map.get_or_load("p", || 2)))
        });
        let looks = on_look.recv_timeout(std::time::Duration::from_secs(10));
        assert_eq!(looks, Ok((1, 2)));
        assert_eq!(map.insert_if_absent("p", 3), 3);
        assert_eq!(map.get_or_load("p", || 4), 3);
    }

    /// A clear drops the values and the slots of the loads that run, as Go
    /// `ClearCache`. The value of a load that runs over the clear is then
    /// stored, as Go's `Store` after `ClearCache`.
    #[test]
    fn stat_map_insert_after_clear() {
        use std::sync::mpsc;
        let map = &StatMap::<u32>::default();
        assert_eq!(map.get_or_load("p", || 1), 1);
        map.clear();
        assert_eq!(map.get("p"), None);
        assert_eq!(map.insert_if_absent("p", 2), 2);
        assert_eq!(map.get_or_load("p", || 3), 2);
        std::thread::scope(|scope| {
            let (started, on_start) = mpsc::channel();
            let (finish, on_finish) = mpsc::channel::<()>();
            let loader = scope.spawn(move || {
                map.get_or_load("q", || {
                    started.send(()).unwrap();
                    let _ = on_finish.recv();
                    4
                })
            });
            on_start.recv().unwrap();
            map.clear();
            drop(finish);
            assert_eq!(loader.join().unwrap(), 4);
        });
        assert_eq!(map.get("q"), Some(4));
        assert_eq!(map.get_or_load("q", || 5), 4);
    }

    /// An early pool's stat cache goes only to the first `LoaderStatFs`
    /// made on its thread after the pool started (`worker_stats`).
    #[test]
    fn early_pool_stats_go_to_the_next_host() {
        let new_host = || LoaderStatFs::new(crate::frontend::vfs::osvfs_fs()).host;
        let before = new_host();
        let pool = PrefetchPool::start(
            PrefetchConfig {
                current_directory: "/".to_string(),
                use_case_sensitive_file_names: true,
                default_library_path: String::new(),
            },
            0,
        );
        let stats = pool.shared.stats.clone();
        if let Some(old) = EARLY_POOL.with(|early| early.borrow_mut().replace(pool)) {
            old.discard();
        }
        let first = new_host();
        let later = new_host();
        let reads = |host| worker_stats(host).is_some_and(|got| Arc::ptr_eq(&got, &stats));
        let read = [reads(before), reads(first), reads(later)];
        if let Some(pool) = EARLY_POOL.with(|early| early.borrow_mut().take()) {
            pool.discard();
        }
        assert_eq!(read, [false, true, false], "before, first, later");
    }
}
