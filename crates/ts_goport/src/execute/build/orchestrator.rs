//! Go: execute/build/orchestrator.go, and `tscBuildCompilation` of
//! execute/tsc.go:90 (the `tsc -b` entry).
//!
//! The watch part (`Watch`, `updateWatch`, `resetCaches`,
//! `checkTasksForEventChanges`, `computeDesiredWatches`, `DoCycle`) is in
//! orchestrator_watch.rs.
//!
//! PORT: concurrency. Go runs `buildOrCleanProject` for the tasks of the
//! build order (`order`, or the part of it that an API build asks for,
//! ts#64158) on up to `numRoutines` goroutines (`rangeTasks`); each
//! goroutine takes the next task, waits for its upstream tasks, builds, and
//! closes the task's `built` channel. One more goroutine reports the tasks
//! in the same order, each when it is built (ts#64220). Go computes
//! `scheduleOrder` with the graph, but since ts#64158 the builders take the
//! tasks in build order. Tasks here are
//! `Rc<RefCell<BuildTask>>` on this thread, which makes, emits and releases
//! the program of every task (see build_task.rs). `build_all_tasks` keeps
//! the Go schedule: at most `numRoutines` tasks are taken and not yet
//! built, and tasks are taken and report in build order.
//! A taken task starts when its upstream tasks are done, and a task that
//! compiles makes its program at once (`build_project_start`) and starts
//! its check on the program's checker threads, so the checkers of the
//! started tasks work at the same time, as the Go goroutines do. Each
//! checker emits when its check ends (a task that checks nothing, as with
//! cached semantic diagnostics, `noCheck` or a syntax error, emits at
//! once), and the emit keeps its writes in
//! memory. The started tasks write their outputs one at a time
//! (`build_project_finish`), in the order their check and emit end, as each
//! Go builder writes when its own task ends. PORT (determinism): when tasks
//! can see each other's writes (shared_outputs.rs), all tasks finish in
//! build order instead. Tasks start before a started task writes, so a task
//! that runs beside others in Go reads the file system before they write
//! their outputs. Every task uses `o.host` and its caches (parsed `.d.ts` and
//! `.json` files, configs, the cached file system, the mtimes), as in Go.
//! Outside tests each program is released when its task is built, as Go
//! drops it there; in tests when its task reports. Its checker threads
//! free it in the background. Where Go does task work on its goroutines
//! that needs no task state, threads do it ahead of this thread: the file
//! name match of each config, and the build info read, its check parts
//! and the source mtimes of each task (config_prefetch.rs,
//! `BuildInfoPrefetch`).
//!
//! PORT: the task keeps its project statistics (see build_task.rs), and
//! `report_task` adds them to the aggregate `--diagnostics` and
//! `--extendedDiagnostics` statistics.
//!
//! PORT: testing. `opts.testing` is `None` outside tests. A test compiles
//! each started task to the end at once, in build order, so the one test
//! file system and clock see one ordered sequence (see `build_all_tasks`).

use crate::contentmapper;
use crate::execute::build::build_task::*;
use crate::execute::build::command_line::ParsedBuildCommandLine;
use crate::execute::build::config_prefetch::{
    BuildInfoRead, BuildInfoResult, BuildInfoSlot, PrefetchPool,
};
use crate::execute::build::host::BuildHost;
use crate::execute::build::shared_outputs::{PathKeys, outputs_overlap};
use crate::execute::incremental::build_info::{BuildInfo, is_build_info_file_name_default_library};
use crate::execute::incremental::incremental::new_build_info_reader;
use crate::execute::tsc::compile::{
    CommandLineResult, ExitStatus, System, Watcher, Writer, new_content_mapper_host, write_str,
};
use crate::execute::tsc::diagnostics::{
    DiagnosticReporter, DiagnosticsReporter, create_builder_status_reporter,
    create_diagnostic_reporter, create_report_error_summary, create_watch_status_reporter,
};
use crate::execute::tsc::statistics::Statistics;
use crate::execute::watchmanager::{WatchManager, new_watch_manager};
use crate::frontend::prelude::*;
use crate::gostd::Context;
// PORT: testing
use crate::execute::tsc::compile::CommandLineTesting;
use std::collections::VecDeque;
use std::sync::{Arc, PoisonError};
use std::time::SystemTime;

// Go: build/orchestrator.go:27 Options
pub struct Options {
    pub sys: Rc<dyn System>,
    pub command: Rc<ParsedBuildCommandLine>,
    // PORT: testing. `None` outside tests.
    pub testing: Option<Rc<dyn CommandLineTesting>>,
}

// Go: build/orchestrator.go:32 OrchestratorResult (ts#64158)
// PORT: Go `FilesToDelete` is nil until a task adds a file, so an empty
// `Vec` is the Go nil.
#[derive(Default)]
pub struct OrchestratorResult {
    pub result: CommandLineResult,
    pub errors: Vec<Diagnostic>,
    pub statistics: Statistics,
    pub files_to_delete: Vec<String>,
}

impl OrchestratorResult {
    /// Go `&OrchestratorResult{Result: tsc.CommandLineResult{Status: status}}`.
    fn with_status(status: ExitStatus) -> Self {
        OrchestratorResult {
            result: CommandLineResult {
                status,
                watcher: None,
            },
            ..OrchestratorResult::default()
        }
    }

    // Go: build/orchestrator.go:39 (*OrchestratorResult).report
    fn report(&mut self, o: &Orchestrator) {
        self.report_with_files_to_delete(o, true);
    }

    // Go: build/orchestrator.go:43 (*OrchestratorResult).reportWithFilesToDelete (ts#64158)
    fn report_with_files_to_delete(&mut self, o: &Orchestrator, report_files_to_delete: bool) {
        if o.opts.command.compiler_options.watch.is_true() {
            let message = if self.errors.len() == 1 {
                diag::Found_1_error_Watching_for_file_changes
            } else {
                diag::Found_0_errors_Watching_for_file_changes
            };
            (o.watch_status_reporter
                .as_ref()
                .expect("watch status reporter"))(&new_compiler_diagnostic(
                message,
                args![self.errors.len()],
            ));
        } else {
            (o.error_summary_reporter
                .as_ref()
                .expect("error summary reporter"))(&self.errors);
        }
        if report_files_to_delete && !self.files_to_delete.is_empty() {
            (o.create_builder_status_reporter())(&new_compiler_diagnostic(
                diag::A_non_dry_build_would_delete_the_following_files_Colon_0,
                args![
                    self.files_to_delete
                        .iter()
                        .map(|f| format!("\r\n * {f}"))
                        .collect::<String>()
                ],
            ));
        }
        if !o.opts.command.compiler_options.diagnostics.is_true()
            && !o
                .opts
                .command
                .compiler_options
                .extended_diagnostics
                .is_true()
        {
            return;
        }
        self.statistics.set_total_time(o.opts.sys.since_start());
        self.statistics
            .report_to(&o.opts.sys.writer(), o.opts.testing.clone());
    }
}

// Go: build/orchestrator.go:66 Orchestrator
// PORT: Go `*SyncMap` of tasks is a plain map; tasks are
// `Rc<RefCell<BuildTask>>`. Go `wm *watchmanager.WatchManager` is
// `Rc<RefCell<WatchManager>>`, so the watch loop can run while `DoCycle`
// borrows the orchestrator (see orchestrator_watch.rs).
pub struct Orchestrator {
    pub(crate) opts: Options,
    pub(crate) compare_paths_options: ComparePathsOptions,
    pub(crate) host: Rc<BuildHost>,

