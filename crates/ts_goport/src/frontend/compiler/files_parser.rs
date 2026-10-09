//! Go: compiler/filesparser.go (parse tasks, the files parser and
//! `getProcessedFiles`).

use super::file_loader::SyntheticImports;
use crate::frontend::prelude::*;
use crate::gostd::slices::stable_sort_by;
use crate::program::ThreadBudget;
use std::cell::Cell;
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
    pub fn add_sub_task(&mut self, ref_: ResolvedRef, lib_file: Option<Rc<LibFile>>) {
        // PERF: a resolved name is normal already (Go normalizes it again);
        // then the name moves, with no copy.
        let normalized_file_path = if is_normalized_path(&ref_.file_name) {
            ref_.file_name
        } else {
            normalize_path(&ref_.file_name)
        };
        let sub_task = Rc::new(RefCell::new(ParseTask {
            normalized_file_path,
            lib_file,
            increase_depth: ref_.increase_depth,
            elide_on_depth: ref_.elide_on_depth,
            include_reason: ref_.include_reason,
            package_id: ref_.package_id,
            ..Default::default()
        }));
        self.sub_tasks.push(sub_task);
    }
}

// Go: filesparser.go:211 resolvedRef
#[derive(Clone, Default)]
pub struct ResolvedRef {
    pub file_name: String,
    pub increase_depth: bool,
    pub elide_on_depth: bool,
    pub include_reason: Option<Rc<FileIncludeReason>>,
    pub package_id: PackageId,
}

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
    pub task_data_by_path: FxHashMap<Path, Rc<RefCell<ParseTaskData>>>,
    pub max_depth: i32,
    /// Go `singleThreaded`: no parse workers.
    pub single_threaded: bool,
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
struct ParseTaskLinks(FxHashMap<Path, Rc<RefCell<ParseTaskData>>>);

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
        let mut pool = match early {
            Some(pool) if pool.shared.config == config => pool,
            other => {
                if let Some(other) = other {
                    other.discard();
                }
                PrefetchPool::start(config, workers)
            }
        };
        // The workers use the cache of the loader's file system
        // (`SharedStatCache`). Only on the plain OS file system: another
        // file system caches other files than the workers read. It is set
        // before the resolve config, so a worker's resolver sees it
        // (`WorkerResolver::new`).
        let build_host_cache = loader
            .host
            .stat_cache()
            .filter(|_| loader.host.is_plain_os_fs());
        if let Some(cache) = build_host_cache {
            cache.start_load();
            let _ = pool.shared.stats.host.set(cache);
        }
        let _ = pool
            .shared
            .resolve
            .set(WorkerResolveConfig::of_loader(loader));
        {
            let mut queue = lock(&pool.shared.queue);
            queue.cached = cached;
            queue.freeable = freeable;
        }
        // A program that does not use the sources of its references loads
        // their output `.d.ts` files in place of the sources (Go
        // `getParseFileRedirect`), so the workers parse those.
        if !loader.opts.can_use_project_reference_source() {
            lock(&pool.shared.queue).redirects = loader
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
        drop(pool);
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
            // PERF: Go `loader.toPath` asks the host for its current
            // directory and case sensitivity each time; the loader keeps
            // the same values (`compare_paths_options`), so no copy of the
            // directory is made per task.
            let path = to_path(
                &task.borrow().normalized_file_path,
                &loader.compare_paths_options.current_directory,
                loader.compare_paths_options.use_case_sensitive_file_names,
            );
            task.borrow_mut().path = path.clone();
            let (data, loaded) = match self.task_data_by_path.get(&path) {
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
            let name = task.borrow().normalized_file_path.clone();
            let existing_task = data.borrow().tasks.get(&name).cloned();
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

        // PORT: Go does not change `data.tasks` in this loop, so a copy of the
        // values iterates the same tasks.
        let tasks_by_file_name: Vec<ParseTaskRef> = data.borrow().tasks.values().cloned().collect();
        for task_by_file_name in tasks_by_file_name {
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
                let sub_tasks = task_by_file_name.borrow().sub_tasks.clone();
                let lowest_depth = data.borrow().lowest_depth;
                self.start(loader, &sub_tasks, lowest_depth);
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

        let mut include_processor = IncludeProcessor::default();
        let mut output_file_to_project_reference_source: Option<FxHashMap<Path, String>> =
            if !loader.opts.can_use_project_reference_source() {
                Some(FxHashMap::default())
            } else {
                None
            };
        let mut resolved_modules: FxHashMap<Path, ModeAwareCache<Arc<ResolvedModule>>> =
            FxHashMap::default();
        let mut type_resolutions_in_file: FxHashMap<
            Path,
            ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>,
        > = FxHashMap::default();
        let mut source_file_meta_datas: FxHashMap<Path, SourceFileMetaData> = FxHashMap::default();
        let mut jsx_runtime_import_specifiers: Option<
            FxHashMap<Path, Rc<JsxRuntimeImportSpecifier>>,
        > = None;
        let mut import_helpers_import_specifiers: Option<FxHashMap<Path, Node>> = None;
        let mut source_files_found_searching_node_modules: FxHashSet<Path> = FxHashSet::default();
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

        // PORT: Go `seen map[*parseTaskData]string` is keyed by pointer. The
        // key here is the `Rc` pointer of the task data.
        let mut seen: FxHashMap<*const RefCell<ParseTaskData>, String> =
            FxHashMap::with_capacity_and_hasher(self.task_data_by_path.len(), Default::default());

        // PORT: the Go closure `collectFiles` is an explicit recursive
        // function over a struct that holds its captured variables.
        struct Collector<'a> {
            parser: &'a FilesParser,
            loader: &'a FileLoader,
            seen: &'a mut FxHashMap<*const RefCell<ParseTaskData>, String>,
            missing_files: &'a mut Vec<String>,
            duplicate_source_files: &'a mut Vec<DuplicateSourceFile>,
            files: &'a mut Vec<Rc<ParsedSourceFile>>,
            lib_files: &'a mut Vec<Rc<ParsedSourceFile>>,
            files_by_path: &'a mut FxHashMap<Path, Rc<ParsedSourceFile>>,
            tasks_seen_by_name_ignore_case: &'a mut Option<FxHashMap<String, ParseTaskRef>>,
            include_processor: &'a mut IncludeProcessor,
            output_file_to_project_reference_source: &'a mut Option<FxHashMap<Path, String>>,
            resolved_modules: &'a mut FxHashMap<Path, ModeAwareCache<Arc<ResolvedModule>>>,
            type_resolutions_in_file:
                &'a mut FxHashMap<Path, ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>>,
            source_file_meta_datas: &'a mut FxHashMap<Path, SourceFileMetaData>,
            jsx_runtime_import_specifiers:
                &'a mut Option<FxHashMap<Path, Rc<JsxRuntimeImportSpecifier>>>,
            import_helpers_import_specifiers: &'a mut Option<FxHashMap<Path, Node>>,
            source_files_found_searching_node_modules: &'a mut FxHashSet<Path>,
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
            // Go: filesparser.go:386 collectFiles
            fn collect_files(&mut self, tasks: &[ParseTaskRef]) {
                let loader = self.loader;
                for task in tasks {
                    let mut task = task.clone();
                    let include_reason = task.borrow().include_reason.clone();
                    // Exclude automatic type directive tasks from include reason processing,
                    // as these are internal implementation details and should not contribute
                    // to the reasons for including files.
                    let (has_redirect, is_automatic) = {
                        let t = task.borrow();
                        (
                            t.redirected_parse_task.is_some(),
                            t.is_for_automatic_type_directive,
                        )
                    };
                    if !has_redirect && !is_automatic {
                        let loaded_task = task.borrow().loaded_task.clone();
                        if let Some(loaded_task) = loaded_task {
                            task = loaded_task;
                        }
                        self.parser.add_include_reason(
                            self.include_processor,
                            &task,
                            include_reason.clone(),
                        );
                    }
                    let data = self
                        .parser
                        .task_data_by_path
                        .get(&task.borrow().path)
                        .cloned();
                    if !task.borrow().loaded {
                        continue;
                    }
                    let data = data.expect("loaded parse task without task data");
                    let data_key = Rc::as_ptr(&data);

                    let normalized_file_path = task.borrow().normalized_file_path.clone();
                    let task_path = task.borrow().path.clone();

                    // ensure we only walk each task once
                    if let Some(checked_name) = self.seen.get(&data_key).cloned() {
                        if let Some(file) = task.borrow().file.clone() {
                            if checked_name != normalized_file_path
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
                        }
                        // PORT: equal names give equal absolute paths, so
                        // the check below only runs for different names.
                        if checked_name != normalized_file_path
                            && !loader
                                .opts
                                .config
                                .compiler_options()
                                .force_consistent_casing_in_file_names
                                .is_false()
                        {
                            // Check if it differs only in drive letters its ok to ignore that error:
                            let checked_absolute_path = get_normalized_absolute_path_without_root(
                                &checked_name,
                                &loader.compare_paths_options.current_directory,
                            );
                            let input_absolute_path = get_normalized_absolute_path_without_root(
                                &normalized_file_path,
                                &loader.compare_paths_options.current_directory,
                            );
                            if checked_absolute_path != input_absolute_path {
                                self.include_processor
                                    .add_processing_diagnostics_for_file_casing(
                                        &task_path,
                                        &checked_name,
                                        &normalized_file_path,
                                        include_reason
                                            .clone()
                                            .expect("nil pointer dereference: includeReason"),
                                    );
                            }
                        }
                        continue;
                    } else {
                        self.seen.insert(data_key, normalized_file_path.clone());
                    }

                    if let Some(tasks_seen_by_name_ignore_case) =
                        self.tasks_seen_by_name_ignore_case.as_mut()
                    {
                        let path_lower_case = to_file_name_lower_case(&task_path);
                        if let Some(task_by_ignore_case) =
                            tasks_seen_by_name_ignore_case.get(&path_lower_case)
                        {
                            let t = task_by_ignore_case.borrow();
                            self.include_processor
                                .add_processing_diagnostics_for_file_casing(
                                    &t.path,
                                    &t.normalized_file_path,
                                    &normalized_file_path,
                                    include_reason
                                        .clone()
                                        .expect("nil pointer dereference: includeReason"),
                                );
                        } else {
                            tasks_seen_by_name_ignore_case.insert(path_lower_case, task.clone());
                        }
                    }

                    {
                        let t = task.borrow();
                        for trace in &t.type_resolutions_trace {
                            loader.host.trace(trace.message, trace.args.clone());
                        }
                        for trace in &t.resolutions_trace {
                            loader.host.trace(trace.message, trace.args.clone());
                        }
                    }

                    let file = task.borrow().file.clone();
                    let data_package_id = data.borrow().package_id.clone();
                    let data_lowest_depth = data.borrow().lowest_depth;
                    if let Some(package_id_to_source_file) = self.package_id_to_source_file.as_mut()
                    {
                        if !data_package_id.name.is_empty() {
                            if let Some(package_id_file) =
                                package_id_to_source_file.get(&data_package_id).cloned()
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
                                    .push(normalized_file_path.clone());
                                let redirect_files_by_path =
                                    self.redirect_files_by_path.get_or_insert_with(|| {
                                        FxHashMap::with_capacity_and_hasher(
                                            self.total_file_count,
                                            Default::default(),
                                        )
                                    });
                                let index =
                                    (self.files.len() + redirect_files_by_path.len()) as i32;
                                redirect_files_by_path.insert(
                                    task_path.clone(),
                                    RedirectsFile {
                                        index,
                                        file_name: normalized_file_path.clone(),
                                        path: task_path.clone(),
                                        target: package_id_file.path().clone(),
                                    },
                                );
                                self.files_by_path
                                    .insert(task_path.clone(), package_id_file);
                                if data_lowest_depth > 0 {
                                    self.source_files_found_searching_node_modules
                                        .insert(task_path.clone());
                                }
                                continue;
                            } else if let Some(file) = &file {
                                package_id_to_source_file
                                    .insert(data_package_id.clone(), file.clone());
                            }
                        }
                    }

                    {
                        // PERF: walks the subtasks in place (no copy of the
                        // list). Collecting only reads tasks.
                        let t = task.borrow();
                        if !t.sub_tasks.is_empty() {
                            self.collect_files(&t.sub_tasks);
                        }
                    }

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
                    self.resolved_modules
                        .insert(path.clone(), std::mem::take(&mut t.resolutions_in_file));
                    self.type_resolutions_in_file.insert(
                        path.clone(),
                        std::mem::take(&mut t.type_resolutions_in_file),
                    );
                    self.source_file_meta_datas
                        .insert(path.clone(), std::mem::take(&mut t.metadata));

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
                    if data_lowest_depth > 0 {
                        self.source_files_found_searching_node_modules.insert(path);
                    }
                }
            }
        }

        let mut collector = Collector {
            parser: self,
            loader,
            seen: &mut seen,
            missing_files: &mut missing_files,
            duplicate_source_files: &mut duplicate_source_files,
            files: &mut files,
            lib_files: &mut lib_files,
            files_by_path: &mut files_by_path,
            tasks_seen_by_name_ignore_case: &mut tasks_seen_by_name_ignore_case,
            include_processor: &mut include_processor,
            output_file_to_project_reference_source: &mut output_file_to_project_reference_source,
            resolved_modules: &mut resolved_modules,
            type_resolutions_in_file: &mut type_resolutions_in_file,
            source_file_meta_datas: &mut source_file_meta_datas,
            jsx_runtime_import_specifiers: &mut jsx_runtime_import_specifiers,
            import_helpers_import_specifiers: &mut import_helpers_import_specifiers,
            source_files_found_searching_node_modules:
                &mut source_files_found_searching_node_modules,
            lib_files_map: &mut lib_files_map,
            redirect_targets_map: &mut redirect_targets_map,
            redirect_files_by_path: &mut redirect_files_by_path,
            package_id_to_source_file: &mut package_id_to_source_file,
            recorded_duplicates: FxHashMap::default(),
            total_file_count,
        };
        collector.collect_files(&loader.root_tasks);
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

    // Go: filesparser.go:594 (*filesParser).addIncludeReason
    // PORT: Go can append a nil reason. Only the automatic type directive
    // root task has no reason, and `collectFiles` never passes it here, so a
    // nil reason is skipped.
    pub fn add_include_reason(
        &self,
        include_processor: &mut IncludeProcessor,
        task: &ParseTaskRef,
        reason: Option<Rc<FileIncludeReason>>,
    ) {
        let t = task.borrow();
        if let Some(redirected) = &t.redirected_parse_task {
            self.add_include_reason(include_processor, redirected, reason);
        } else if t.loaded {
            let Some(reason) = reason else {
                return;
            };
            // The map is not shared yet while the files are collected, so
            // `make_mut` copies nothing.
            let reasons = Rc::make_mut(&mut include_processor.file_include_reasons);
            if let Some(existing) = reasons.get_mut(&t.path) {
                existing.push(reason);
            } else {
                reasons.insert(t.path.clone(), vec![reason]);
            }
        }
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
    /// What the worker that parsed the file resolved for the loader after
    /// the parse (`FilePrep`), once it is there.
    prep: Mutex<Option<Box<FilePrep>>>,
    /// The transform of a content-mapped file: the worker parses the
    /// mapper's virtual text (`prefetch_mapped`).
    // PORT: not in Go, where the parse goroutines transform (tsgo#4712).
    mapped: Option<Arc<ConcurrentTransform>>,
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
/// that the worker used (`answer`); it still builds the subtasks, the
/// synthetic nodes and the per-file maps itself, in the queue order.
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
    /// The mode of every answer: Go `getModeForUsageLocation` of a plain
    /// `import .. from` in the file (`guess_import_mode`). The worker
    /// cannot read the import nodes (`DetachedParse::import_specifiers`).
    mode: ResolutionMode,
    /// The synthetic imports, then the imports, by module name index.
    /// `None` for an empty name.
    answers: Vec<Option<SharedResolution<Arc<ResolvedModule>>>>,
}

impl FilePrep {
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
        if mode != self.mode {
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
        let prep = lock(&self.0.prep).take();
        if prep.is_none() {
            count_prep(|c| c.preps_missing += 1);
        }
        prep
    }
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
    let job = lock(&shared.queue).by_name.get(file_name).cloned()?;
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
                state = job
                    .done
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            // A test can make the loader wait for a job that no worker has
            // started yet (`set_meta_wait`), so the worker finds the
            // metadata whatever the timing.
            #[cfg(test)]
            PrefetchState::Queued if META_WAIT.with(Cell::get) => {
                state = job
                    .done
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
}

/// True when the parse workers of a load on this thread make preps for
/// the loader (`FilePrep`): unless `GOPORT_LOAD_PREP` is `0` (an A/B
/// switch), or a test chose (`set_load_prep`).
fn load_prep_enabled() -> bool {
    LOAD_PREP
        .with(Cell::get)
        .unwrap_or_else(|| std::env::var_os("GOPORT_LOAD_PREP").is_none_or(|value| value != "0"))
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

#[derive(Default)]
struct PrefetchQueue {
    /// Jobs no worker has taken yet. Workers take the newest first, like
    /// the loader's queue, but take `lib.dom.d.ts` before all others. A job
    /// can be in the list twice (`rank_largest`, `queue_batch`); a worker
    /// skips a job that is no longer queued.
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
    /// Every job by file name. A file name is queued once.
    by_name: FxHashMap<String, Arc<PrefetchJob>>,
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
            return Some(job.clone());
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
            prep: Mutex::new(None),
            mapped,
        });
        self.next_job += 1;
        self.by_name.insert(job.opts.file_name.clone(), job.clone());
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
                return self.by_name.get(file_name).cloned();
            }
        };
        match self.by_name.get(&opts.file_name) {
            Some(job) => Some(job.clone()),
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
}