    // contentMapperHost transforms content-mapped files; it is created once per build session (when
    // enabled) and shared across all projects so mapper processes are consolidated. It closes itself when
    // the session context is cancelled (see contentmapper.New).
    // PORT: Go nil is `None`.
    pub(crate) content_mapper_host: Option<Rc<dyn contentmapper::Host>>,

    // order generation result
    tasks: FxHashMap<Path, Rc<RefCell<BuildTask>>>,
    pub(crate) order: Vec<String>,
    errors: Vec<Diagnostic>,
    graph_generated: bool,

    error_summary_reporter: Option<DiagnosticsReporter>,
    pub(crate) watch_status_reporter: Option<DiagnosticReporter>,

    // fswatch event-based watching
    pub(crate) wm: Rc<RefCell<WatchManager>>,
    // order sorted by dependency depth, to reduce how often builders block on upstream projects
    pub(crate) schedule_order: Vec<String>,

    // PORT: not in Go (perf). The build info files that threads read ahead
    // of the up-to-date checks of this build cycle (`BuildInfoPrefetch`).
    build_info_prefetch: RefCell<Option<BuildInfoPrefetch>>,
    // PORT: not in Go (perf). The threads that parsed the configs of the
    // graph (`start_config_prefetch`) and read build info files, from when
    // the graph is made until the build takes their reads
    // (`start_build_info_prefetch`).
    prefetch_pool: RefCell<Option<PrefetchPool>>,
    // PORT: not in Go (perf). The released programs of built tasks whose
    // frontend programs are not freed yet (`release_task_program`), oldest
    // first. Go's GC frees them in the background. Their `Rc` data frees on
    // this thread: when it would wait for a task (`build_all_tasks`), or
    // when more than `MAX_KEPT_RELEASED` wait.
    released: RefCell<VecDeque<crate::program::ReleasedProgram>>,
    // PORT: not in Go (perf). True when the process ends after this `tsc -b`
    // build (`start_exported`, not in watch mode or a test), so what the
    // build keeps is not freed.
    ends_process: std::cell::Cell<bool>,
    // PORT: not in Go (perf). The check parts that a thread made from the
    // build info that `read_build_info_file` gave last, and its file name.
    status_prefetch: RefCell<Option<(String, StatusPrefetch)>>,
}

impl Orchestrator {
    // Go: build/orchestrator.go:94 (*Orchestrator).relativeFileName
    pub fn relative_file_name(&self, file_name: &str) -> String {
        convert_to_relative_path(file_name, &self.compare_paths_options)
    }

    // Go: build/orchestrator.go:97 (*Orchestrator).toPath (at 673a5f17d713; removed by
    // ts#64159: Go N' calls caseSensitivity.PathKey)
    pub fn to_path(&self, file_name: &str) -> Path {
        to_path(
            file_name,
            &self.compare_paths_options.current_directory,
            self.compare_paths_options.use_case_sensitive_file_names,
        )
    }

    // Go: build/orchestrator.go:101 (*Orchestrator).resolveBuildInfoFileName (at
    // 673a5f17d713; ts#64159 makes it incremental.ResolveBuildInfoFileName,
    // incremental/buildInfo.go:529)
    pub fn resolve_build_info_file_name(&self, file_name: &str, build_info_dir: &str) -> String {
        if is_build_info_file_name_default_library(file_name) {
            return combine_paths(
                &CompilerHost::default_library_path(&*self.host),
                &[file_name],
            );
        }
        get_normalized_absolute_path(file_name, build_info_dir)
    }

    // Go: build/orchestrator.go:105 (*Orchestrator).Order
    pub fn order(&self) -> &[String] {
        &self.order
    }

    // Go: build/orchestrator.go:112 (*Orchestrator).ScheduleOrder (ts#64220)
    // ScheduleOrder is the order in which builders pick up projects: Order() stably sorted by dependency depth.
    pub fn schedule_order(&self) -> &[String] {
        &self.schedule_order
    }

    // Go: build/orchestrator.go:127 (*Orchestrator).computeScheduleOrder (ts#64220)
    // computeScheduleOrder sorts the build order by dependency depth (projects with no
    // upstream first, then their dependents, and so on). Builders take projects from this
    // order and block until upstream projects are done, so with the plain depth-first order
    // a builder that picks the root of a long chain sits idle while another builder works
    // through the chain, even when unrelated projects are ready to build. Depth order reduces
    // that avoidable blocking but does not eliminate it: a shallower project that has been
    // picked up may not be done yet, so a builder can take a dependent of a slow project and
    // wait on that project while a later project's upstream has already finished. The stable
    // sort preserves the original order within a depth, and reporting still follows Order().
    // PORT: Go keys `depths` by `*BuildTask`; the key is the task's `Rc`
    // pointer. A missing key is Go's zero depth.
    fn compute_schedule_order(&self) -> Vec<String> {
        struct ScheduleEntry {
            config: String,
            depth: i32,
        }
        let mut entries: Vec<ScheduleEntry> = Vec::with_capacity(self.order.len());
        let mut depths: FxHashMap<*const RefCell<BuildTask>, i32> =
            FxHashMap::with_capacity_and_hasher(self.order.len(), Default::default());
        for config in &self.order {
            let task = self.get_task(&self.to_path(config));
            let mut depth = 0;
            for upstream in &task.borrow().up_stream {
                let upstream_depth = depths
                    .get(&Rc::as_ptr(&upstream.task))
                    .copied()
                    .unwrap_or(0);
                depth = depth.max(upstream_depth + 1);
            }
            depths.insert(Rc::as_ptr(&task), depth);
            entries.push(ScheduleEntry {
                config: config.clone(),
                depth,
            });
        }
        // Go `slices.SortStableFunc`; `stable_sort_by` is stable.
        crate::gostd::slices::stable_sort_by(&mut entries, |a, b| a.depth.cmp(&b.depth));
        entries.into_iter().map(|entry| entry.config).collect()
    }

    // Go: build/orchestrator.go:150 (*Orchestrator).Upstream
    pub fn upstream(&self, config_name: &str) -> Vec<String> {
        let path = self.to_path(config_name);
        let task = self.get_task(&path);
        let task = task.borrow();
        task.up_stream
            .iter()
            .map(|t| t.task.borrow().config.clone())
            .collect()
    }

    // Go: build/orchestrator.go:158 (*Orchestrator).Downstream
    pub fn downstream(&self, config_name: &str) -> Vec<String> {
        let path = self.to_path(config_name);
        let task = self.get_task(&path);
        let task = task.borrow();
        task.down_stream
            .iter()
            .map(|t| t.borrow().config.clone())
            .collect()
    }

    // Go: build/orchestrator.go:166 (*Orchestrator).getTask
    pub fn get_task(&self, path: &Path) -> Rc<RefCell<BuildTask>> {
        match self.tasks.get(path) {
            Some(task) => task.clone(),
            None => panic!("No build task found for {}", path.as_str()),
        }
    }

    // Go: build/orchestrator.go:174 (*Orchestrator).createBuildTasks
    // PORT: Go parses the configs in parallel on a work group; here they
    // parse depth first on one thread. The task map and each task's
    // `resolved` are the same, because a path is taken by the first
    // `LoadOrStore` in both. Threads match the file names of the configs
    // ahead of this thread (`start_config_prefetch`).
    fn create_build_tasks(
        &mut self,
        old_tasks: Option<&FxHashMap<Path, Rc<RefCell<BuildTask>>>>,
        configs: &[String],
    ) {
        for config in configs {
            let path = self.to_path(config);
            let mut task: Option<Rc<RefCell<BuildTask>>> = None;
            let mut build_info: Option<BuildInfoEntry> = None;
            if let Some(old_tasks) = old_tasks {
                if let Some(existing) = old_tasks.get(&path) {
                    if !existing.borrow().dirty {
                        // Reuse existing task if config is same
                        task = Some(existing.clone());
                    } else {
                        if let Some(project) = &existing.borrow().content_mapper_project {
                            let _ = project.close();
                        }
                        build_info = existing.borrow().build_info_entry.clone();
                    }
                }
            }
            let task = task.unwrap_or_else(|| {
                let mut task = BuildTask::new(config.clone(), old_tasks.is_none());
                task.build_info_entry = build_info;
                Rc::new(RefCell::new(task))
            });
            if self.tasks.contains_key(&path) {
                continue;
            }
            self.tasks.insert(path.clone(), task.clone());
            // Go runs this on a work group (orchestrator.go:178) that is
            // parallel unless `--singleThreaded` (:263), so a Go panic in the
            // parse prints ` [recovered, repanicked]` there.
            let resolved = if self.opts.command.compiler_options.single_threaded.is_true() {
                self.host.get_resolved_project_reference(config, &path)
            } else {
                crate::core::go_work_group_task(|| {
                    self.host.get_resolved_project_reference(config, &path)
                })
            };
            {
                let mut task = task.borrow_mut();
                task.resolved = resolved.clone();
                task.up_stream = Vec::new();
            }
            if let Some(resolved) = resolved {
                let references = resolved.resolved_project_reference_paths().to_vec();
                if old_tasks.is_none() {
                    self.start_config_prefetch(&references);
                    if let Some(prefetch) = &*self.host.config_prefetch.borrow() {
                        prefetch.queue_read(&path, &resolved);
                    }
                }
                self.create_build_tasks(old_tasks, &references);
            }
        }
    }

    /// PORT: not in Go (perf). Queues the parses of `references` on the
    /// prefetch threads (config_prefetch.rs), and starts the threads when
    /// there are two references or more. Go parses the configs on a work
    /// group that is parallel unless `--singleThreaded`. Only for the first
    /// graph of a build that is not a watch (a watch parses changed configs
    /// again), and only on the OS file system (not in tests). The threads
    /// also read the build info file of each config when the build can use
    /// the reads (`start_build_info_prefetch`).
    fn start_config_prefetch(&self, references: &[String]) {
        if let Some(prefetch) = &*self.host.config_prefetch.borrow() {
            prefetch.queue(references);
            return;
        }
        let options = &self.opts.command.compiler_options;
        if references.len() < 2
            || options.watch.is_true()
            || options.single_threaded.is_true()
            || self.opts.testing.is_some()
            || !is_wrapped_os_fs(&self.opts.sys.fs())
        {
            return;
        }
        let reads_build_info = !self.opts.command.build_options.clean.is_true()
            && !self.opts.command.build_options.force.is_true()
            && self.num_routines() >= 2;
        *self.host.config_prefetch.borrow_mut() = PrefetchPool::start(
            (**options).clone(),
            self.host.command_line_raw(),
            &self.compare_paths_options,
            reads_build_info,
            references,
        );
    }

    // Go: build/orchestrator.go:210 (*Orchestrator).setupBuildTask
    // PORT: the Go `built` and `done` channels are dropped (see top).
    fn setup_build_task(
        &mut self,
        config_name: &str,
        down_stream: Option<&Rc<RefCell<BuildTask>>>,
        in_circular_context: bool,
        completed: &mut FxHashSet<Path>,
        analyzing: &mut FxHashSet<Path>,
        circularity_stack: &mut Vec<String>,
    ) -> Option<Rc<RefCell<BuildTask>>> {
        let path = self.to_path(config_name);
        let task = self.get_task(&path);
        if !completed.contains(&path) {
            if analyzing.contains(&path) {
                if !in_circular_context {
                    self.errors.push(new_compiler_diagnostic(
                        diag::Project_references_may_not_form_a_circular_graph_Cycle_detected_Colon_0,
                        args![circularity_stack.join("\n")],
                    ));
                }
                return None;
            }
            analyzing.insert(path.clone());
            circularity_stack.push(config_name.to_string());
            let resolved = task.borrow().resolved.clone();
            if let Some(resolved) = resolved {
                let references = resolved.resolved_project_reference_paths().to_vec();
                for (index, sub_reference) in references.iter().enumerate() {
                    let upstream = self.setup_build_task(
                        sub_reference,
                        Some(&task),
                        in_circular_context || resolved.project_references()[index].circular,
                        completed,
                        analyzing,
                        circularity_stack,
                    );
                    if let Some(upstream) = upstream {
                        task.borrow_mut().up_stream.push(UpstreamTask {
                            task: upstream,
                            ref_index: index,
                        });
                    }
                }
            }
            circularity_stack.pop();
            completed.insert(path);
            self.order.push(config_name.to_string());
        }
        if self.opts.command.compiler_options.watch.is_true() {
            if let Some(down_stream) = down_stream {
                task.borrow_mut().down_stream.push(down_stream.clone());
            }
        }
        Some(task)
    }

    // Go: build/orchestrator.go:252 (*Orchestrator).GenerateGraphReusingOldTasks
    pub fn generate_graph_reusing_old_tasks(&mut self) {
        let tasks = std::mem::take(&mut self.tasks);
        // PORT: Go appends to the `downStream` of a reused task in each new
        // graph (`setupBuildTask`), so the list keeps the downstream tasks
        // of every older graph, and those keep their upstream tasks. Here
        // that is an `Rc` cycle: each config change of a downstream project
        // kept its old task (about 0.5 MiB for query-persist-client-core in
        // `tsc -b -w`) for the rest of the session. So the lists are cleared,
        // and `setup_build_task` fills them again from the new graph. The
        // output does not change: a task of an older graph is not built
        // again, and `update_downstream` gives a task that a list has twice
        // the same status as one update.
        for task in tasks.values() {
            task.borrow_mut().down_stream.clear();
        }
        self.order = Vec::new();
        self.errors = Vec::new();
        self.generate_graph(Some(&tasks));
    }