impl WorkerResolveConfig {
    /// The config of `loader`. `None` when workers do not resolve.
    fn of_loader(loader: &FileLoader) -> Option<Self> {
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
        if loader.shared_resolution.is_some() {
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
            shared: loader.shared_resolution.clone(),
            references,
            redirects,
            prep: loader.shared_resolution.is_some() && load_prep_enabled(),
        })
    }
}

/// The jobs that the loading thread and the parse workers share.
struct PrefetchShared {
    queue: Mutex<PrefetchQueue>,
    ready: Condvar,
    /// `queue.closed`, which a worker reads between resolutions.
    closed: AtomicBool,
    config: PrefetchConfig,
    /// The stat cache of the workers' file systems (`WorkerFs`).
    stats: Arc<SharedStatCache>,
    /// Set when the loader takes the workers (`FilesParser::parse`). Until
    /// then, and with `None` inside, the workers do not resolve.
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
    /// Worker parses with no prep when the loader needed it: the worker
    /// still resolved, or it made none.
    preps_missing: usize,
    /// Module names of taken preps with a worker answer, and with none
    /// (another mode than the worker's).
    pub(crate) names_taken: usize,
    pub(crate) names_own: usize,
}

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
            "goport prefetch: took {} worker metadata; preps: took {} ({} names, {} resolved here), unfit {}, missing {}",
            self.meta_taken,
            self.preps_taken,
            self.names_taken,
            self.names_own,
            self.preps_unfit,
            self.preps_missing,
        );
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

    /// The job of the worker parse that `take_prefetched` gave last, and
    /// the root node of the adopted parse (`TakenParse::of`).
    static TAKEN: RefCell<Option<(Arc<PrefetchJob>, Node)>> = const { RefCell::new(None) };
}