    // Go: build/orchestrator.go:260 (*Orchestrator).GenerateGraph (ts#64220, ts#64158)
    pub fn generate_graph(&mut self, old_tasks: Option<&FxHashMap<Path, Rc<RefCell<BuildTask>>>>) {
        let projects = self.opts.command.resolved_project_paths().to_vec();
        // Parse all config files in parallel
        self.create_build_tasks(old_tasks, &projects);
        // The prefetch threads parse no more configs. Their build info reads
        // go on until the build takes them.
        let prefetch = self.host.config_prefetch.borrow_mut().take();
        if let Some(prefetch) = &prefetch {
            prefetch.close_configs();
        }
        *self.prefetch_pool.borrow_mut() = prefetch;

        // Generate the graph
        let mut completed = FxHashSet::default();
        let mut analyzing = FxHashSet::default();
        let mut circularity_stack = Vec::new();
        for project in &projects {
            self.setup_build_task(
                project,
                None,
                false,
                &mut completed,
                &mut analyzing,
                &mut circularity_stack,
            );
        }
        self.schedule_order = self.compute_schedule_order();
        if let Some(old_tasks) = old_tasks {
            for (path, old_task) in old_tasks {
                if self
                    .tasks
                    .get(path)
                    .is_some_and(|task| Rc::ptr_eq(task, old_task))
                {
                    continue;
                }
                if let Some(project) = &old_task.borrow().content_mapper_project {
                    let _ = project.close();
                }
            }
        }
        self.graph_generated = true;
    }

    // Go: build/orchestrator.go:290 (*Orchestrator).Start
    // tsc -b entrypoint
    // PORT: Go `start` sets `result.Result.Watcher = o` in watch mode. The
    // watcher is the boxed orchestrator, so `Start` sets it after `start`.
    // `Watch` blocks in the watch loop until `ctx` ends
    // (orchestrator_watch.rs).
    pub fn start_exported(mut self: Box<Self>, ctx: &Context) -> CommandLineResult {
        // PORT: not in Go (perf). The process ends after `tsc -b`, and Go
        // does not free at exit: the orchestrator, its caches and the kept
        // released programs (`released`) are not freed. With a content
        // mapper host the orchestrator drops, as its projects close then.
        let ends_process =
            !self.opts.command.compiler_options.watch.is_true() && self.opts.testing.is_none();
        self.ends_process.set(ends_process);
        let mut result = self.start(ctx, "", false /*onlyReferences*/).result;
        // PORT: the wasm test is not in Go. A wasm build ends a watch build
        // before this (execute_tsc.rs `wasm_unsupported`), so the wasm
        // module leaves the watch code out.
        if !cfg!(target_family = "wasm") && self.opts.command.compiler_options.watch.is_true() {
            result.watcher = Some(self as Box<dyn Watcher>);
        } else if ends_process && self.content_mapper_host.is_none() {
            std::mem::forget(self);
        }
        result
    }

    // Go: build/orchestrator.go:295 (*Orchestrator).Build (ts#64158)
    // orchestrator.Build() entrypoint for api
    // PORT: in watch mode the result has no watcher (see `start_exported`).
    pub fn build(&mut self, ctx: &Context, project: &str) -> OrchestratorResult {
        self.recheck_all_projects(project);
        self.start(ctx, project, false /*onlyReferences*/)
    }

    // Go: build/orchestrator.go:301 (*Orchestrator).BuildReferences (ts#64158)
    // orchestrator.BuildReferences() entrypoint for api
    pub fn build_references(&mut self, ctx: &Context, project: &str) -> OrchestratorResult {
        self.recheck_all_projects(project);
        self.start(ctx, project, true /*onlyReferences*/)
    }

    // Go: build/orchestrator.go:306 (*Orchestrator).start (ts#64158)
    // PORT: Go `defer o.contentMapperHost.Close()` runs at each return; the
    // returns break out of the `'start` block, and the close follows it.
    // Go also sets `result.Result.Watcher = o` (see `start_exported`).
    fn start(&mut self, ctx: &Context, project: &str, only_references: bool) -> OrchestratorResult {
        self.content_mapper_host =
            new_content_mapper_host(ctx, &self.opts.sys, &self.opts.command.compiler_options);
        let close_content_mapper_host = self.content_mapper_host.clone().filter(|_| {
            !self.opts.command.compiler_options.watch.is_true() || self.opts.testing.is_none()
        });
        let result = 'start: {
            if self.opts.command.compiler_options.watch.is_true() {
                // PORT: not in Go. From the second build of a file on, its
                // new parse is a freeable file version (watchfree1, see
                // `Watcher::start`), and a source file keeps its parse while
                // it does not change (`BuildHost::watch_source_file`).
                crate::ast::set_watch_process();
                self.host.watch_sources.replace(Some(FxHashMap::default()));
                (self
                    .watch_status_reporter
                    .as_ref()
                    .expect("watch status reporter"))(&new_compiler_diagnostic(
                    diag::Starting_compilation_in_watch_mode,
                    args![],
                ));
            }
            if self.graph_generated {
                self.generate_graph_reusing_old_tasks();
            } else {
                self.generate_graph(None);
            }
            let (mut order, ok) = self.get_build_order_for(project);
            if !ok {
                break 'start OrchestratorResult::with_status(
                    ExitStatus::InvalidProjectOutputsSkipped,
                );
            }
            if only_references && self.errors.is_empty() {
                if project.is_empty() {
                    break 'start OrchestratorResult::with_status(
                        ExitStatus::InvalidProjectOutputsSkipped,
                    );
                }
                // Go `order[:len(order)-1]`: the project itself is last.
                order.pop();
            }
            let result = self.build_or_clean_order(&order);
            // PORT: the wasm test is not in Go (see `start_exported`).
            if !cfg!(target_family = "wasm") && self.opts.command.compiler_options.watch.is_true() {
                self.watch(ctx);
            }
            result
        };
        // A build that did not take the prefetch reads drops them.
        self.prefetch_pool.borrow_mut().take();
        if let Some(host) = close_content_mapper_host {
            let _ = host.close();
        }
        result
    }

    // Go: build/orchestrator.go:337 (*Orchestrator).recheckAllProjects (ts#64158)
    fn recheck_all_projects(&self, project: &str) {
        if !self.graph_generated {
            return;
        }
        let (order, ok) = self.get_build_order_for(project);
        if !ok {
            return;
        }
        self.range_tasks(
            &order,
            &mut |_path: &Path, task: &Rc<RefCell<BuildTask>>| {
                let mut task = task.borrow_mut();
                task.reset_status();
                let path = self.to_path(&task.config);
                task.reset_config(self, &path);
            },
        );
        *self
            .host
            .m_times
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = FxHashMap::default();
        self.reset_caches();
    }

    // Go: build/orchestrator.go:354 (*Orchestrator).Clean (ts#64158)
    // orchestrator.Clean() entrypoint for api
    pub fn clean_exported(&mut self, project: &str) -> OrchestratorResult {
        self.clean(project, false)
    }

    // Go: build/orchestrator.go:359 (*Orchestrator).CleanReferences (ts#64158)
    // orchestrator.CleanReferences() entrypoint for api
    pub fn clean_references(&mut self, project: &str) -> OrchestratorResult {
        self.clean(project, true)
    }

    // Go: build/orchestrator.go:363 (*Orchestrator).clean (ts#64158)
    // PORT: Go `task.buildInfoEntryMu` guards the entry; the task is on
    // this thread (build_task.rs).
    fn clean(&mut self, project: &str, only_references: bool) -> OrchestratorResult {
        if !self.graph_generated {
            self.generate_graph(None);
        }
        // PORT: not in Go (perf). A clean reads no build info file.
        self.prefetch_pool.borrow_mut().take();
        if !self.errors.is_empty() {
            let mut result = OrchestratorResult {
                result: CommandLineResult {
                    status: ExitStatus::ProjectReferenceCycleOutputsSkipped,
                    watcher: None,
                },
                errors: self.errors.clone(),
                ..OrchestratorResult::default()
            };
            result.report_with_files_to_delete(self, true);
            return result;
        }

        let (mut order, ok) = self.get_build_order_for(project);
        if !ok {
            return OrchestratorResult::with_status(ExitStatus::InvalidProjectOutputsSkipped);
        }
        if only_references {
            // Go `order[:len(order)-1]` panics on an empty order.
            assert!(!order.is_empty(), "slice bounds out of range [:-1]");
            order.pop();
        }

        let mut result = OrchestratorResult::default();
        result.statistics.projects = order.len() as i32;
        let dry = self.opts.command.build_options.dry.is_true();
        let report_diagnostic = self.create_diagnostic_reporter();
        for config in &order {
            let task = self.get_task(&self.to_path(config));
            let mut task = task.borrow_mut();
            let Some(resolved) = task.resolved.clone() else {
                let diagnostic =
                    new_compiler_diagnostic(diag::File_0_not_found, args![task.config.clone()]);
                report_diagnostic(&diagnostic);
                result.errors.push(diagnostic);
                continue;
            };

            let inputs: FxHashSet<Path> = resolved
                .file_names()
                .iter()
                .map(|file_name| self.to_path(file_name))
                .collect();
            let project_outputs = resolved.get_output_file_names();
            let mut deleted = false;
            for output_file in project_outputs {
                deleted = self.clean_project_output(
                    &output_file,
                    &inputs,
                    dry,
                    &mut result.files_to_delete,
                    &report_diagnostic,
                ) || deleted;
            }
            deleted = self.clean_project_output(
                &resolved.get_build_info_file_name(),
                &inputs,
                dry,
                &mut result.files_to_delete,
                &report_diagnostic,
            ) || deleted;
            if deleted {
                task.reset_status();
                task.build_info_entry = None;
            }
        }

        result.report_with_files_to_delete(self, dry);
        result
    }

    // Go: build/orchestrator.go:417 (*Orchestrator).getBuildOrderFor (ts#64158)
    // PORT: Go returns `o.order` itself for an empty project; this clones it.
    fn get_build_order_for(&self, project: &str) -> (Vec<String>, bool) {
        if project.is_empty() {
            return (self.order.clone(), true);
        }

        let config = resolve_config_file_name_of_project_reference(&resolve_path(
            &self.opts.sys.get_current_directory(),
            &[project],
        ));
        let Some(target) = self.tasks.get(&self.to_path(&config)).cloned() else {
            return (Vec::new(), false);
        };

        let mut projects: FxHashSet<Path> = FxHashSet::default();
        fn add_project_and_references(
            o: &Orchestrator,
            projects: &mut FxHashSet<Path>,
            task: &Rc<RefCell<BuildTask>>,
        ) {
            let task = task.borrow();
            let path = o.to_path(&task.config);
            if projects.contains(&path) {
                return;
            }
            projects.insert(path);
            for upstream in &task.up_stream {
                add_project_and_references(o, projects, &upstream.task);
            }
        }
        add_project_and_references(self, &mut projects, &target);

        let mut order: Vec<String> = Vec::with_capacity(projects.len());
        for config in &self.order {
            if projects.contains(&self.to_path(config)) {
                order.push(config.clone());
            }
        }
        (order, true)
    }

    // Go: build/orchestrator.go:452 (*Orchestrator).cleanProjectOutput (ts#64158)
    fn clean_project_output(
        &self,
        output_file: &str,
        inputs: &FxHashSet<Path>,
        dry: bool,
        files_to_delete: &mut Vec<String>,
        report_diagnostic: &DiagnosticReporter,
    ) -> bool {
        let fs = CompilerHost::fs(&*self.host);
        if output_file.is_empty()
            || inputs.contains(&self.to_path(output_file))
            || !fs.file_exists(output_file)
        {
            return false;
        }
        files_to_delete.push(output_file.to_string());
        if dry {
            return false;
        }
        if fs.remove(output_file).is_err() {
            report_diagnostic(&new_compiler_diagnostic(
                diag::Failed_to_delete_file_0,
                args![output_file],
            ));
            return false;
        }
        true
    }

    // Go: build/orchestrator.go:865 (*Orchestrator).buildOrClean
    pub(crate) fn build_or_clean(&mut self) -> CommandLineResult {
        let order = self.order.clone();
        self.build_or_clean_order(&order).result
    }

    // Go: build/orchestrator.go:869 (*Orchestrator).buildOrCleanOrder (ts#64158)
    fn build_or_clean_order(&mut self, order: &[String]) -> OrchestratorResult {
        if !self.opts.command.build_options.clean.is_true()
            && self.opts.command.build_options.verbose.is_true()
        {
            (self.create_builder_status_reporter())(&new_compiler_diagnostic(
                diag::Projects_in_this_build_Colon_0,
                args![
                    order
                        .iter()
                        .map(|p| format!("\r\n    * {}", self.relative_file_name(p)))
                        .collect::<String>()
                ],
            ));
        }
        let mut build_result = OrchestratorResult::default();
        if self.errors.is_empty() {
            build_result.statistics.projects = order.len() as i32;
            self.build_all_tasks(order, &mut build_result);
        } else {
            // Circularity errors prevent any project from being built
            build_result.result.status = ExitStatus::ProjectReferenceCycleOutputsSkipped;
            let report_diagnostic = self.create_diagnostic_reporter();
            for err in &self.errors {
                report_diagnostic(err);
            }
            build_result.errors = self.errors.clone();
        }
        build_result.report(self);
        build_result
    }

    // Go: build/orchestrator.go:918 the numRoutines part of (*Orchestrator).rangeTasks
    pub(crate) fn num_routines(&self) -> i64 {
        let mut num_routines = 4;
        if self.opts.command.compiler_options.single_threaded.is_true() {
            num_routines = 1;
        } else if let Some(builders) = self.opts.command.build_options.builders {
            num_routines = builders;
        }
        num_routines
    }

    // Go: build/orchestrator.go:917 (*Orchestrator).rangeTasks over `order`
    // with build/orchestrator.go:951 (*Orchestrator).buildOrCleanProject, and
    // the reporting goroutine of build/orchestrator.go:873 buildOrCleanOrder
    // (ts#64220, ts#64158).
    // PORT: see the top comment for the schedule. Go `numRoutines <= 0`
    // starts no builder, so no task runs; that is kept.
    fn build_all_tasks(&self, order: &[String], build_result: &mut OrchestratorResult) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            NotTaken,
            Waiting,
            Compiling,
            Done,
        }
        let num_routines = self.num_routines();
        if num_routines <= 0 {
            return;
        }
        let num_routines = num_routines as usize;
        let clean = self.opts.command.build_options.clean.is_true();
        // PORT: testing (see the top comment)
        let testing = self.opts.testing.is_some();
        let paths: Vec<Path> = order.iter().map(|c| self.to_path(c)).collect();
        let pool = self.prefetch_pool.borrow_mut().take();
        self.host.written.clear();
        if !clean {
            *self.build_info_prefetch.borrow_mut() = self.start_build_info_prefetch(&paths, pool);
        }
        let index_of: FxHashMap<Path, usize> = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        let mut states = vec![State::NotTaken; paths.len()];
        // PORT: perf. Each checker thread of a compiling task's program
        // drops a `ReadySignal` of the task when the check and emit that it
        // started are done (`BuildTask::notify_when_compiled`); `signals`
        // counts the signals that each task still waits for. `compiled`
        // holds the compiling tasks whose signals have all arrived, in the
        // order their last signal arrived: the order in which their checks
        // and emits ended.
        let (ready, ready_calls) = std::sync::mpsc::channel::<usize>();
        let mut signals = vec![0usize; paths.len()];
        let mut compiled = VecDeque::new();
        fn signal_arrived(signals: &mut [usize], compiled: &mut VecDeque<usize>, index: usize) {
            signals[index] -= 1;
            if signals[index] == 0 {
                compiled.push_back(index);
            }
        }
        // PORT: not in Go (determinism). True when the tasks can see each
        // other's writes (`outputs_overlap`), so they finish in build order.
        // It is found when the first task compiles: until then every task
        // was done when it started. With one builder the order is the same.
        let mut in_build_order = false;
        let mut overlap_checked = false;
        // Tasks taken (Go `currentTaskIndex`), taken and not built, and
        // reported. The tasks before `next_report` are built.
        let mut next_take = 0;
        let mut in_flight = 0;
        let mut next_report = 0;
        while next_report < paths.len() {
            // Each free builder takes the next task in order.
            while in_flight < num_routines && next_take < paths.len() {
                states[next_take] = State::Waiting;
                next_take += 1;
                in_flight += 1;
            }
            // A taken task starts once its upstream tasks are done
            // (Go `waitOnUpstream`; `cleanProject` does not wait). A task
            // that compiles makes its program now. A built task frees its
            // builder (Go `close(task.built)`).
            let mut progressed = false;
            for index in next_report..next_take {
                if states[index] != State::Waiting {
                    continue;
                }
                let task = self.get_task(&paths[index]);
                if !clean {
                    let upstream_done = task.borrow().up_stream.iter().all(|upstream| {
                        let path = self.to_path(&upstream.task.borrow().config);
                        index_of
                            .get(&path)
                            .is_none_or(|&i| states[i] == State::Done)
                    });
                    if !upstream_done {
                        continue;
                    }
                }
                let mut task = task.borrow_mut();
                task.result = Some(TaskResult::new(
                    self.create_task_builder_status_reporter(),
                    self.create_task_diagnostic_reporter(),
                ));
                states[index] = if clean {
                    task.clean_project(self, &paths[index]);
                    State::Done
                } else if !task.build_project_start(self, &paths[index]) {
                    State::Done
                } else if testing {
                    task.build_project_finish(self, &paths[index]);
                    State::Done
                } else {
                    signals[index] = task.notify_when_compiled(|| ReadySignal {
                        index,
                        ready: ready.clone(),
                    });
                    if signals[index] == 0 {
                        compiled.push_back(index);
                    }
                    State::Compiling
                };
                if states[index] == State::Done {
                    self.task_built(&mut task);
                    in_flight -= 1;
                    progressed = true;
                }
                drop(task);
                if states[index] == State::Compiling && !overlap_checked && num_routines > 1 {
                    overlap_checked = true;
                    in_build_order = self.outputs_overlap(&paths);
                }
            }
            // Tasks report in order, each when it is built.
            while next_report < paths.len() && states[next_report] == State::Done {
                let task = self.get_task(&paths[next_report]);
                self.report_task(&mut task.borrow_mut(), build_result);
                next_report += 1;
                progressed = true;
            }
            if progressed {
                continue;
            }
            // No task can start or report, so a taken task compiles (the
            // first task that is not built, `next_report`, has its upstream
            // tasks done). A Go builder writes the outputs of its task
            // when the task's check ends, and then takes the next task. So
            // the task whose started check and emit ended first finishes
            // now: it writes its outputs, and its builder takes the next
            // task. When the outputs overlap, only the first task that is not
            // built finishes, as in Go when the tasks end in build order.
            // When no task can finish yet, this waits for a signal, and frees
            // a kept released program first.
            let index = loop {
                while let Ok(index) = ready_calls.try_recv() {
                    signal_arrived(&mut signals, &mut compiled, index);
                }
                let can_finish = |&index: &usize| !in_build_order || index == next_report;
                if let Some(at) = compiled.iter().position(can_finish) {
                    break compiled.remove(at).expect("the position is in the queue");
                }
                if !self.free_released() {
                    let index = ready_calls.recv().expect("this thread keeps a sender");
                    signal_arrived(&mut signals, &mut compiled, index);
                }
            };
            let task = self.get_task(&paths[index]);
            let mut task = task.borrow_mut();
            task.build_project_finish(self, &paths[index]);
            states[index] = State::Done;
            self.task_built(&mut task);
            in_flight -= 1;
        }
        // The kept released programs free now, unless the process ends
        // after this build (`start_exported`).
        if !self.ends_process.get() {
            while self.free_released() {}
        }
        // A task that did not read its build info leaves its read unused.
        self.build_info_prefetch.borrow_mut().take();
        self.status_prefetch.borrow_mut().take();
        // No prefetched mtime outlives this build (see `drop_status_prefetch`).
        for path in &paths {
            self.get_task(path).borrow_mut().drop_status_prefetch();
        }
        self.host.written.clear();
    }

    // Go: build/orchestrator.go:951 (*Orchestrator).buildOrCleanProject,
    // after the build (ts#64220).
    fn task_built(&self, task: &mut BuildTask) {
        if self.opts.testing.is_none() {
            // The program is only needed by Testing.OnProgram at report time; drop it now so a task
            // that has finished but is not yet reported does not keep its program alive.
            if let Some(program) = task
                .result
                .as_mut()
                .and_then(|result| result.program.take())
            {
                self.keep_released(release_task_program(program));
            }
        }
    }

    /// PORT: not in Go (determinism). `outputs_overlap` for the tasks at
    /// `paths` (the build order of `build_all_tasks`).
    fn outputs_overlap(&self, paths: &[Path]) -> bool {
        let configs: Vec<_> = paths
            .iter()
            .map(|path| self.get_task(path).borrow().resolved.clone())
            .collect();
        // The file system without the build host's cache: that cache keeps
        // each lookup for the whole build, and this one looks up output
        // directories that do not exist yet.
        outputs_overlap(&configs, &self.opts.sys.fs(), &self.compare_paths_options)
    }

    /// PORT: not in Go (perf). Keeps `released` to free later (see
    /// `released`).
    fn keep_released(&self, released: crate::program::ReleasedProgram) {
        let oldest = {
            let mut kept = self.released.borrow_mut();
            kept.push_back(released);
            (kept.len() > MAX_KEPT_RELEASED).then(|| kept.pop_front())
        };
        drop(oldest);
    }

    /// PORT: not in Go (perf). Frees the oldest kept released program.
    /// False when none is kept.
    fn free_released(&self) -> bool {
        let oldest = self.released.borrow_mut().pop_front();
        oldest.is_some()
    }

    /// PORT: not in Go (perf). Reads the build info files that the
    /// up-to-date checks of the tasks at `paths` will read (see
    /// `BuildInfoPrefetch`), on the threads of `pool` (which read some of
    /// them while the graph was made), or else on new ones. None
    /// when there is nothing to gain or the read could differ from the task's
    /// own read: one routine (`--singleThreaded` or `--builders 1`: Go
    /// checks one task at a time), `--force` (no check reads the build
    /// info), a file system other than the OS one (tests), or no `pool` and
    /// fewer than two files. A solution (Go `upToDateStatusTypeSolution`) and a task
    /// that keeps the build info of an earlier cycle (watch) read nothing.
    /// A build info file that two tasks name is left out, so no other task
    /// writes it as its build info. Another task can write it as an
    /// ordinary output: then the check reads it again
    /// (`read_build_info_file`). Names are
    /// compared by key (`PathKeys::build_info_key`, as `outputs_overlap`
    /// does), so two names of one file through a symbolic link, even one
    /// that does not resolve before the build writes the file, are one file.
    /// When a build info key cannot be trusted (a file with more than one
    /// hard link, or a link target with `..` after a name), that file can
    /// be any build info file of the build, so nothing is read.
    /// The threads also make the check parts of each build info, with the
    /// mtimes of its task's TypeScript sources (`StatusPrefetch`).
    fn start_build_info_prefetch(
        &self,
        paths: &[Path],
        pool: Option<PrefetchPool>,
    ) -> Option<BuildInfoPrefetch> {
        let num_routines = usize::try_from(self.num_routines()).unwrap_or(0);
        if num_routines < 2
            || self.opts.command.build_options.force.is_true()
            || !is_wrapped_os_fs(&self.opts.sys.fs())
        {
            return None;
        }
        // The file system without the build host's cache (see
        // `outputs_overlap`).
        let fs = self.opts.sys.fs();
        let keys = PathKeys::new(&fs, &self.compare_paths_options);
        let mut named: FxHashMap<String, usize> = FxHashMap::default();
        let mut reads: Vec<(String, Path, BuildInfoRead)> = Vec::new();
        for path in paths {
            let task = self.get_task(path);
            let task = task.borrow();
            let Some(resolved) = &task.resolved else {
                continue;
            };
            let name = resolved.get_build_info_file_name();
            if name.is_empty() {
                continue;
            }
            // PORT: perf. A prefetch thread found most keys before the graph
            // was made, as this does.
            let build_info_key = match pool
                .as_ref()
                .and_then(|pool| pool.build_info_key(path, &name))
            {
                Some(key) => key,
                None => keys.build_info_key(&name),
            }?;
            *named.entry(build_info_key.clone()).or_default() += 1;
            let build_info_path = self.to_path(&name);
            let keeps = task
                .build_info_entry
                .as_ref()
                .is_some_and(|entry| entry.path == build_info_path);
            if !keeps && let Some(read) = BuildInfoRead::of(resolved) {
                reads.push((build_info_key, path.clone(), read));
            }
        }
        let reads: Vec<(Path, BuildInfoRead)> = reads
            .into_iter()
            .filter_map(|(key, path, read)| (named[&key] == 1).then_some((path, read)))
            .collect();
        if reads.is_empty() || (pool.is_none() && reads.len() < 2) {
            return None;
        }
        // The checks store about this many mtimes; the map grows once.
        let inputs: usize = reads.iter().map(|(_, read)| read.input_files.len()).sum();
        self.host
            .m_times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reserve(inputs);
        let pool = pool.unwrap_or_else(|| PrefetchPool::start_reads(&self.compare_paths_options));
        let slots = pool.finish_reads(reads)?;
        Some(BuildInfoPrefetch { slots })
    }

    // Go: build/buildtask.go:121 (*BuildTask).report, the orchestrator part
    // (see `BuildTask::report`).
    fn report_task(&self, task: &mut BuildTask, build_result: &mut OrchestratorResult) {
        let (result, errors) = task.report();
        if !errors.is_empty() {
            build_result.errors.extend(errors);
        }
        write_str(&self.opts.sys.writer(), &result.builder);
        if result.exit_status.code() > build_result.result.status.code() {
            build_result.result.status = result.exit_status;
        }
        if let Some(statistics) = &result.statistics {
            build_result.statistics.aggregate(statistics);
        }
        // If we built the program, or updated timestamps, or had errors, we need to
        // delete files that are no longer needed
        match result.build_kind {
            BuildKind::Program => {
                // PORT: testing. The program is current for the call, as
                // the test reads its files.
                if let (Some(testing), Some(program)) = (&self.opts.testing, &result.program) {
                    let _scope = crate::core::enter_program(Some(program.get_program()));
                    testing.on_program(program);
                }
                build_result.statistics.projects_built += 1
            }
            BuildKind::Pseudo => build_result.statistics.timestamp_updates += 1,
            BuildKind::None => {}
        }
        build_result.files_to_delete.extend(result.files_to_delete);
        // Go drops `t.result` here (`t.result = nil`).
        if let Some(program) = result.program {
            self.keep_released(release_task_program(program));
        }
    }

    // Go: build/orchestrator.go:968 (*Orchestrator).getWriter with a nil task
    fn writer(&self) -> Writer {
        self.opts.sys.writer()
    }

    // Go: build/orchestrator.go:975 (*Orchestrator).createBuilderStatusReporter(nil)
    fn create_builder_status_reporter(&self) -> DiagnosticReporter {
        create_builder_status_reporter(
            self.opts.sys.clone(),
            self.writer(),
            &self.opts.command.locale(),
            &self.opts.command.compiler_options,
            self.opts.testing.clone(),
        )
    }

    // Go: build/orchestrator.go:979 (*Orchestrator).createDiagnosticReporter(nil)
    fn create_diagnostic_reporter(&self) -> DiagnosticReporter {
        create_diagnostic_reporter(
            &*self.opts.sys,
            self.writer(),
            &self.opts.command.locale(),
            &self.opts.command.compiler_options,
        )
    }

    // Go: build/orchestrator.go:975 (*Orchestrator).createBuilderStatusReporter(task)
    fn create_task_builder_status_reporter(&self) -> TaskDiagnosticReporter {
        task_reporter(|w| {
            create_builder_status_reporter(
                self.opts.sys.clone(),
                w,
                &self.opts.command.locale(),
                &self.opts.command.compiler_options,
                self.opts.testing.clone(),
            )
        })
    }

    // Go: build/orchestrator.go:979 (*Orchestrator).createDiagnosticReporter(task)
    fn create_task_diagnostic_reporter(&self) -> TaskDiagnosticReporter {
        task_reporter(|w| {
            create_diagnostic_reporter(
                &*self.opts.sys,
                w,
                &self.opts.command.locale(),
                &self.opts.command.compiler_options,
            )
        })
    }
}