impl PrefetchShared {
    fn new(config: PrefetchConfig) -> Self {
        Self {
            queue: Mutex::new(PrefetchQueue::default()),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
            config,
            stats: Arc::new(SharedStatCache::default()),
            resolve: OnceLock::new(),
            counts: std::env::var_os("GOPORT_PREFETCH_STATS")
                .map(|_| Mutex::new(PrefetchStats::default())),
        }
    }

    /// Closes the queue: the workers stop after their current job.
    fn close(&self) {
        lock(&self.queue).closed = true;
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
        let queue = lock(&self.queue);
        let mut untaken = (0, 0);
        for job in queue.by_name.values() {
            if let PrefetchState::Done(Some(result)) = &*lock(&job.state) {
                untaken.0 += 1;
                untaken.1 += result.text.len() as u64;
            }
        }
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
            .filter_map(|task| {
                queue
                    .by_name
                    .get(&task.borrow().normalized_file_path)
                    .cloned()
            })
            .collect();
        if jobs.len() < 2 || count == 0 {
            return;
        }
        queue.rank = Some((jobs, count));
        drop(queue);
        self.ready.notify_all();
    }

    /// Pushes the `count` largest of `jobs` that are still queued on top of
    /// the queue, the largest last so it is taken first. A worker runs it,
    /// so the loader does not wait for the `stat` calls.
    fn rank_largest(&self, fs: &dyn Fs, jobs: Vec<Arc<PrefetchJob>>, count: usize) {
        let queued = |job: &Arc<PrefetchJob>| matches!(*lock(&job.state), PrefetchState::Queued);
        let mut sized: Vec<(i64, Arc<PrefetchJob>)> = jobs
            .into_iter()
            .filter(|job| queued(job))
            .filter_map(|job| fs.stat(&job.opts.file_name).map(|info| (info.size(), job)))
            .collect();
        // Stable: files of one size keep the root order.
        sized.sort_by_key(|(size, _)| *size);
        let largest = sized.split_off(sized.len().saturating_sub(count));
        let mut queue = lock(&self.queue);
        for (_, job) in largest {
            if queued(&job) {
                queue.push_pending(job);
            }
        }
        drop(queue);
        self.ready.notify_all();
    }