// PORT: Go `getWriter(task)` gives the reporter `&task.result.builder`. A
// `TaskDiagnosticReporter` gets the builder on each call instead (see
// build_task.rs), so the tsc reporter writes into its own buffer, and the
// buffer moves into the builder after each call.
fn task_reporter(make: impl FnOnce(Writer) -> DiagnosticReporter) -> TaskDiagnosticReporter {
    let buffer: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let reporter = make(buffer.clone());
    Box::new(move |builder: &mut String, diagnostic: &Diagnostic| {
        reporter(diagnostic);
        let bytes = std::mem::take(&mut *buffer.borrow_mut());
        builder.push_str(&String::from_utf8_lossy(&bytes));
    })
}

impl BuildTaskOrchestrator for Orchestrator {
    fn command(&self) -> &ParsedBuildCommandLine {
        &self.opts.command
    }

    fn compare_paths_options(&self) -> &ComparePathsOptions {
        &self.compare_paths_options
    }

    fn relative_file_name(&self, file_name: &str) -> String {
        Orchestrator::relative_file_name(self, file_name)
    }

    fn to_path(&self, file_name: &str) -> Path {
        Orchestrator::to_path(self, file_name)
    }

    fn now(&self) -> SystemTime {
        self.opts.sys.now()
    }