    /// Queues a parse of `opts.file_name`, unless one is queued already.
    fn queue(&self, opts: SourceFileParseOptions, script_kind: ScriptKind) {
        let mut queue = lock(&self.queue);
        if queue.by_name.contains_key(&opts.file_name) {
            return;
        }
        let Some(job) = queue.add(opts, script_kind, None) else {
            return;
        };
        if !queue.is_first(&job) {
            queue.pending.push(job);
        }
        drop(queue);
        self.ready.notify_all();
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
    fn queue_batch(&self, requests: Vec<PrefetchRequest>) {
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
            queue.push_pending(newest);
            for job in jobs {
                queue.push_pending(job);
            }
        }
        drop(queue);
        // The workers of content-mapped jobs wait on `ready` too, so a
        // single wake could reach one that cannot take the job.
        if woken > 0 {
            self.ready.notify_all();
        }
    }

    /// Queues a parse of each file in `names` that the parser can take (a
    /// normalized absolute name with a known extension).
    fn queue_names(&self, names: Vec<String>) {
        let config = &self.config;
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
        for file_name in names {
            let script_kind = get_script_kind_from_file_name(&file_name);
            if script_kind == ScriptKind::UNKNOWN
                || !has_extension(&file_name)
                || get_encoded_root_length(&file_name) == 0
                || file_name != normalize_path(&file_name)
            {
                continue;
            }
            let path = to_path(
                &file_name,
                &config.current_directory,
                config.use_case_sensitive_file_names,
            );
            // PORT: the options are a guess (see `FilesParser::prefetch_request`).
            let external_module_indicator_options =
                options.map_or_else(Default::default, |options| {
                    get_external_module_indicator_options(
                        &file_name,
                        options,
                        &SourceFileMetaData::default(),
                    )
                });
            self.queue(
                SourceFileParseOptions {
                    file_name,
                    path,
                    external_module_indicator_options,
                },
                script_kind,
            );
        }
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
            let name = if is_rooted_disk_path(&reference.file_name) {
                reference.file_name.clone()
            } else {
                combine_paths(
                    &get_directory_path(&refs.file_name),
                    &[&reference.file_name],
                )
            };
            names.push(normalize_path(&name));
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
}

impl PrefetchPool {
    fn start(config: PrefetchConfig, workers: usize) -> Self {
        let mut pool = Self {
            shared: Arc::new(PrefetchShared::new(config)),
            threads: Vec::new(),
            mapped_workers: false,
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

    /// Stops an unused pool without waiting: each worker ends after its
    /// current job.
    fn discard(mut self) {
        self.shared.close();
        // Dropping a join handle detaches its thread.
        self.threads.clear();
    }
}

impl Drop for PrefetchPool {
    fn drop(&mut self) {
        self.shared.close();
        // A discarded pool has no threads here and prints no counts.
        let used = !self.threads.is_empty();
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
// PORT: not in Go. Go starts the lib parses when `NewProgram` queues its
// root tasks; here the lib.dom parse is the longest path of small
// programs, so it starts earlier.
pub fn start_default_lib_prefetch(
    config: &ParsedCommandLine,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
    default_library_path: &str,
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
    // Go: fileloader.go:153 (root lib tasks)
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
        *lock(&job.state) = PrefetchState::Done(result);
        job.done.notify_all();
    }
}

impl Drop for RunningJob<'_> {
    fn drop(&mut self) {
        *lock(&self.0.state) = PrefetchState::Done(None);
        self.0.done.notify_all();
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
                let mut result = prefetch_parse(&*fs, &job, opts.as_ref().unwrap_or(&job.opts));
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
                        (Some(Arc::new(FileRefs::take_from(parse))), synthetic)
                    }
                    None => (None, None),
                };
                (result, refs, synthetic)
            }
        };
        running.finish(result);
        let Some(refs) = refs else {
            continue;
        };
        let prep = meta.zip(synthetic).map(|(meta, synthetic)| {
            (
                PrepInput {
                    meta: meta.meta,
                    synthetic,
                },
                &*job,
            )
        });
        // A resolution that panics (a Go panic) panics on the loader too
        // when it resolves the same import. The worker only stops resolving.
        let queued = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            shared.queue_references(&*fs, &refs, resolver.as_ref(), prep);
        }));
        if queued.is_err() {
            resolver = None;
            resolver_failed = true;
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
    /// (`FilePrep`). The module names have no import nodes here, so each
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
        let (meta, synthetic) = match prep {
            Some(PrepInput { meta, synthetic }) => (meta, Some(synthetic)),
            // Go `loadSourceFileMetaData` reads the program's options.
            None => (
                super::file_loader::source_file_meta_data(&self.resolver, &self.options, file_name),
                None,
            ),
        };
        for reference in &refs.type_reference_directives {
            if shared.is_closed() {
                return None;
            }
            // Go: fileloader.go:1021 getModeForTypeReferenceDirectiveInFile
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
        let mode = super::file_loader::guess_import_mode(file_name, &meta, &options);
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
        let specifiers = synthetic
            .iter()
            .flat_map(SyntheticImports::names)
            .chain(refs.import_specifiers.iter().map(String::as_str));
        for specifier in specifiers {
            if shared.is_closed() {
                return None;
            }
            if specifier.is_empty() {
                answers.push(None);
                continue;
            }
            let (resolved, _, _) =
                self.resolver
                    .resolve_module_name(specifier, containing_file, mode, redirect);
            if let Some((cache, directory, redirect_name)) = &answers_from {
                answers.push(cache.get_module(&(
                    directory.as_str(),
                    specifier,
                    mode,
                    redirect_name.as_str(),
                )));
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
            mode,
            answers,
        })
    }
}