    fn fs(&self) -> Rc<dyn Fs> {
        CompilerHost::fs(&*self.host)
    }

    fn get_m_time(&self, file: &str) -> Option<SystemTime> {
        self.host.get_m_time(file)
    }

    fn get_m_time_of_path(
        &self,
        file: &str,
        path: &Path,
        prefetched: Option<Option<SystemTime>>,
    ) -> Option<SystemTime> {
        self.host.get_m_time_of_path(file, path, prefetched)
    }

    fn set_m_time(&self, file: &str, m_time: SystemTime) -> Result<(), FsError> {
        self.host.set_m_time(file, Some(m_time))
    }

    fn store_m_time(&self, file: &str, m_time: SystemTime) {
        self.host.store_m_time(file, Some(m_time));
    }

    // PORT: when a task of this build wrote the build info file after the
    // prefetch (another project's output with this name), the check reads
    // it again, as Go reads it in the check (`BuildHost::written`).
    fn read_build_info_file(&self, config: &ParsedCommandLine) -> Option<Rc<BuildInfo>> {
        let name = config.get_build_info_file_name();
        let prefetched = self
            .build_info_prefetch
            .borrow_mut()
            .as_mut()
            .and_then(|prefetch| prefetch.take(&name))
            .filter(|_| !self.host.was_written(&self.to_path(&name)));
        if let Some((build_info, status_prefetch)) = prefetched {
            *self.status_prefetch.borrow_mut() = status_prefetch.map(|status| (name, status));
            return build_info.map(Rc::new);
        }
        new_build_info_reader(self.host.clone() as Rc<dyn CompilerHost>)
            .read_build_info(config)
            .map(Rc::new)
    }