/// What a parse worker knows of its file for the prep (`FilePrep`) before
/// it resolves: the metadata and the synthetic imports.
struct PrepInput {
    meta: SourceFileMetaData,
    synthetic: SyntheticImports,
}

/// The lookups that Go `cachedvfs` caches (all but `Stat`), in maps that
/// other threads can read. The parse workers of one load share one
/// (`SharedStatCache`). The `tsc -b` host's cached file system keeps its
/// cache in them too (`BuildStatCache`).
#[derive(Default)]
pub struct StatCache {
    file_exists: Mutex<FxHashMap<String, bool>>,
    directory_exists: Mutex<FxHashMap<String, bool>>,
    realpath: Mutex<FxHashMap<String, String>>,
    entries: Mutex<FxHashMap<String, Entries>>,
}

/// The cached value of `path`, or `load()` stored as it. The lock is not
/// held while `load` runs.
fn cached_stat<V: Clone>(
    cache: &Mutex<FxHashMap<String, V>>,
    path: &str,
    load: impl FnOnce() -> V,
) -> V {
    if let Some(value) = lock(cache).get(path) {
        return value.clone();
    }
    let value = load();
    lock(cache).entry(path.to_string()).or_insert(value).clone()
}

/// Stores the value of `path` in `from` in `to`, when `to` has none.
fn copy_stat<V: Clone>(
    from: &Mutex<FxHashMap<String, V>>,
    to: &Mutex<FxHashMap<String, V>>,
    path: &str,
) {
    let Some(value) = lock(from).get(path).cloned() else {
        return;
    };
    lock(to).entry(path.to_string()).or_insert(value);
}