    fn take_status_prefetch(&self, build_info_file_name: &str) -> Option<StatusPrefetch> {
        let (name, status_prefetch) = self.status_prefetch.borrow_mut().take()?;
        (name == build_info_file_name).then_some(status_prefetch)
    }

    fn sys(&self) -> Rc<dyn System> {
        self.opts.sys.clone()
    }

    fn host(&self) -> Rc<BuildHost> {
        self.host.clone()
    }

    // PORT: testing
    fn testing(&self) -> Option<Rc<dyn CommandLineTesting>> {
        self.opts.testing.clone()
    }

    fn content_mapper_host(&self) -> Option<Rc<dyn contentmapper::Host>> {
        self.content_mapper_host.clone()
    }
}

/// PORT: not in Go (perf). Go checks whether up to `numRoutines` projects
/// are up to date at the same time, each on its goroutine, and each reads
/// and unmarshals its build info file there (`loadOrStoreBuildInfo`). The
/// orchestrator here checks one task at a time, so threads read and parse
/// the build info files first (config_prefetch.rs `BuildInfoRead`), and a
/// task takes the parse of its file (`take`), waiting for it when a thread
/// has not finished it.
///
/// After the parse, the thread does the other parts of the check that need
/// no task state (`StatusPrefetch`): the root info reader, the paths of the
/// file names, and the mtimes of the task's TypeScript sources that are not
/// declaration files, the root files and the files of the build info. A
/// task of a build most often writes no such file, so the mtime is the one
/// that the check would read later; `StatusPrefetch::read_m_times` names
/// the exceptions and how the check reads those files again. A build info
/// file is read for one task only (`start_build_info_prefetch`), so no
/// other task writes it as its build info. When another task writes it as
/// an ordinary output, the check reads it again (`read_build_info_file`).
struct BuildInfoPrefetch {
    slots: FxHashMap<String, Arc<BuildInfoSlot>>,
}

/// The most released programs that `Orchestrator::released` keeps.
const MAX_KEPT_RELEASED: usize = 4;

/// PORT: not in Go (perf). Sends the build order index of its task when it
/// drops (see `build_all_tasks`).
struct ReadySignal {
    index: usize,
    ready: std::sync::mpsc::Sender<usize>,
}

impl Drop for ReadySignal {
    fn drop(&mut self) {
        let _ = self.ready.send(self.index);
    }
}

impl BuildInfoPrefetch {
    /// The build info of `name` that a thread read, and its check parts,
    /// once: `None` when it is not prefetched, was taken, or its read
    /// panicked; then the caller reads it. Waits for the thread that reads
    /// it.
    fn take(&mut self, name: &str) -> Option<BuildInfoResult> {
        self.slots.remove(name)?.take()
    }
}

// Go: build/orchestrator.go:983 NewOrchestrator
pub fn new_orchestrator(opts: Options) -> Orchestrator {
    // PORT: Go passes the method value `opts.Sys.FS().DirectoryExists`.
    let fs = opts.sys.fs();
    let use_case_sensitive_file_names = fs.use_case_sensitive_file_names();
    let wm = new_watch_manager(
        opts.sys.writer(),
        Box::new(move |path: &str| fs.directory_exists(path)),
        use_case_sensitive_file_names,
    );
    // Go: the `comparePathsOptions` field of the `Orchestrator` literal.
    let compare_paths_options = ComparePathsOptions {
        current_directory: opts.sys.get_current_directory(),
        use_case_sensitive_file_names: opts.sys.fs().use_case_sensitive_file_names(),
    };
    let host = Rc::new(BuildHost::new(
        opts.sys.clone(),
        opts.command.clone(),
        compare_paths_options.clone(),
    ));
    let mut orchestrator = Orchestrator {
        opts,
        compare_paths_options,
        host,
        content_mapper_host: None,
        tasks: FxHashMap::default(),
        order: Vec::new(),
        errors: Vec::new(),
        error_summary_reporter: None,
        watch_status_reporter: None,
        wm: Rc::new(RefCell::new(wm)),
        schedule_order: Vec::new(),
        graph_generated: false,
        build_info_prefetch: RefCell::new(None),
        prefetch_pool: RefCell::new(None),
        released: RefCell::default(),
        ends_process: std::cell::Cell::new(false),
        status_prefetch: RefCell::new(None),
    };
    // PORT: the wasm test is not in Go (see `Orchestrator::start_exported`).
    if !cfg!(target_family = "wasm") && orchestrator.opts.command.compiler_options.watch.is_true() {
        orchestrator.watch_status_reporter = Some(create_watch_status_reporter(
            orchestrator.opts.sys.clone(),
            &orchestrator.opts.command.locale(),
            orchestrator.opts.command.compiler_options.clone(),
            orchestrator.opts.testing.clone(),
        ));
        // Go: if t, ok := opts.Testing.(CommandLineTestingWithWatchBackend); ok { wm.SetBackend(t.WatchBackend()) }
        // PORT: the test backend comes from `watcher::set_test_watch_backend`.
        if let Some(backend) = crate::execute::watcher::test_watch_backend() {
            orchestrator.wm.borrow_mut().set_backend(backend);
        }
    } else {
        orchestrator.error_summary_reporter = Some(create_report_error_summary(
            &*orchestrator.opts.sys,
            &orchestrator.opts.command.locale(),
            Some(&orchestrator.opts.command.compiler_options),
        ));
    }
    orchestrator
}