/// Stores each value of `from` in `to`, when `to` has none for its path.
fn merge_stats<V>(from: &Mutex<FxHashMap<String, V>>, to: &Mutex<FxHashMap<String, V>>) {
    let from = std::mem::take(&mut *lock(from));
    let mut to = lock(to);
    for (path, value) in from {
        to.entry(path).or_insert(value);
    }
}

impl StatCache {
    /// The cached `FileExists(path)`, or `load()` stored as it.
    pub fn file_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        cached_stat(&self.file_exists, path, load)
    }

    /// The cached `DirectoryExists(path)`, or `load()` stored as it.
    pub fn directory_exists(&self, path: &str, load: impl FnOnce() -> bool) -> bool {
        cached_stat(&self.directory_exists, path, load)
    }

    /// The cached `Realpath(path)`, or `load()` stored as it.
    pub fn realpath(&self, path: &str, load: impl FnOnce() -> String) -> String {
        cached_stat(&self.realpath, path, load)
    }

    /// The cached `GetAccessibleEntries(path)`, or `load()` stored as it.
    pub fn entries(&self, path: &str, load: impl FnOnce() -> Entries) -> Entries {
        cached_stat(&self.entries, path, load)
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
        lock(&self.file_exists).clear();
        lock(&self.directory_exists).clear();
        lock(&self.realpath).clear();
        lock(&self.entries).clear();
    }
}

/// The lookup cache of the `tsc -b` host's file system
/// (`CompilerHost::stat_cache`), shared with the parse workers of its
/// program loads.
///
/// PORT: Go parse tasks share the host's cachedvfs, which lasts for the
/// whole build, and a write does not update it (cachedvfs.go:144). So each
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
        map: impl Fn(&StatCache) -> &Mutex<FxHashMap<String, V>>,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        let cached = map(&self.cached);
        if let Some(value) = lock(cached).get(path) {
            return value.clone();
        }
        let found = lock(map(&self.load)).get(path).cloned();
        let value = found.unwrap_or_else(load);
        lock(cached)
            .entry(path.to_string())
            .or_insert(value)
            .clone()
    }

    /// A parse worker's lookup: the value in `cached`, else in `load`, else
    /// `load()` stored in `load`. A lookup that `cached` does not have goes
    /// into the log of the resolution that runs (`note_worker_lookup`).
    fn worker_lookup<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &Mutex<FxHashMap<String, V>>,
        kind: StatKind,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        if let Some(value) = lock(map(&self.cached)).get(path) {
            return value.clone();
        }
        note_worker_lookup(kind, path);
        cached_stat(map(&self.load), path, load)
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
struct SharedStatCache {
    own: StatCache,
    host: OnceLock<Arc<BuildStatCache>>,
}

impl SharedStatCache {
    /// The cached value of `path` in the host's cache (when there is one,
    /// `BuildStatCache::worker_lookup`) or else in the workers' own cache,
    /// or `load()` stored there.
    fn lookup<V: Clone>(
        &self,
        map: impl Fn(&StatCache) -> &Mutex<FxHashMap<String, V>>,
        kind: StatKind,
        path: &str,
        load: impl FnOnce() -> V,
    ) -> V {
        match self.host.get() {
            Some(host) => host.worker_lookup(map, kind, path, load),
            None => cached_stat(map(&self.own), path, load),
        }
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
        self.stats.lookup(
            |c| &c.file_exists,
            StatKind::FileExists,
            path,
            || self.fs.file_exists(path),
        )
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        let read = self.fs.read_file(path);
        if let Some(shared) = &self.keep_package_jsons
            && path.ends_with("/package.json")
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
) -> Option<PrefetchResult> {
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
        parse_source_file_detached(job.job, opts, text.clone(), job.script_kind)
    }));
    let parse = match parse {
        Ok(parse) => (parse.store.is_self_contained()
            && (synthetic_slot_count(), next_ids()) == before)
            .then_some(parse),
        Err(_) => {
            let _ = take_detached_file_store();
            None
        }
    };
    Some(PrefetchResult {
        text,
        parse,
        meta: None,
        mapped: None,
    })
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
    let job = lock(&shared.queue).by_name.get(&opts.file_name).cloned()?;
    job.mapped.as_ref()?;
    let mut state = lock(&job.state);
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
                state = job
                    .done
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let Some(job) = lock(&shared.queue).by_name.get(&opts.file_name).cloned() else {
        shared.count(|c| c.not_queued.push(opts.file_name.clone()));
        return Prefetched::Nothing;
    };
    let mut state = lock(&job.state);
    let mut waited: Option<std::time::Instant> = None;
    let result = loop {
        match std::mem::replace(&mut *state, PrefetchState::Claimed) {
            PrefetchState::Queued | PrefetchState::Claimed => {
                shared.count(|c| c.claimed.push(opts.file_name.clone()));
                return Prefetched::Nothing;
            }
            PrefetchState::Running => {
                *state = PrefetchState::Running;
                waited.get_or_insert_with(std::time::Instant::now);
                state = job
                    .done
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
