use crate::contentmapper;
use crate::emitter::program_emit::{EmitOptions, WriteFile, WriteFileData};
use crate::execute::build::command_line::ParsedBuildCommandLine;
use crate::execute::build::host::{BuildCompilerHost, BuildHost, WrittenPaths};
use crate::execute::build::up_to_date_status::*;
use crate::execute::incremental::build_info::{
    BuildInfoRootInfoReader, build_info_version, content_mapper_identities,
    is_build_info_file_name_default_library, resolve_build_info_file_name,
};
use crate::execute::incremental::emit_files::{
    buffer_early_emit_writes, flush_writes_on_this_thread, fs_error_text,
};
use crate::execute::incremental::incremental::Host as IncrementalHost;
use crate::execute::incremental::program::{
    NestedEmitNow, Program as IncrementalProgram, build_info_program,
    new_program as new_incremental_program,
};
use crate::execute::incremental::{BuildInfo, compute_hash};
use crate::execute::tsc::compile::{CompileTimes, System, Writer};
use crate::execute::tsc::diagnostics::{
    DiagnosticReporter, create_diagnostic_reporter, quiet_diagnostics_reporter,
};
use crate::execute::tsc::emit::{
    EmitInput, emit_and_report_statistics, get_trace_with_writer_from_sys,
};
use crate::execute::tsc::{ExitStatus, Statistics};
use crate::frontend::prelude::*;
use crate::gostd::GoError;
// PORT: testing
use crate::execute::tsc::CommandLineTesting;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

// This file ports execute/build/buildtask.go.
//
// PORT: concurrency. `ParsedCommandLine` and the frontend program are not
// `Send`, so tasks are `Rc<RefCell<BuildTask>>` and run on one thread, the
// orchestrator thread. It is the loading thread of every program of the
// build. Go `done` and `built` channels and the mutexes are dropped.
// `buildProject` is split where `compileAndEmit` has made the program
// (`build_project_start`) and the rest
// (`build_project_finish`). The orchestrator starts a task after its
// upstream tasks are done, finishes it later, and calls `report` in
// `order` (see orchestrator.rs). The work that Go does on the task
// goroutines still runs at the same time: each program checks on its own
// checker threads from `build_project_start` on (`start_check`), emits on
// them when the check ends (the writes wait for `build_project_finish`),
// and frees them in the background (`release_task_program`).
//
// PORT: the watch-only `updateWatch` and `resetConfig` are in
// orchestrator_watch.rs, with the orchestrator watch code.
//
// PORT: the task keeps the statistics of `tsc.EmitAndReportStatistics` for
// the build aggregate, as Go does. `opts.Testing` is
// `BuildTaskOrchestrator::testing` (`None` outside tests).
//
// PORT: Go `time.Time` is `Option<SystemTime>` (`None` = zero), as in
// up_to_date_status.rs.

// Go: build/buildtask.go:25 buildKind
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuildKind {
    #[default]
    None,
    Pseudo,
    Program,
}

// Go: build/buildtask.go:33 upstreamTask
// PORT: Go `int` index into `ProjectReferences()` is `usize`.
pub struct UpstreamTask {
    pub task: Rc<RefCell<BuildTask>>,
    pub ref_index: usize,
}

// Go: build/buildtask.go:37 buildInfoEntry
// PORT: Go `*time.Time` is `Option<Option<SystemTime>>` (nil pointer vs a
// pointer to a possibly zero time). Go `*incremental.BuildInfo` is
// `Option<Rc<BuildInfo>>`.
#[derive(Clone)]
pub struct BuildInfoEntry {
    pub build_info: Option<Rc<BuildInfo>>,
    /// ts#64159 (Go `fileName`, buildtask.go:39): the absolute build info
    /// file name. `path` is its path key.
    pub file_name: String,
    pub path: Path,
    pub m_time: Option<SystemTime>,
    pub dts_time: Option<Option<SystemTime>>,
    // PORT: not in Go (perf). What a prefetch thread made from
    // `build_info` for the up-to-date check (`StatusPrefetch`), until the
    // check takes it or the build ends (`BuildTask::drop_status_prefetch`).
    pub status_prefetch: Option<Arc<StatusPrefetch>>,
}

/// PORT: not in Go (perf). The parts of `getUpToDateStatus` that a
/// prefetch thread computes from the build info it read, ahead of the check
/// (see `BuildInfoPrefetch` in orchestrator.rs). Go computes them in the
/// check, on the task's builder goroutine. The check uses them only when
/// they were made for its build info directory and file lists.
pub struct StatusPrefetch {
    /// The build info directory that the other fields are for.
    pub build_info_directory: String,
    /// Go `getBuildInfoRootInfoReader()`.
    pub root_info_reader: BuildInfoRootInfoReader,
    /// `toPath` of each root file (`resolved.FileNames()`), in order.
    pub input_paths: Vec<Path>,
    /// `GetNormalizedAbsolutePath(buildInfoFileName, buildInfoDirectory)`
    /// and its `toPath`, for each build info file name, in order.
    pub file_names: Vec<(String, Path)>,
    /// The set of `input_paths`: the check's `seenRoots` after its loop
    /// over the root files.
    pub input_path_set: FxHashSet<Path>,
    /// The check's `resolvedRoots`: the resolved path of each root of the
    /// root info reader.
    pub resolved_roots: FxHashSet<Path>,
    /// Go `buildInfo.GetPackageJsons(buildInfoDirectory)` and
    /// `GetMissingPackageJsons`, collected. The check takes them.
    pub package_jsons: Vec<String>,
    pub missing_package_jsons: Vec<String>,
    /// The mtime (`incremental.GetMTime`) of each root file, by its index
    /// in `input_paths`, and of each file of `file_names`, by its index
    /// there, that the thread read (`read_m_times`); None for a file that
    /// it did not read. The check takes them where it would read the file
    /// system (`BuildTaskOrchestrator::get_m_time_of_path`).
    pub input_m_times: Vec<Option<Option<SystemTime>>>,
    pub file_m_times: Vec<Option<Option<SystemTime>>>,
}

/// PORT: not in Go (perf). The options of a task that decide whether its
/// up-to-date check returns before it reads the mtimes of its inputs. A
/// prefetch thread reads for the check only what the check reads
/// (`reads_input_times`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StatusCheckOptions {
    is_incremental: bool,
    emit_declarations: bool,
    no_check: bool,
    no_emit: bool,
    /// The Effect rules run (`BuildInfo::is_valid_version`).
    effect: bool,
}

impl StatusCheckOptions {
    pub fn new(options: &CompilerOptions) -> Self {
        StatusCheckOptions {
            is_incremental: options.is_incremental(),
            emit_declarations: options.get_emit_declarations(),
            no_check: options.no_check.is_true(),
            no_emit: options.no_emit.is_true(),
            effect: crate::effect::rulerunner::enabled_options(options).is_some(),
        }
    }

    /// False when `get_up_to_date_status` returns for `build_info` before
    /// it reads an input mtime: the version, error and pending emit checks
    /// that read only the build info and these options. True otherwise,
    /// also when the check may return early for another reason.
    pub fn reads_input_times(self, build_info: &BuildInfo) -> bool {
        if !build_info.is_valid_version(self.effect) {
            return false;
        }
        if build_info.errors
            || (!self.no_check && (build_info.semantic_errors || build_info.check_pending))
        {
            return false;
        }
        if self.is_incremental {
            if !build_info.is_incremental() {
                return false;
            }
            if (self.emit_declarations && build_info.emit_diagnostics_per_file.is_some())
                || (!self.no_check
                    && (build_info.change_file_set.is_some()
                        || build_info.semantic_diagnostics_per_file.is_some()))
            {
                return false;
            }
            if !self.no_emit
                && (build_info.change_file_set.is_some()
                    || build_info.affected_files_pending_emit.is_some())
            {
                return false;
            }
        }
        true
    }
}

impl StatusPrefetch {
    /// The check parts of `build_info`, read from `build_info_file_name`
    /// for a task with the root files `input_files`.
    pub fn new(
        build_info: &BuildInfo,
        build_info_file_name: &str,
        input_files: &[String],
        compare_paths_options: &ComparePathsOptions,
    ) -> Self {
        let to_path_fn = |file_name: &str| {
            to_path(
                file_name,
                &compare_paths_options.current_directory,
                compare_paths_options.use_case_sensitive_file_names,
            )
        };
        let build_info_directory = get_directory_path(&get_normalized_absolute_path(
            build_info_file_name,
            &compare_paths_options.current_directory,
        ));
        let root_info_reader = build_info
            .get_build_info_root_info_reader(&build_info_directory, compare_paths_options);
        let input_paths: Vec<Path> = input_files.iter().map(|file| to_path_fn(file)).collect();
        let input_path_set = input_paths.iter().cloned().collect();
        let resolved_roots = root_info_reader
            .roots()
            .filter_map(|root| {
                let (_, resolved) = root_info_reader.get_build_info_file_info(root);
                (!resolved.as_str().is_empty()).then_some(resolved)
            })
            .collect();
        let file_names = build_info
            .file_names
            .iter()
            .flatten()
            .map(|name| {
                let file = get_normalized_absolute_path(name, &build_info_directory);
                let path = to_path_fn(&file);
                (file, path)
            })
            .collect();
        let package_jsons = build_info
            .get_package_jsons(&build_info_directory)
            .collect();
        let missing_package_jsons = build_info
            .get_missing_package_jsons(&build_info_directory)
            .collect();
        StatusPrefetch {
            build_info_directory,
            root_info_reader,
            input_paths,
            file_names,
            input_path_set,
            resolved_roots,
            package_jsons,
            missing_package_jsons,
            input_m_times: Vec::new(),
            file_m_times: Vec::new(),
        }
    }

    /// Reads the mtimes (`input_m_times`, `file_m_times`) of the root files
    /// (`input_files`, the names of `input_paths`) and the files of the
    /// build info that are TypeScript sources (`is_typescript_source`), as
    /// `BuildHost::get_m_time` reads them (`incremental.GetMTime`) on `fs`.
    /// A build writes no TypeScript source as an output, so the mtime is
    /// most often the one that the check of the same build would read
    /// later. The exceptions, where a task writes the file (or its target)
    /// after this read and before the check, as Go reads it:
    /// - a build info file whose name has a TypeScript source extension
    ///   (`tsBuildInfoFile` `x.ts`). The check reads again each file that a
    ///   task of the build wrote (`BuildHost::get_m_time_of_path`).
    /// - a name that is a symbolic link to an output of another task. On
    ///   the OS file system a link is not read here
    ///   (`os_mod_times_of_non_links`), so the check reads it.
    /// - a link in a directory part of the name, to the output directory of
    ///   another task: not found, and the prefetched mtime stays.
    /// - a name that is a hard link to an output of another task (one file
    ///   with two names). The task writes the file in place under its own
    ///   name, and only that name is noted as written, so the prefetched
    ///   mtime of this name stays.
    /// The build drops what its checks did not take
    /// (`BuildTask::drop_status_prefetch`), so a later build reads the file
    /// system. Each path is read once: the build info lists the root files
    /// too. `os_fs`: `fs` is the wrapped OS file system (`is_wrapped_os_fs`),
    /// so the OS paths are read together.
    pub fn read_m_times(&mut self, fs: &dyn Fs, os_fs: bool, input_files: &[String]) {
        /// The index in `files` of the mtime of `file`, when it is read.
        fn index_of<'a>(
            files: &mut Vec<&'a str>,
            indexes: &mut FxHashMap<&'a Path, usize>,
            file: &'a str,
            path: &'a Path,
        ) -> Option<usize> {
            is_typescript_source(file).then(|| {
                *indexes.entry(path).or_insert_with(|| {
                    files.push(file);
                    files.len() - 1
                })
            })
        }
        let mut files = Vec::with_capacity(input_files.len());
        let mut indexes =
            FxHashMap::with_capacity_and_hasher(input_files.len(), Default::default());
        let input_indexes: Vec<Option<usize>> = input_files
            .iter()
            .zip(&self.input_paths)
            .map(|(file, path)| index_of(&mut files, &mut indexes, file, path))
            .collect();
        let file_indexes: Vec<Option<usize>> = self
            .file_names
            .iter()
            .map(|(file, path)| index_of(&mut files, &mut indexes, file, path))
            .collect();
        drop(indexes);
        let m_times: Vec<Option<Option<SystemTime>>> =
            if os_fs && files.iter().all(|file| file.starts_with('/')) {
                os_mod_times_of_non_links(files)
            } else {
                files
                    .iter()
                    .map(|file| Some(fs.stat(file).and_then(|stat| stat.mod_time())))
                    .collect()
            };
        let m_time_at = |index: &Option<usize>| index.and_then(|index| m_times[index]);
        self.input_m_times = input_indexes.iter().map(m_time_at).collect();
        self.file_m_times = file_indexes.iter().map(m_time_at).collect();
    }
}

/// True for a TypeScript file that is not a declaration file. A build
/// writes one only as a build info file with such a name (its outputs are
/// JavaScript, declaration, map, JSON and build info files; see
/// `StatusPrefetch::read_m_times`).
fn is_typescript_source(file_name: &str) -> bool {
    file_extension_is_one_of(
        file_name,
        &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_MTS, EXTENSION_CTS],
    ) && !is_declaration_file_name(file_name)
}

// Go: tsc/diagnostics.go:23 DiagnosticReporter, bound to the task's writer.
// PORT: Go reporters capture `&t.result.builder` as their `io.Writer`. A
// Rust closure cannot hold that borrow while the task also writes to the
// builder, so the reporter takes the writer as its first argument. The
// orchestrator wraps the tsc reporters (`CreateBuilderStatusReporter`,
// `CreateDiagnosticReporter`) into this shape.
pub type TaskDiagnosticReporter = Box<dyn Fn(&mut String, &Diagnostic)>;

// Go: build/buildtask.go:45 taskResult
// PORT: Go `program *incremental.Program` is `program` (`None` = nil). In
// tests the task keeps it until it reports, for `Testing.OnProgram`, and
// the orchestrator releases it there (`release_task_program`), where Go
// drops `t.result`. Outside tests the orchestrator releases it when the
// task is built, where Go sets it to nil (ts#64220).
// `has_changed_dts_file` is its `HasChangedDtsFile()` after the emit,
// which `updateDownstream` reads. Go `*tsc.Statistics` is an `Option`
// (nil = `None`).
pub struct TaskResult {
    pub builder: String,
    pub report_status: TaskDiagnosticReporter,
    pub diagnostic_reporter: TaskDiagnosticReporter,
    pub exit_status: ExitStatus,
    pub statistics: Option<Statistics>,
    pub program: Option<IncrementalProgram>,
    pub has_changed_dts_file: bool,
    pub build_kind: BuildKind,
    pub files_to_delete: Vec<String>,
}

impl TaskResult {
    // PORT: Go `&taskResult{}` plus the two reporters that
    // `buildOrCleanProject` (orchestrator.go:575) sets right after.
    pub fn new(
        report_status: TaskDiagnosticReporter,
        diagnostic_reporter: TaskDiagnosticReporter,
    ) -> Self {
        TaskResult {
            builder: String::new(),
            report_status,
            diagnostic_reporter,
            exit_status: ExitStatus::Success,
            statistics: None,
            program: None,
            has_changed_dts_file: false,
            build_kind: BuildKind::None,
            files_to_delete: Vec::new(),
        }
    }
}

/// Go drops the task's program when the task is built (outside tests) or
/// `t.result` after `report`.
/// This frees the checker pool and the frontend of the program. Its static
/// files stay published, and the task holds the freeable file versions
/// that `t.errors` point at (`BuildTask::held_file_versions`), so the
/// diagnostics in `t.errors` can still be written.
// PORT: Go frees the program in the background GC. The checker threads
// free their checkers while the build goes on
// (`program::release_program_in_background_later`). The frontend program
// frees when the result drops; the orchestrator drops it when its thread
// would wait anyway (`Orchestrator::keep_released`).
pub fn release_task_program(program: IncrementalProgram) -> crate::program::ReleasedProgram {
    let go_program = program.get_program();
    // PORT: perf. The snapshot's maps free on a thread (`drop_in_background`).
    if let Some(snapshot) = program.into_snapshot() {
        drop_in_background(snapshot);
    }
    crate::program::release_program_in_background_later(go_program)
}

/// PORT: not in Go (perf). An optional value that frees on the thread of
/// `drop_in_background` when it drops.
struct DropInBackground<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> std::ops::Deref for DropInBackground<T> {
    type Target = Option<T>;
    fn deref(&self) -> &Option<T> {
        &self.0
    }
}

impl<T: Send + 'static> std::ops::DerefMut for DropInBackground<T> {
    fn deref_mut(&mut self) -> &mut Option<T> {
        &mut self.0
    }
}

impl<T: Send + 'static> Drop for DropInBackground<T> {
    fn drop(&mut self) {
        if let Some(value) = self.0.take() {
            drop_in_background(value);
        }
    }
}

/// PORT: not in Go (perf). Frees `value` on a thread that frees the values
/// sent to it in order, as Go's GC frees memory beside the build. Frees it
/// here when that thread cannot start. A value still queued at exit is not
/// freed.
pub(crate) fn drop_in_background<T: Send + 'static>(value: T) {
    // wasm32-wasip1 has no threads, so the thread cannot start. Saying so
    // leaves the thread out of the wasm module.
    if cfg!(target_family = "wasm") {
        drop(value);
        return;
    }
    type Garbage = Box<dyn Send>;
    static QUEUE: std::sync::OnceLock<Option<Mutex<std::sync::mpsc::Sender<Garbage>>>> =
        std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Garbage>();
        std::thread::Builder::new()
            .name("goport-free".to_string())
            .spawn(move || {
                for garbage in receive {
                    drop(garbage);
                }
            })
            .ok()
            .map(|_| Mutex::new(send))
    });
    let value: Garbage = Box::new(value);
    let unsent = match queue {
        Some(send) => send
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(value)
            .err()
            .map(|err| err.0),
        None => Some(value),
    };
    drop(unsent);
}

// The parts of Go `*Orchestrator` (and its `host`) that a build task uses.
// The orchestrator (orchestrator.rs) implements this.
pub trait BuildTaskOrchestrator {
    // Go: `o.opts.Command`
    fn command(&self) -> &ParsedBuildCommandLine;
    // Go: `o.comparePathsOptions`
    fn compare_paths_options(&self) -> &ComparePathsOptions;
    // Go: orchestrator.go:94 (*Orchestrator).relativeFileName
    fn relative_file_name(&self, file_name: &str) -> String;
    // Go: orchestrator.go:97 (*Orchestrator).toPath (at 673a5f17d713; removed by ts#64159: Go N' calls
    // caseSensitivity.PathKey)
    fn to_path(&self, file_name: &str) -> Path;
    // Go: `o.opts.Sys.Now()`
    fn now(&self) -> SystemTime;
    // Go: `o.host.FS()`
    fn fs(&self) -> Rc<dyn Fs>;
    // Go: build/host.go (*host).GetMTime (cached)
    fn get_m_time(&self, file: &str) -> Option<SystemTime>;
    /// PORT: not in Go (perf). `get_m_time` of `file`, whose `toPath` is
    /// `path`.
    /// `prefetched`: the mtime that a prefetch thread read for `file`
    /// (`StatusPrefetch`), taken where the host would read the file system.
    fn get_m_time_of_path(
        &self,
        file: &str,
        path: &Path,
        prefetched: Option<Option<SystemTime>>,
    ) -> Option<SystemTime>;
    // Go: build/host.go (*host).SetMTime
    fn set_m_time(&self, file: &str, m_time: SystemTime) -> Result<(), FsError>;
    // Go: build/host.go (*host).storeMTime
    fn store_m_time(&self, file: &str, m_time: SystemTime);
    // Go: `incremental.NewBuildInfoReader(o.host).ReadBuildInfo(config)`
    // (uncached read from disk).
    fn read_build_info_file(&self, config: &ParsedCommandLine) -> Option<Rc<BuildInfo>>;

    /// PORT: not in Go (perf). What a prefetch thread made for the check
    /// from the build info that `read_build_info_file` just gave for
    /// `build_info_file_name` (`StatusPrefetch`).
    fn take_status_prefetch(&self, _build_info_file_name: &str) -> Option<StatusPrefetch> {
        None
    }
    // Go: `o.opts.Sys`
    fn sys(&self) -> Rc<dyn System>;
    // Go: `o.host`
    fn host(&self) -> Rc<BuildHost>;
    // Go: `o.opts.Testing`
    // PORT: testing. `None` outside tests.
    fn testing(&self) -> Option<Rc<dyn CommandLineTesting>> {
        None
    }
    // Go: `o.contentMapperHost` (tsgo#4712)
    fn content_mapper_host(&self) -> Option<Rc<dyn contentmapper::Host>>;
}

// Go: build/buildtask.go:56 BuildTask
// PORT: Go `*tsoptions.ParsedCommandLine` shared with the host cache is
// `Option<Rc<ParsedCommandLine>>`. Go `*upToDateStatus` is
// `Option<UpToDateStatus>`. `pending` is a plain bool (see top). Go
// `contentMapperProjectOnce` (a `sync.Once`) is a bool; Go nil project and
// error are `None`.
pub struct BuildTask {
    pub config: String,
    pub resolved: Option<Rc<ParsedCommandLine>>,
    pub up_stream: Vec<UpstreamTask>,
    pub down_stream: Vec<Rc<RefCell<BuildTask>>>, // Only set and used in watch mode
    pub status: Option<UpToDateStatus>,

    // task reporting
    pub result: Option<TaskResult>,

    pub build_info_entry: Option<BuildInfoEntry>,
    pub package_jsons: Vec<String>,

    pub errors: Vec<Diagnostic>,
    /// PORT: not in Go. The freeable file versions that `errors` point at
    /// (`ast::diagnostic_file_versions`). The task's program is released
    /// when the task is built, before the task and the build summary report
    /// `errors` (`tsc -b --watch`; watchfree1). It is set after the
    /// compile, and cleared with `errors` (`compile_and_emit_start`,
    /// `reset_status`).
    held_file_versions: Vec<std::sync::Arc<crate::ast::FileVersion>>,
    pub pending: bool,
    pub is_initial_cycle: bool,
    pub dirty: bool,

    content_mapper_project_once: bool,
    pub content_mapper_project: Option<Rc<dyn contentmapper::Project>>,
    pub content_mapper_project_err: Option<GoError>,

    // PORT: not in Go. The compile between `build_project_start` and
    // `build_project_finish`.
    compile: Option<PendingCompile>,

    // PORT: not in Go. A Go panic in this task's builder goroutine (see
    // `build_project_start`).
    go_panic: Option<TaskGoPanic>,
}

/// PORT: a Go panic in a builder goroutine of Go `rangeTasks`. In Go the
/// report goroutine still writes the reports of the tasks that are built
/// before the panic ends the process. The port reports on the thread that
/// runs the tasks, so `build_project_start` keeps the panic and `report`
/// raises it: the tasks before this one in `order` report first.
enum TaskGoPanic {
    /// The task panicked with this `go_panic` payload.
    Panicked(Box<dyn std::any::Any + Send>),
    /// An upstream task panicked. Go's builder of this task waits on it
    /// (`waitOnUpstream`) until the process ends, so the task does nothing.
    Upstream,
}

// The state of Go `compileAndEmit` from `NewProgram` to
// `EmitAndReportStatistics`: the program is made and not emitted yet.
struct PendingCompile {
    program: &'static GoProgram,
    incremental_program: IncrementalProgram,
    // Go `&t.result.builder` as the compile writer (see
    // `compile_and_emit_start`).
    builder: Rc<RefCell<Vec<u8>>>,
    writer: Writer,
    // Go `t.reportDiagnostic`, and what it appends to `t.errors`.
    report_diagnostic: DiagnosticReporter,
    errors: Rc<RefCell<Vec<Diagnostic>>>,
    compile_times: Rc<RefCell<CompileTimes>>,
    // Go `t.writeFile`, and the build info that it wrote.
    write_file: WriteFile,
    written_build_info: WrittenBuildInfo,
    // The writes that wait for the orchestrator thread (`DeferredWrites`).
    deferred_writes: Option<DeferredWrites>,
    // `incremental_program.start_emit` ran (see `compile_and_emit_start`).
    emit_started: bool,
}

impl BuildTask {
    // PORT: Go `&BuildTask{config: config, isInitialCycle: ...}` followed by
    // `task.pending.Store(true)` in createBuildTasks (orchestrator.go:139).
    pub fn new(config: String, is_initial_cycle: bool) -> Self {
        BuildTask {
            config,
            resolved: None,
            up_stream: Vec::new(),
            down_stream: Vec::new(),
            status: None,
            result: None,
            build_info_entry: None,
            package_jsons: Vec::new(),
            errors: Vec::new(),
            held_file_versions: Vec::new(),
            pending: true,
            is_initial_cycle,
            dirty: false,
            content_mapper_project_once: false,
            content_mapper_project: None,
            content_mapper_project_err: None,
            compile: None,
            go_panic: None,
        }
    }

    // Go: build/buildtask.go:84 (*BuildTask).getContentMapperProject (tsgo#4712)
    pub(crate) fn get_content_mapper_project(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
    ) -> (Option<Rc<dyn contentmapper::Project>>, Option<GoError>) {
        if !self.content_mapper_project_once {
            self.content_mapper_project_once = true;
            if let (Some(host), Some(resolved)) =
                (orchestrator.content_mapper_host(), self.resolved.as_ref())
                && !resolved.content_mappers().is_empty()
            {
                self.content_mapper_project = host.project(contentmapper::ProjectSpec {
                    config_file_name: resolved.config_name().to_string(),
                    mappers: resolved.content_mappers().to_vec(),
                    compiler_options: Some(resolved.compiler_options().clone()),
                });
            }
        }
        (
            self.content_mapper_project.clone(),
            self.content_mapper_project_err.clone(),
        )
    }

    // Go: build/buildtask.go:98 (*BuildTask).refreshContentMapperProject (tsgo#4712)
    pub(crate) fn refresh_content_mapper_project(&mut self) {
        if let Some(project) = &self.content_mapper_project {
            self.content_mapper_project_err = project.refresh().err();
        }
    }

    fn result_mut(&mut self) -> &mut TaskResult {
        self.result.as_mut().expect("task result is set")
    }

    fn resolved(&self) -> &Rc<ParsedCommandLine> {
        self.resolved.as_ref().expect("resolved config")
    }

    fn status(&self) -> &UpToDateStatus {
        self.status.as_ref().expect("status is set")
    }

    // Go: `t.result.reportStatus(diagnostic)`
    fn report_status(&mut self, diagnostic: Diagnostic) {
        let result = self.result_mut();
        (result.report_status)(&mut result.builder, &diagnostic);
    }

    // Go: build/buildtask.go:104 (*BuildTask).waitOnUpstream
    // PORT: no-op. The orchestrator starts a task only when its upstream
    // tasks are done (see top).
    pub fn wait_on_upstream(&self) {}

    // Go: build/buildtask.go:110 (*BuildTask).unblockDownstream
    pub fn unblock_downstream(&mut self) {
        self.pending = false;
        self.is_initial_cycle = false;
    }

    // Go: build/buildtask.go:116 (*BuildTask).reportDiagnostic
    pub fn report_diagnostic(&mut self, err: Diagnostic) {
        self.errors.push(err.clone());
        let result = self.result_mut();
        (result.diagnostic_reporter)(&mut result.builder, &err);
    }

    // Go: build/buildtask.go:121 (*BuildTask).report
    // PORT: the orchestrator calls it in `order` when the task is built
    // (ts#64220: Go no longer waits for the previous task here). Go writes
    // the buffered output to `Sys.Writer()` and merges into the
    // orchestrator's `OrchestratorResult` (ts#64158). That type belongs to the
    // orchestrator, so this takes the task result and errors and returns
    // them; the orchestrator must, in build order:
    //   - append `errors` to `buildResult.Errors` when not empty,
    //   - write `result.builder` to the writer,
    //   - raise `buildResult.Result.Status` to `result.exit_status` if higher,
    //   - aggregate `result.statistics` into `buildResult.Statistics` when set,
    //   - count `result.build_kind` (ProjectsBuilt / TimestampUpdates),
    //   - append `result.files_to_delete` to `buildResult.FilesToDelete`.
    // PORT: a Go panic that `build_project_start` kept ends the run here
    // (see `TaskGoPanic`).
    pub fn report(&mut self) -> (TaskResult, Vec<Diagnostic>) {
        if let Some(TaskGoPanic::Panicked(payload)) = self.go_panic.take() {
            std::panic::resume_unwind(payload);
        }
        let result = self.result.take().expect("task result is set");
        (result, self.errors.clone())
    }

    // Go: build/buildtask.go:147 (*BuildTask).buildProject, up to the
    // program that `compileAndEmit` makes (`compile_and_emit_start`).
    // PORT: Go runs up to `numRoutines` tasks at the same time, and a task
    // that runs beside others makes its program before they write their
    // outputs. The orchestrator keeps that order on one thread (see
    // orchestrator.rs), so `buildProject` is split here. It returns true
    // when the task compiles: then the caller must call
    // `build_project_finish`. When it returns false the task is done
    // (downstream unblocked).
    // PORT: a `go_panic` in the task is kept for `report` (see
    // `TaskGoPanic`). Other panics are port gaps and continue.
    pub fn build_project_start(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        path: &Path,
    ) -> bool {
        if self
            .up_stream
            .iter()
            .any(|upstream| upstream.task.borrow().go_panic.is_some())
        {
            self.go_panic = Some(TaskGoPanic::Upstream);
            return false;
        }
        let payload = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.build_project_start_task(orchestrator, path)
        })) {
            Ok(compiles) => return compiles,
            Err(payload) => payload,
        };
        self.keep_go_panic(orchestrator, payload);
        false
    }

    /// Keeps a `go_panic` of this task's builder goroutine for `report`
    /// (see `TaskGoPanic`). Any other panic is a port gap and continues.
    fn keep_go_panic(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        mut payload: Box<dyn std::any::Any + Send>,
    ) {
        let Some(panic) = payload.downcast_mut::<crate::core::GoPanic>() else {
            std::panic::resume_unwind(payload);
        };
        // Go: orchestrator.go:944 `wg.Queue(runTask)` unless one builder
        // runs (`numRoutines`, see `Orchestrator::num_routines`). The
        // goroutine of `sync.WaitGroup.Go` panics again (see
        // `go_wait_group_task`).
        let command = orchestrator.command();
        let num_routines = if command.compiler_options.single_threaded.is_true() {
            1
        } else {
            command.build_options.builders.unwrap_or(4)
        };
        panic.repanicked |= num_routines != 1;
        self.go_panic = Some(TaskGoPanic::Panicked(payload));
    }

    fn build_project_start_task(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        path: &Path,
    ) -> bool {
        // Wait on upstream tasks to complete
        self.wait_on_upstream();
        if self.pending {
            self.status = Some(self.get_up_to_date_status(orchestrator, path));
            self.report_up_to_date_status(orchestrator);
            if !self.handle_status_that_doesnt_require_build(orchestrator) {
                if self.compile_and_emit_start(orchestrator, path) {
                    return true;
                }
                // Go `compileAndEmit` returned before the program (the
                // content mapper project failed); then `updateDownstream`.
                self.update_downstream(orchestrator, path);
            } else {
                if let Some(resolved) = self.resolved.clone() {
                    for diagnostic in resolved.get_config_file_parsing_diagnostics() {
                        self.report_diagnostic(diagnostic);
                    }
                }
                if !self.errors.is_empty() {
                    self.result_mut().exit_status = ExitStatus::DiagnosticsPresentOutputsSkipped;
                }
            }
        } else if !self.errors.is_empty() {
            self.report_up_to_date_status(orchestrator);
            for err in self.errors.clone() {
                // Should not add the diagnostics so just reporting
                let result = self.result_mut();
                (result.diagnostic_reporter)(&mut result.builder, &err);
            }
        }
        self.unblock_downstream();
        false
    }

    /// PORT: not in Go (perf). After `build_project_start` returned true:
    /// sends values that `signal` makes behind the check and emit jobs that
    /// the task's program started (`program::send_checker_barrier`), and
    /// returns how many there are. When all have dropped, those jobs are
    /// done. 0 when the program has no checker pool, so no job runs (no
    /// check and no early emit started).
    pub fn notify_when_compiled<T: Send + 'static>(&self, signal: impl Fn() -> T) -> usize {
        let compile = self.compile.as_ref().expect("compile_and_emit_start ran");
        let _scope = crate::core::enter_program(Some(compile.program));
        crate::program::send_checker_barrier(signal)
    }

    // Go: build/buildtask.go:147 (*BuildTask).buildProject, from the emit
    // of `compileAndEmit` on (see `build_project_start`).
    // PORT: a `go_panic` here (a bad diagnostic of a `.tsbuildinfo` panics
    // when it is reported) is kept for `report` too.
    pub fn build_project_finish(&mut self, orchestrator: &dyn BuildTaskOrchestrator, path: &Path) {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.compile_and_emit_finish(orchestrator);
            self.update_downstream(orchestrator, path);
            self.unblock_downstream();
        })) {
            self.keep_go_panic(orchestrator, payload);
        }
    }

    // Go: build/buildtask.go:178 (*BuildTask).updateDownstream
    pub fn update_downstream(&mut self, orchestrator: &dyn BuildTaskOrchestrator, path: &Path) {
        if self.is_initial_cycle {
            return;
        }
        if orchestrator
            .command()
            .build_options
            .stop_build_on_errors
            .is_true()
            && self.status().is_error()
        {
            return;
        }

        if self
            .result
            .as_ref()
            .expect("task result is set")
            .program
            .is_none()
        {
            for down_stream in &self.down_stream {
                let mut down_stream = down_stream.borrow_mut();
                down_stream.reset_status();
                down_stream.pending = true;
            }
            return;
        }

        let has_changed_dts_file = self.result_mut().has_changed_dts_file;
        for down_stream in &self.down_stream {
            let mut down_stream = down_stream.borrow_mut();
            if let Some(status) = down_stream.status.clone() {
                // PORT: the Go `fallthrough` from UpToDate into the
                // pseudo-build case is written out.
                match status.kind {
                    UpToDateStatusType::UpToDate
                    | UpToDateStatusType::UpToDateWithUpstreamTypes
                    | UpToDateStatusType::UpToDateWithInputFileText => {
                        if status.kind == UpToDateStatusType::UpToDate && !has_changed_dts_file {
                            down_stream.status = Some(UpToDateStatus::with_data(
                                UpToDateStatusType::UpToDateWithUpstreamTypes,
                                status.data.clone(),
                            ));
                        } else if has_changed_dts_file {
                            down_stream.status = Some(UpToDateStatus::with_data(
                                UpToDateStatusType::InputFileNewer,
                                UpToDateStatusData::InputOutputName(InputOutputName {
                                    input: self.config.clone(),
                                    output: status.oldest_output_file_name(),
                                }),
                            ));
                        }
                    }
                    UpToDateStatusType::UpstreamErrors => {
                        let upstream_errors = status.upstream_errors();
                        let ref_config =
                            resolve_config_file_name_of_project_reference(&upstream_errors.ref_);
                        if orchestrator.to_path(&ref_config) == *path {
                            down_stream.reset_status();
                        }
                    }
                    _ => {}
                }
            }
            down_stream.pending = true;
        }
    }

    // Go: build/buildtask.go:223 (*BuildTask).compileAndEmit, up to
    // `incremental.NewProgram` (see `build_project_start`). It returns
    // false when Go returns before the program (the content mapper project
    // failed): then there is nothing for `compile_and_emit_finish`.
    // PORT: the program is a program version of this multi-program process
    // (`program::new_program_version`), made on this thread, the loading
    // thread of every program of the build. It is current
    // (`core::enter_program`) while it is used, because the `program.rs`
    // functions read `prog()`.
    // PORT: Go `EmitInput.Writer`, the trace writer and
    // `t.result.diagnosticReporter` write to `&t.result.builder`. The task
    // reporters take the builder per call (`TaskDiagnosticReporter`) and
    // the tsc code takes a `Writer`, so the compile writes to its own
    // buffer, which `compile_and_emit_finish` appends to the builder.
    // Nothing else writes to the builder meanwhile, so the order is Go's.
    pub fn compile_and_emit_start(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        path: &Path,
    ) -> bool {
        self.errors = Vec::new();
        self.held_file_versions = Vec::new();
        let command = orchestrator.command();
        if command.build_options.verbose.is_true() {
            self.report_status(new_compiler_diagnostic(
                diag::Building_project_0,
                args![orchestrator.relative_file_name(&self.config)],
            ));
        }

        let sys = orchestrator.sys();
        let host = orchestrator.host();
        let testing = orchestrator.testing();
        let resolved = self.resolved().clone();
        let builder: Rc<RefCell<Vec<u8>>> = Rc::default();
        let writer: Writer = builder.clone();
        // Go: build/buildtask.go:114 (*BuildTask).reportDiagnostic. The
        // diagnostics go to `t.errors` in `compile_and_emit_finish`.
        let errors: Rc<RefCell<Vec<Diagnostic>>> = Rc::default();
        let report_diagnostic: DiagnosticReporter = {
            let errors = errors.clone();
            // Go `t.result.diagnosticReporter` (orchestrator.go:597
            // createDiagnosticReporter(task)).
            let report = create_diagnostic_reporter(
                &*sys,
                writer.clone(),
                &command.locale(),
                &command.compiler_options,
            );
            Rc::new(move |err: &Diagnostic| {
                errors.borrow_mut().push(err.clone());
                report(err);
            })
        };

        // Real build
        let compile_times = Rc::new(RefCell::new(CompileTimes::default()));
        let config_time = host
            .config_times
            .borrow()
            .get(path)
            .copied()
            .unwrap_or_default();
        compile_times.borrow_mut().config_time = config_time;
        // PORT: perf, as `tsc -p` does: the parse of the default lib files
        // starts before the build info read and the program load. Only for
        // a program that loads while the build host caches no parse (the
        // first one of a build cycle): later programs take the lib files
        // from that cache, and the workers would parse them for nothing.
        // Not when the load gets no parse workers (`BuildHost::prefetch`):
        // the workers' parses would stay unused in their AST arenas. And
        // not in a watch cycle after the first build: a lib parse there is a
        // freeable file version, which these early workers cannot make
        // (`FilesParser::parse` drops them), and a cycle with a config
        // change keeps the lib parses anyway.
        let mut host_has_parses = host
            .watch_sources
            .borrow()
            .as_ref()
            .is_some_and(|sources| !sources.is_empty());
        host.source_files
            .for_each_stored(|_, _| host_has_parses = true);
        if host.prefetch.get() && !host_has_parses && crate::ast::published_paths().is_empty() {
            crate::execute::execute_tsc::start_lib_prefetch(&*sys, &resolved, testing.is_some());
        }
        let build_info_read_start = sys.now();
        let mut old_program = None;
        let (content_mapper_project, err) = self.get_content_mapper_project(orchestrator);
        if let Some(err) = err {
            // Go `t.reportDiagnostic` writes to `&t.result.builder`.
            self.report_diagnostic(content_mapper_project_diagnostic(&err));
            self.status = Some(UpToDateStatus::new(UpToDateStatusType::BuildErrors));
            self.result_mut().exit_status = ExitStatus::DiagnosticsPresentOutputsSkipped;
            return false;
        }
        let compiler_host = Rc::new(BuildCompilerHost {
            host: host.clone(),
            trace: get_trace_with_writer_from_sys(
                writer.clone(),
                command.locale(),
                testing.clone(),
            ),
            content_mapper_project,
        });
        if !command.build_options.force.is_true() {
            // Go: `ReadBuildInfoProgram(t.resolved, o.host, compilerHost)`.
            // Its `o.host.ReadBuildInfo(t.resolved)` (build/host.go:77) is
            // this task's `loadOrStoreBuildInfo`, and the rest is
            // `build_info_program`, which reads that build info in place.
            let config_path = orchestrator.to_path(resolved.config_name());
            let (build_info, _) = self.load_or_store_build_info(
                orchestrator,
                &config_path,
                &resolved.get_build_info_file_name(),
            );
            old_program = build_info
                .and_then(|build_info| build_info_program(&resolved, &build_info, &*compiler_host));
        }
        compile_times.borrow_mut().build_info_read_time = elapsed(&*sys, build_info_read_start);
        let parse_start = sys.now();
        // Go: compiler.NewProgram(compiler.ProgramOptions{Config, Host})
        // PORT: in `tsc -b --watch` the new parses of files that an earlier
        // build published are freeable file versions (watchfree1,
        // `ast::set_watch_process`).
        let np = crate::execute::execute_tsc::new_frontend_program(compiler_host, resolved);
        crate::program::mark_freeable_parses(&np);
        let program = crate::program::new_program_version(&np, None);
        drop(np);
        compile_times.borrow_mut().parse_time = elapsed(&*sys, parse_start);
        let written_build_info: WrittenBuildInfo = Arc::default();
        let deferred_writes = (!sys.emit_writes_through_osvfs()).then(DeferredWrites::default);
        let write_file = new_task_write_file(
            written_build_info.clone(),
            deferred_writes.clone(),
            self.store_output_time_stamp(orchestrator),
            host.m_times.clone(),
            host.written.clone(),
            orchestrator.compare_paths_options().clone(),
        );
        let emit_started = testing.is_none();
        let changes_compute_start = sys.now();
        let incremental_program = {
            let _scope = crate::core::enter_program(Some(program));
            // Go: orchestrator.opts.Sys.Now
            let nested_emit_now: NestedEmitNow = {
                let sys = sys.clone();
                Rc::new(move || sys.now())
            };
            let incremental_program = new_incremental_program(
                old_program.as_ref(),
                host as Rc<dyn IncrementalHost>,
                Some(nested_emit_now),
                testing.is_some(),
            );
            compile_times.borrow_mut().changes_compute_time = elapsed(&*sys, changes_compute_start);
            // PORT: Go checks and emits this project on its goroutine
            // while other tasks make their programs and emit. Here the
            // check starts on this program's checker threads now, and so
            // does the emit, behind the check, as in `tsc -p` (when the
            // rules of `Program::start_emit` allow it; else the emit runs
            // in `compile_and_emit_finish`). A task that checks nothing
            // (cached semantic diagnostics, `noCheck`, or syntactic, program
            // or global diagnostics) starts its emit there too, so it ends
            // when its emit ends, as its Go goroutine does. The emit keeps
            // its writes
            // until `compile_and_emit_finish`, which writes them first
            // (`buffer_early_emit_writes`). So the task writes when the
            // orchestrator finishes it (in the order the checks and emits
            // end, or in build order when tasks share outputs, see
            // `build_all_tasks`), and a task that runs beside others reads
            // the file system before they write. The statistics' check time
            // is the time of the wait for the check plus the time that
            // `start_check` spent on the affected files
            // (`Program::take_started_check_time`).
            // PORT: testing. A test finishes the task at once, and its emit
            // starts there, as without the early start.
            incremental_program.start_check();
            if emit_started {
                buffer_early_emit_writes(|| {
                    incremental_program.start_emit(EmitOptions {
                        write_file: Some(write_file.clone()),
                        ..EmitOptions::default()
                    });
                });
            }
            incremental_program
        };
        self.compile = Some(PendingCompile {
            program,
            incremental_program,
            builder,
            writer,
            report_diagnostic,
            errors,
            compile_times,
            write_file,
            written_build_info,
            deferred_writes,
            emit_started,
        });
        true
    }

    // Go: build/buildtask.go:223 (*BuildTask).compileAndEmit, from
    // `EmitAndReportStatistics` on (see `compile_and_emit_start`).
    pub fn compile_and_emit_finish(&mut self, orchestrator: &dyn BuildTaskOrchestrator) {
        let PendingCompile {
            program,
            incremental_program,
            builder,
            writer,
            report_diagnostic,
            errors,
            compile_times,
            write_file,
            written_build_info,
            deferred_writes,
            emit_started,
        } = self.compile.take().expect("compile_and_emit_start ran");
        let sys = orchestrator.sys();
        let host = orchestrator.host();
        let resolved = self.resolved().clone();
        let (mut result, statistics) = {
            let _scope = crate::core::enter_program(Some(program));
            WRITE_FILE_SYS.with(|write_file_sys| *write_file_sys.borrow_mut() = Some(sys.clone()));
            // PORT: perf. Each checker emits when its own check ends, as in
            // `tsc -p` (`Program::start_check_and_emit`), with the options of
            // the emit call in `EmitFilesAndReportErrors` (the same
            // `WriteFile`). `EmitAndReportStatistics` makes the same calls
            // as in Go and waits for that work. Outside tests the emit
            // started with the program (`compile_and_emit_start`).
            if !emit_started {
                incremental_program.start_emit(EmitOptions {
                    write_file: Some(write_file.clone()),
                    ..EmitOptions::default()
                });
            }
            let emit = || {
                emit_and_report_statistics(&EmitInput {
                    sys: &*sys,
                    program_like: &incremental_program,
                    config: Some(&resolved),
                    report_diagnostic,
                    report_error_summary: quiet_diagnostics_reporter(),
                    writer,
                    write_file: Some(write_file),
                    compile_times,
                    testing: orchestrator.testing(),
                    testing_m_times_cache: Some(&*host.m_times),
                })
            };
            // PORT: when only this thread reaches the file system
            // (`DeferredWrites`), the emit keeps its writes, early or not,
            // and this thread makes them (`flush_writes_on_this_thread`), so
            // a failed write is in its file's result, as in Go.
            let emitted = if deferred_writes.is_some() {
                flush_writes_on_this_thread(emit)
            } else {
                emit()
            };
            if let Some(deferred_writes) = &deferred_writes {
                write_deferred(deferred_writes, &*sys.fs());
            }
            WRITE_FILE_SYS.with(|write_file_sys| *write_file_sys.borrow_mut() = None);
            emitted
        };
        assert!(
            incremental_program.start_check_used(),
            "{}: the emit did not use the check that started with the program",
            self.config
        );
        let has_changed_dts_file = incremental_program.has_changed_dts_file();
        // Go appends to `t.errors` while `EmitAndReportStatistics` reports.
        self.errors.extend(errors.take());
        self.held_file_versions = crate::ast::diagnostic_file_versions(&self.errors);
        {
            let task_result = self.result_mut();
            task_result
                .builder
                .push_str(&String::from_utf8_lossy(&builder.borrow()));
            task_result.has_changed_dts_file = has_changed_dts_file;
            task_result.program = Some(incremental_program);
        }
        // PORT: see `DeferredWrites`. Go's emitter reports a failed write as
        // TS5033; here a write that waited fails after the emit. The emit
        // makes its writes on this thread, so none is known to wait.
        let failed = deferred_writes.map_or_else(Vec::new, |deferred| {
            std::mem::take(
                &mut deferred
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .failed,
            )
        });
        for (file_name, err) in failed {
            self.report_diagnostic(new_compiler_diagnostic(
                diag::Could_not_write_file_0_Colon_1,
                args![file_name, err],
            ));
            if result.status == ExitStatus::Success {
                result.status = ExitStatus::DiagnosticsPresentOutputsGenerated;
            }
        }
        // Go: build/buildtask.go:906 (*BuildTask).writeFile, the build info
        // part (`onBuildInfoEmit`), with the time of the write.
        // PORT: it runs when the emit is done (see `new_task_write_file`).
        // The build info is the last file that the emit writes, so
        // `HasChangedDtsFile()` has its final value in Go too.
        let written = written_build_info
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((build_info_file_name, build_info, m_time)) = written {
            let build_info = Arc::try_unwrap(build_info).unwrap_or_else(|shared| (*shared).clone());
            self.on_build_info_emit(
                orchestrator,
                &build_info_file_name,
                Some(Rc::new(build_info)),
                has_changed_dts_file,
                m_time,
            );
        }

        self.result_mut().exit_status = result.status;
        self.result_mut().statistics = statistics;
        // Go: build/buildtask.go:236 `t.packageJsons = t.result.program.PackageJsonLookupPaths()`.
        // It reads `prog()`, so the task's program is current.
        self.package_jsons = {
            let _scope = crate::core::enter_program(Some(program));
            self.result
                .as_ref()
                .and_then(|result| result.program.as_ref())
                .expect("compile_and_emit_finish set the program")
                .package_json_lookup_paths()
        };
        let emitted_files = &result.emit_result.emitted_files;
        if (!resolved.compiler_options().no_emit_on_error.is_true()
            || result.diagnostics.is_empty())
            && (!emitted_files.is_empty()
                || self.status().kind != UpToDateStatusType::OutOfDateBuildInfoWithErrors)
        {
            // Update time stamps for rest of the outputs
            self.update_time_stamps(
                orchestrator,
                emitted_files,
                diag::Updating_unchanged_output_timestamps_of_project_0,
            );
        }
        self.result_mut().build_kind = BuildKind::Program;
        if result.status == ExitStatus::DiagnosticsPresentOutputsSkipped
            || result.status == ExitStatus::DiagnosticsPresentOutputsGenerated
        {
            self.status = Some(UpToDateStatus::new(UpToDateStatusType::BuildErrors));
        } else {
            let oldest_output_file_name = match emitted_files.first() {
                Some(first) => first.clone(),
                None => resolved.get_output_file_names().next().unwrap_or_default(),
            };
            self.status = Some(UpToDateStatus::with_data(
                UpToDateStatusType::UpToDate,
                UpToDateStatusData::String(oldest_output_file_name),
            ));
        }
    }

    // Go: build/buildtask.go:298 (*BuildTask).handleStatusThatDoesntRequireBuild
    pub fn handle_status_that_doesnt_require_build(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
    ) -> bool {
        let build_options = &orchestrator.command().build_options;
        match self.status().kind {
            UpToDateStatusType::UpToDate => {
                if build_options.dry.is_true() {
                    let config = self.config.clone();
                    self.report_status(new_compiler_diagnostic(
                        diag::Project_0_is_up_to_date,
                        args![config],
                    ));
                }
                return true;
            }
            UpToDateStatusType::UpstreamErrors => {
                let upstream_status = self.status().upstream_errors().clone();
                if build_options.verbose.is_true() {
                    let message = if upstream_status.ref_has_upstream_errors {
                        diag::Skipping_build_of_project_0_because_its_dependency_1_was_not_built
                    } else {
                        diag::Skipping_build_of_project_0_because_its_dependency_1_has_errors
                    };
                    self.report_status(new_compiler_diagnostic(
                        message,
                        args![
                            orchestrator.relative_file_name(&self.config),
                            orchestrator.relative_file_name(&upstream_status.ref_)
                        ],
                    ));
                }
                return true;
            }
            UpToDateStatusType::Solution => return true,
            UpToDateStatusType::ConfigFileNotFound => {
                let config = self.config.clone();
                self.report_diagnostic(new_compiler_diagnostic(
                    diag::File_0_not_found,
                    args![config],
                ));
                return true;
            }
            _ => {}
        }

        // update timestamps
        if self.status().is_pseudo_build() {
            if build_options.dry.is_true() {
                let config = self.config.clone();
                self.report_status(new_compiler_diagnostic(
                    diag::A_non_dry_build_would_update_timestamps_for_output_of_project_0,
                    args![config],
                ));
                self.status = Some(UpToDateStatus::new(UpToDateStatusType::UpToDate));
                return true;
            }

            self.update_time_stamps(
                orchestrator,
                &[],
                diag::Updating_output_timestamps_of_project_0,
            );
            let data = self.status().data.clone();
            self.status = Some(UpToDateStatus::with_data(
                UpToDateStatusType::UpToDate,
                data,
            ));
            self.result_mut().build_kind = BuildKind::Pseudo;
            return true;
        }

        if build_options.dry.is_true() {
            let config = self.config.clone();
            self.report_status(new_compiler_diagnostic(
                diag::A_non_dry_build_would_build_project_0,
                args![config],
            ));
            self.status = Some(UpToDateStatus::new(UpToDateStatusType::UpToDate));
            return true;
        }
        false
    }

    // Go: build/buildtask.go:348 (*BuildTask).getUpToDateStatus
    pub fn get_up_to_date_status(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        config_path: &Path,
    ) -> UpToDateStatus {
        if let Some(status) = &self.status {
            return status.clone();
        }
        // Config file not found
        let Some(resolved) = self.resolved.clone() else {
            return UpToDateStatus::new(UpToDateStatusType::ConfigFileNotFound);
        };

        // Solution - nothing to build
        if resolved.file_names().is_empty() && resolved.has_project_references() {
            return UpToDateStatus::new(UpToDateStatusType::Solution);
        }

        let build_options = &orchestrator.command().build_options;
        for upstream in &self.up_stream {
            let upstream_task = upstream.task.borrow();
            let upstream_status = upstream_task.status();
            if build_options.stop_build_on_errors.is_true() && upstream_status.is_error() {
                // Upstream project has errors, so we cannot build this project
                return UpToDateStatus::with_data(
                    UpToDateStatusType::UpstreamErrors,
                    UpToDateStatusData::UpstreamErrors(UpstreamErrors {
                        ref_: resolved.project_references()[upstream.ref_index]
                            .path
                            .clone(),
                        ref_has_upstream_errors: upstream_status.kind
                            == UpToDateStatusType::UpstreamErrors,
                    }),
                );
            }
        }

        if build_options.force.is_true() {
            return UpToDateStatus::new(UpToDateStatusType::ForceBuild);
        }

        // Check the build info
        let build_info_path = resolved.get_build_info_file_name();
        let (build_info, build_info_time) =
            self.load_or_store_build_info(orchestrator, config_path, &build_info_path);
        let Some(build_info) = build_info else {
            return UpToDateStatus::with_data(
                UpToDateStatusType::OutputMissing,
                UpToDateStatusData::String(build_info_path),
            );
        };

        // build info version
        if !build_info.is_valid_version(
            crate::effect::rulerunner::enabled_options(resolved.compiler_options()).is_some(),
        ) {
            return UpToDateStatus::with_data(
                UpToDateStatusType::TsVersionOutputOfDate,
                UpToDateStatusData::String(build_info.version.clone()),
            );
        }

        // If a configured content mapper's identity has changed, files it produced may be stale.
        // (tsgo#4712)
        let (content_mapper_project, err) = self.get_content_mapper_project(orchestrator);
        let (content_mapper_identities, identity_err) =
            match content_mapper_identities(content_mapper_project.as_deref()) {
                Ok(identities) => (identities, None),
                Err(identity_err) => (None, Some(identity_err)),
            };
        if let Some(identity_err) = &identity_err {
            self.content_mapper_project_err = Some(identity_err.clone());
        }
        if err.is_some()
            || identity_err.is_some()
            || !build_info.content_mapper_identities_match(content_mapper_identities.as_deref())
        {
            return UpToDateStatus::with_data(
                UpToDateStatusType::OutOfDateOptions,
                UpToDateStatusData::String(build_info_path),
            );
        }

        let options = resolved.compiler_options();
        // Report errors if build info indicates errors
        if build_info.errors || // Errors that need to be reported irrespective of "--noCheck"
            (!options.no_check.is_true() && (build_info.semantic_errors || build_info.check_pending))
        {
            // Errors without --noCheck
            return UpToDateStatus::with_data(
                UpToDateStatusType::OutOfDateBuildInfoWithErrors,
                UpToDateStatusData::String(build_info_path),
            );
        }

        let build_info_directory = get_directory_path(&get_normalized_absolute_path(
            &build_info_path,
            &orchestrator.compare_paths_options().current_directory,
        ));
        // PORT: perf. The parts that a prefetch thread computed, when
        // they are for this directory and these file lists. They free on a
        // thread when the check returns (`DropInBackground`).
        let mut prefetched = DropInBackground(
            self.build_info_entry
                .as_mut()
                .and_then(|entry| entry.status_prefetch.take()),
        );
        *prefetched = prefetched.take().filter(|prefetched| {
            prefetched.build_info_directory == build_info_directory
                && prefetched.input_paths.len() == resolved.file_names().len()
                && prefetched.file_names.len() == build_info.file_names.as_ref().map_or(0, Vec::len)
        });
        if options.is_incremental() {
            if !build_info.is_incremental() {
                // Program options out of date
                return UpToDateStatus::with_data(
                    UpToDateStatusType::OutOfDateOptions,
                    UpToDateStatusData::String(build_info_path),
                );
            }

            // Errors need to be reported if build info has errors
            // PORT: Go `!= nil` on these slices is `is_some()`. Go json
            // leaves them nil when the key is absent (`omitzero`).
            if (options.get_emit_declarations() && build_info.emit_diagnostics_per_file.is_some()) || // Always reported errors
                (!options.no_check.is_true() && // Semantic errors if not --noCheck
                    (build_info.change_file_set.is_some()
                        || build_info.semantic_diagnostics_per_file.is_some()))
            {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::OutOfDateBuildInfoWithErrors,
                    UpToDateStatusData::String(build_info_path),
                );
            }

            // Pending emit files
            if !options.no_emit.is_true()
                && (build_info.change_file_set.is_some()
                    || build_info.affected_files_pending_emit.is_some())
            {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::OutOfDateBuildInfoWithPendingEmit,
                    UpToDateStatusData::String(build_info_path),
                );
            }

            // Some of the emit files like source map or dts etc are not yet done
            if build_info.is_emit_pending(&resolved, &build_info_directory) {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::OutOfDateOptions,
                    UpToDateStatusData::String(build_info_path),
                );
            }
        }
        let mut input_text_unchanged = false;
        let mut oldest_output_file_and_time = FileAndTime {
            file: build_info_path.clone(),
            time: build_info_time,
        };
        let mut newest_input_file_and_time = FileAndTime::default();
        // PORT: perf. With a prefetch, the prefetch thread made the set
        // (`StatusPrefetch::input_path_set`).
        let mut own_seen_roots: FxHashSet<Path> = FxHashSet::default();
        // Go `getBuildInfoRootInfoReader`, made once.
        let mut owned_reader = None;
        let make_reader = |owned_reader: &mut Option<BuildInfoRootInfoReader>| {
            if prefetched.is_none() && owned_reader.is_none() {
                *owned_reader = Some(build_info.get_build_info_root_info_reader(
                    &build_info_directory,
                    orchestrator.compare_paths_options(),
                ));
            }
        };
        for (index, input_file) in resolved.file_names().iter().enumerate() {
            // PORT: perf. `toPath` first, so the mtime lookup does not
            // compute it again (`get_m_time_of_path`).
            let owned_path;
            let input_path = match &*prefetched {
                Some(prefetched) => &prefetched.input_paths[index],
                None => {
                    owned_path = orchestrator.to_path(input_file);
                    &owned_path
                }
            };
            let prefetched_time = prefetched
                .as_ref()
                .and_then(|prefetched| prefetched.input_m_times.get(index).copied().flatten());
            let input_time =
                orchestrator.get_m_time_of_path(input_file, input_path, prefetched_time);
            if input_time.is_none() {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileMissing,
                    UpToDateStatusData::String(input_file.clone()),
                );
            }
            if input_time > oldest_output_file_and_time.time {
                let mut version = String::new();
                let mut current_version = String::new();
                if build_info.is_incremental() {
                    make_reader(&mut owned_reader);
                    let reader = reader_of(&prefetched, &owned_reader);
                    let (build_info_file_info, resolved_input_path) =
                        reader.get_build_info_file_info(input_path);
                    if let Some(file_info) = build_info_file_info.map(|b| b.get_file_info()) {
                        if !file_info.version().is_empty() {
                            version = file_info.version().to_string();
                            let (text, ok) =
                                orchestrator.fs().read_file(resolved_input_path.as_str());
                            if ok {
                                current_version =
                                    compute_hash(&text, orchestrator.testing().is_some());
                                if version == current_version {
                                    input_text_unchanged = true;
                                }
                            }
                        }
                    }
                }

                if version.is_empty() || version != current_version {
                    return UpToDateStatus::with_data(
                        UpToDateStatusType::InputFileNewer,
                        UpToDateStatusData::InputOutputName(InputOutputName {
                            input: input_file.clone(),
                            output: build_info_path,
                        }),
                    );
                }
            }
            if input_time > newest_input_file_and_time.time {
                newest_input_file_and_time = FileAndTime {
                    file: input_file.clone(),
                    time: input_time,
                };
            }
            if prefetched.is_none() {
                own_seen_roots.insert(input_path.clone());
            }
        }
        let seen_roots = match &*prefetched {
            Some(prefetched) => &prefetched.input_path_set,
            None => &own_seen_roots,
        };

        make_reader(&mut owned_reader);
        let reader = reader_of(&prefetched, &owned_reader);
        for root in reader.roots() {
            let root: &Path = &root;
            if !seen_roots.contains(root) {
                // File was root file when project was built but its not any more
                // ts#64159: the message names the root as the build info
                // spells it, not its path key (buildtask.go:470).
                return UpToDateStatus::with_data(
                    UpToDateStatusType::OutOfDateRoots,
                    UpToDateStatusData::InputOutputName(InputOutputName {
                        input: reader.root_file_name(root).to_string(),
                        output: build_info_path,
                    }),
                );
            }
        }

        if build_info.is_incremental() {
            let own_resolved_roots: FxHashSet<Path>;
            let resolved_roots = match &*prefetched {
                Some(prefetched) => &prefetched.resolved_roots,
                None => {
                    let mut resolved_roots = FxHashSet::default();
                    for root in reader.roots() {
                        let (_, resolved) = reader.get_build_info_file_info(root);
                        if !resolved.as_str().is_empty() {
                            resolved_roots.insert(resolved);
                        }
                    }
                    own_resolved_roots = resolved_roots;
                    &own_resolved_roots
                }
            };
            let file_names = build_info.file_names.as_deref().unwrap_or_default();
            for (index, build_info_file_info) in build_info.file_infos.iter().flatten().enumerate()
            {
                let build_info_file_name = &file_names[index];
                // Lib files bundled with the compiler can change only with the version of the compiler,
                // which is already verified with buildInfo.Version
                if is_build_info_file_name_default_library(build_info_file_name) {
                    continue;
                }
                let owned: (String, Path);
                let (input_file, input_path) = match &*prefetched {
                    Some(prefetched) => {
                        let (file, path) = &prefetched.file_names[index];
                        (file, path)
                    }
                    None => {
                        let input_file = get_normalized_absolute_path(
                            build_info_file_name,
                            &build_info_directory,
                        );
                        let input_path = orchestrator.to_path(&input_file);
                        owned = (input_file, input_path);
                        (&owned.0, &owned.1)
                    }
                };
                // Root files are already checked
                if seen_roots.contains(input_path) || resolved_roots.contains(input_path) {
                    continue;
                }
                // ts#63936: a supplemental file that exists is an input like any other.
                if is_content_mapper_supplemental_build_info_path(input_path, reader.roots())
                    && !orchestrator.fs().file_exists(input_file)
                {
                    continue;
                }
                let prefetched_time = prefetched
                    .as_ref()
                    .and_then(|prefetched| prefetched.file_m_times.get(index).copied().flatten());
                let input_time =
                    orchestrator.get_m_time_of_path(input_file, input_path, prefetched_time);
                if input_time.is_none() {
                    // Input file that was part of the program is missing (eg: dependency was removed)
                    return UpToDateStatus::with_data(
                        UpToDateStatusType::InputFileMissing,
                        UpToDateStatusData::String(input_file.clone()),
                    );
                }
                if input_time > oldest_output_file_and_time.time {
                    let mut current_version = String::new();
                    let version = build_info_file_info.get_file_info().version().to_string();
                    if !version.is_empty() {
                        let (text, ok) = orchestrator.fs().read_file(input_file);
                        if ok {
                            current_version = compute_hash(&text, orchestrator.testing().is_some());
                        }
                    }
                    if version.is_empty() || version != current_version {
                        return UpToDateStatus::with_data(
                            UpToDateStatusType::InputFileNewer,
                            UpToDateStatusData::InputOutputName(InputOutputName {
                                input: input_file.clone(),
                                output: build_info_path,
                            }),
                        );
                    }
                    input_text_unchanged = true;
                }
            }
        }

        if !options.is_incremental() {
            // Check output file stamps
            for output_file in resolved.get_output_file_names() {
                let output_time = orchestrator.get_m_time(&output_file);
                if output_time.is_none() {
                    // Output file missing
                    return UpToDateStatus::with_data(
                        UpToDateStatusType::OutputMissing,
                        UpToDateStatusData::String(output_file),
                    );
                }

                if output_time < newest_input_file_and_time.time {
                    // Output file is older than input file
                    return UpToDateStatus::with_data(
                        UpToDateStatusType::InputFileNewer,
                        UpToDateStatusData::InputOutputName(InputOutputName {
                            input: newest_input_file_and_time.file,
                            output: output_file,
                        }),
                    );
                }

                if output_time < oldest_output_file_and_time.time {
                    oldest_output_file_and_time = FileAndTime {
                        file: output_file,
                        time: output_time,
                    };
                }
            }
        }

        let mut ref_dts_unchanged = false;
        for upstream in &self.up_stream {
            let upstream_kind = upstream.task.borrow().status().kind;
            if upstream_kind == UpToDateStatusType::Solution {
                // Not dependent on the status or this upstream project
                // (eg: expected cycle was detected and hence skipped, or is solution)
                continue;
            }

            // If the upstream project's newest file is older than our oldest output,
            // we can't be out of date because of it
            // inputTime will not be present if we just built this project or updated timestamps
            // - in that case we do want to either build or update timestamps
            let skip = match upstream.task.borrow().status().input_output_file_and_time() {
                Some(ref_input_output_file_and_time) => {
                    ref_input_output_file_and_time.input.time.is_some()
                        && ref_input_output_file_and_time.input.time
                            < oldest_output_file_and_time.time
                }
                None => false,
            };
            if skip {
                continue;
            }

            // Check if tsbuildinfo path is shared, then we need to rebuild
            if self.has_conflicting_build_info(orchestrator, &upstream.task.borrow()) {
                // We have an output older than an upstream output - we are out of date
                return UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileNewer,
                    UpToDateStatusData::InputOutputName(InputOutputName {
                        input: resolved.project_references()[upstream.ref_index]
                            .path
                            .clone(),
                        output: oldest_output_file_and_time.file,
                    }),
                );
            }

            // If the upstream project has only change .d.ts files, and we've built
            // *after* those files, then we're "pseudo up to date" and eligible for a fast rebuild
            let newest_dts_change_time = upstream
                .task
                .borrow_mut()
                .get_latest_changed_dts_m_time(orchestrator);
            if newest_dts_change_time.is_some()
                && newest_dts_change_time < oldest_output_file_and_time.time
            {
                ref_dts_unchanged = true;
                continue;
            }

            // We have an output older than an upstream output - we are out of date
            return UpToDateStatus::with_data(
                UpToDateStatusType::InputFileNewer,
                UpToDateStatusData::InputOutputName(InputOutputName {
                    input: resolved.project_references()[upstream.ref_index]
                        .path
                        .clone(),
                    output: oldest_output_file_and_time.file,
                }),
            );
        }

        let check_input_file_time = |input_file: &str| -> Option<UpToDateStatus> {
            let input_time = orchestrator.get_m_time(input_file);
            if input_time > oldest_output_file_and_time.time {
                // Output file is older than input file
                return Some(UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileNewer,
                    UpToDateStatusData::InputOutputName(InputOutputName {
                        input: input_file.to_string(),
                        output: oldest_output_file_and_time.file.clone(),
                    }),
                ));
            }
            None
        };

        if let Some(config_status) = check_input_file_time(&self.config) {
            return config_status;
        }

        for extended_config in resolved.extended_source_files() {
            if let Some(extended_config_status) = check_input_file_time(extended_config) {
                return extended_config_status;
            }
        }

        // PORT: perf. Go normalizes each list twice, for the checks and for
        // `t.packageJsons`; here once, or on a prefetch thread.
        let (package_jsons, missing_package_jsons) =
            match prefetched.as_mut().and_then(Arc::get_mut) {
                Some(prefetched) => (
                    std::mem::take(&mut prefetched.package_jsons),
                    std::mem::take(&mut prefetched.missing_package_jsons),
                ),
                None => (
                    build_info
                        .get_package_jsons(&build_info_directory)
                        .collect::<Vec<_>>(),
                    build_info
                        .get_missing_package_jsons(&build_info_directory)
                        .collect::<Vec<_>>(),
                ),
            };
        for package_json in &package_jsons {
            let package_json_time = orchestrator.get_m_time(package_json);
            if package_json_time.is_none() {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileMissing,
                    UpToDateStatusData::String(package_json.clone()),
                );
            }
            if package_json_time > oldest_output_file_and_time.time {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileNewer,
                    UpToDateStatusData::InputOutputName(InputOutputName {
                        input: package_json.clone(),
                        output: oldest_output_file_and_time.file.clone(),
                    }),
                );
            }
        }
        for package_json in &missing_package_jsons {
            if orchestrator.get_m_time(package_json).is_some() {
                return UpToDateStatus::with_data(
                    UpToDateStatusType::InputFileNewer,
                    UpToDateStatusData::InputOutputName(InputOutputName {
                        input: package_json.clone(),
                        output: oldest_output_file_and_time.file.clone(),
                    }),
                );
            }
        }
        let mut all_package_jsons = package_jsons;
        all_package_jsons.extend(missing_package_jsons);
        self.package_jsons = all_package_jsons;

        UpToDateStatus::with_data(
            if ref_dts_unchanged {
                UpToDateStatusType::UpToDateWithUpstreamTypes
            } else if input_text_unchanged {
                UpToDateStatusType::UpToDateWithInputFileText
            } else {
                UpToDateStatusType::UpToDate
            },
            UpToDateStatusData::InputOutputFileAndTime(InputOutputFileAndTime {
                input: newest_input_file_and_time,
                output: oldest_output_file_and_time,
                build_info: build_info_path,
            }),
        )
    }

    // Go: build/buildtask.go:640 (*BuildTask).reportUpToDateStatus
    pub fn report_up_to_date_status(&mut self, orchestrator: &dyn BuildTaskOrchestrator) {
        if !orchestrator.command().build_options.verbose.is_true() {
            return;
        }
        let o = orchestrator;
        let config = o.relative_file_name(&self.config);
        let status = self.status().clone();
        let diagnostic = match status.kind {
            UpToDateStatusType::ConfigFileNotFound => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_config_file_does_not_exist,
                args![config],
            ),
            UpToDateStatusType::UpstreamErrors => {
                let upstream_status = status.upstream_errors();
                new_compiler_diagnostic(
                    if upstream_status.ref_has_upstream_errors {
                        diag::Project_0_can_t_be_built_because_its_dependency_1_was_not_built
                    } else {
                        diag::Project_0_can_t_be_built_because_its_dependency_1_has_errors
                    },
                    args![config, o.relative_file_name(&upstream_status.ref_)],
                )
            }
            UpToDateStatusType::BuildErrors => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_it_has_errors,
                args![config],
            ),
            UpToDateStatusType::UpToDate => {
                // This is to ensure skipping verbose log for projects that were built,
                // and then some other package changed but this package doesnt need update
                let Some(input_output_file_and_time) = status.input_output_file_and_time() else {
                    return;
                };
                new_compiler_diagnostic(
                    diag::Project_0_is_up_to_date_because_newest_input_1_is_older_than_output_2,
                    args![
                        config,
                        o.relative_file_name(&input_output_file_and_time.input.file),
                        o.relative_file_name(&input_output_file_and_time.output.file)
                    ],
                )
            }
            UpToDateStatusType::UpToDateWithUpstreamTypes => new_compiler_diagnostic(
                diag::Project_0_is_up_to_date_with_d_ts_files_from_its_dependencies,
                args![config],
            ),
            UpToDateStatusType::UpToDateWithInputFileText => new_compiler_diagnostic(
                diag::Project_0_is_up_to_date_but_needs_to_update_timestamps_of_output_files_that_are_older_than_input_files,
                args![config],
            ),
            UpToDateStatusType::InputFileMissing => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_input_1_does_not_exist,
                args![config, o.relative_file_name(status.data_string())],
            ),
            UpToDateStatusType::OutputMissing => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_output_file_1_does_not_exist,
                args![config, o.relative_file_name(status.data_string())],
            ),
            UpToDateStatusType::InputFileNewer => {
                let input_output = status.input_output_name().expect("inputOutputName");
                new_compiler_diagnostic(
                    diag::Project_0_is_out_of_date_because_output_1_is_older_than_input_2,
                    args![
                        config,
                        o.relative_file_name(&input_output.output),
                        o.relative_file_name(&input_output.input)
                    ],
                )
            }
            UpToDateStatusType::OutOfDateBuildInfoWithPendingEmit => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_some_of_the_changes_were_not_emitted,
                args![config, o.relative_file_name(status.data_string())],
            ),
            UpToDateStatusType::OutOfDateBuildInfoWithErrors => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_program_needs_to_report_errors,
                args![config, o.relative_file_name(status.data_string())],
            ),
            UpToDateStatusType::OutOfDateOptions => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_there_is_change_in_compilerOptions,
                args![config, o.relative_file_name(status.data_string())],
            ),
            UpToDateStatusType::OutOfDateRoots => {
                let input_output = status.input_output_name().expect("inputOutputName");
                new_compiler_diagnostic(
                    diag::Project_0_is_out_of_date_because_buildinfo_file_1_indicates_that_file_2_was_root_file_of_compilation_but_not_any_more,
                    args![
                        config,
                        o.relative_file_name(&input_output.output),
                        o.relative_file_name(&input_output.input)
                    ],
                )
            }
            UpToDateStatusType::TsVersionOutputOfDate => new_compiler_diagnostic(
                diag::Project_0_is_out_of_date_because_output_for_it_was_generated_with_version_1_that_differs_with_current_version_2,
                // ts#64159: the build info version as it is (buildtask.go:737).
                args![
                    config,
                    status.data_string(),
                    build_info_version(self.resolved.as_ref().is_some_and(|r| {
                        crate::effect::rulerunner::enabled_options(r.compiler_options()).is_some()
                    }))
                ],
            ),
            UpToDateStatusType::ForceBuild => new_compiler_diagnostic(
                diag::Project_0_is_being_forcibly_rebuilt,
                args![config],
            ),
            UpToDateStatusType::Solution => {
                // Does not need to report status
                return;
            }
        };
        self.report_status(diagnostic);
    }

    // Go: build/buildtask.go:752 (*BuildTask).canUpdateJsDtsOutputTimestamps
    pub fn can_update_js_dts_output_timestamps(&self) -> bool {
        let options = self.resolved().compiler_options();
        !options.no_emit.is_true() && !options.is_incremental()
    }

    // Go: build/buildtask.go:756 (*BuildTask).updateTimeStamps
    pub fn update_time_stamps(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        emitted_files: &[String],
        verbose_message: &'static Message,
    ) {
        let emitted: FxHashSet<&str> = emitted_files.iter().map(String::as_str).collect();
        let mut verbose_message_reported = false;
        let build_info_name = self.resolved().get_build_info_file_name();
        let now = orchestrator.now();
        let mut update_time_stamp = |this: &mut BuildTask, file: &str| {
            if emitted.contains(file) {
                return;
            }
            if !verbose_message_reported && orchestrator.command().build_options.verbose.is_true() {
                let config = orchestrator.relative_file_name(&this.config);
                this.report_status(new_compiler_diagnostic(verbose_message, args![config]));
                verbose_message_reported = true;
            }
            let err = orchestrator.set_m_time(file, now);
            if err.is_ok() {
                if file == build_info_name {
                    if let Some(entry) = &mut this.build_info_entry {
                        entry.m_time = Some(now);
                    }
                } else if this.store_output_time_stamp(orchestrator) {
                    orchestrator.store_m_time(file, now);
                }
            }
        };

        if self.can_update_js_dts_output_timestamps() {
            let resolved = self.resolved().clone();
            for output_file in resolved.get_output_file_names() {
                update_time_stamp(self, &output_file);
            }
        }
        let build_info_file_name = self.resolved().get_build_info_file_name();
        update_time_stamp(self, &build_info_file_name);
    }

    // Go: build/buildtask.go:791 (*BuildTask).cleanProject
    pub fn clean_project(&mut self, orchestrator: &dyn BuildTaskOrchestrator, path: &Path) {
        let Some(resolved) = self.resolved.clone() else {
            let config = self.config.clone();
            self.report_diagnostic(new_compiler_diagnostic(
                diag::File_0_not_found,
                args![config],
            ));
            self.result_mut().exit_status = ExitStatus::DiagnosticsPresentOutputsSkipped;
            return;
        };

        let inputs: FxHashSet<Path> = resolved
            .file_names()
            .iter()
            .map(|file_name| orchestrator.to_path(file_name))
            .collect();
        for output_file in resolved.get_output_file_names() {
            self.clean_project_output(orchestrator, &output_file, &inputs);
        }
        self.clean_project_output(orchestrator, &resolved.get_build_info_file_name(), &inputs);
    }

    // Go: build/buildtask.go:807 (*BuildTask).cleanProjectOutput
    pub fn clean_project_output(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        output_file: &str,
        inputs: &FxHashSet<Path>,
    ) {
        let output_path = orchestrator.to_path(output_file);
        // If output name is same as input file name, do not delete and ignore the error
        if inputs.contains(&output_path) {
            return;
        }
        let fs = orchestrator.fs();
        if fs.file_exists(output_file) {
            if !orchestrator.command().build_options.dry.is_true() {
                let err = fs.remove(output_file);
                if err.is_err() {
                    self.report_diagnostic(new_compiler_diagnostic(
                        diag::Failed_to_delete_file_0,
                        args![output_file],
                    ));
                }
            } else {
                self.result_mut()
                    .files_to_delete
                    .push(output_file.to_string());
            }
        }
    }

    // Go: build/buildtask.go:825 (*BuildTask).updateWatch
    // PORT: in orchestrator_watch.rs.

    /// PORT: not in Go (perf). Drops what a prefetch thread made for the
    /// check of this task (`BuildInfoEntry::status_prefetch`) when the check
    /// did not take it: it returned before (a content mapper error, errors
    /// in the build info), or only the compile read the build info. The
    /// entry can outlive the build (watch reuses it when the compile writes
    /// no build info), and the mtimes in it are of this build: the check of
    /// a later build reads the file system, as Go's does
    /// (build/host.go:102 loadOrStoreMTime, with the new mtime cache of
    /// build/orchestrator.go:498 updateWatch). The orchestrator calls this
    /// for each task at the end of a build.
    pub fn drop_status_prefetch(&mut self) {
        if let Some(prefetch) = self
            .build_info_entry
            .as_mut()
            .and_then(|entry| entry.status_prefetch.take())
        {
            drop_in_background(prefetch);
        }
    }

    // Go: build/buildtask.go:835 (*BuildTask).resetStatus
    pub fn reset_status(&mut self) {
        self.status = None;
        self.pending = true;
        self.errors = Vec::new();
        // PORT: Go's GC frees a file that only `t.errors` held. The versions
        // that `errors` held go with them, also when the task is not built
        // again in this cycle (a dependency with errors).
        self.held_file_versions = Vec::new();
    }

    // Go: build/buildtask.go:841 (*BuildTask).resetConfig
    // PORT: in orchestrator_watch.rs.

    // Go: build/buildtask.go:846 (*BuildTask).loadOrStoreBuildInfo
    pub fn load_or_store_build_info(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        config_path: &Path,
        build_info_file_name: &str,
    ) -> (Option<Rc<BuildInfo>>, Option<SystemTime>) {
        let path = orchestrator.to_path(build_info_file_name);
        if let Some(entry) = &self.build_info_entry {
            if entry.path == path {
                return (entry.build_info.clone(), entry.m_time);
            }
        }
        let build_info = orchestrator.read_build_info_file(self.resolved());
        let status_prefetch = orchestrator
            .take_status_prefetch(build_info_file_name)
            .map(Arc::new);
        let mut m_time = None;
        if build_info.is_some() {
            m_time = orchestrator.get_m_time(build_info_file_name);
        }
        self.build_info_entry = Some(BuildInfoEntry {
            build_info: build_info.clone(),
            file_name: get_normalized_absolute_path(
                build_info_file_name,
                &orchestrator.compare_paths_options().current_directory,
            ),
            path,
            m_time,
            dts_time: None,
            status_prefetch,
        });
        (build_info, m_time)
    }

    // Go: build/buildtask.go:866 (*BuildTask).onBuildInfoEmit
    // PORT: Go takes `mTime := orchestrator.opts.Sys.Now()` here, in the
    // `writeFile` call of the build info, before the test `OnEmittedFiles`
    // stamps the emitted files. `new_task_write_file` takes the time at
    // that write, and the caller passes it as `m_time`.
    pub fn on_build_info_emit(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
        build_info_file_name: &str,
        build_info: Option<Rc<BuildInfo>>,
        has_changed_dts_file: bool,
        m_time: SystemTime,
    ) {
        let dts_time = if has_changed_dts_file {
            Some(Some(m_time))
        } else if let Some(entry) = &self.build_info_entry {
            entry.dts_time
        } else {
            None
        };
        self.build_info_entry = Some(BuildInfoEntry {
            build_info,
            file_name: get_normalized_absolute_path(
                build_info_file_name,
                &orchestrator.compare_paths_options().current_directory,
            ),
            path: orchestrator.to_path(build_info_file_name),
            m_time: Some(m_time),
            dts_time,
            status_prefetch: None,
        });
    }

    // Go: build/buildtask.go:885 (*BuildTask).hasConflictingBuildInfo
    pub fn has_conflicting_build_info(
        &self,
        orchestrator: &dyn BuildTaskOrchestrator,
        upstream: &BuildTask,
    ) -> bool {
        if let (Some(entry), Some(upstream_entry)) =
            (&self.build_info_entry, &upstream.build_info_entry)
        {
            return entry.path == upstream_entry.path;
        }
        false
    }

    // Go: build/buildtask.go:892 (*BuildTask).getLatestChangedDtsMTime
    pub fn get_latest_changed_dts_m_time(
        &mut self,
        orchestrator: &dyn BuildTaskOrchestrator,
    ) -> Option<SystemTime> {
        // Go reads `t.buildInfoEntry.dtsTime` and panics on a nil entry (an
        // upstream task whose config file is gone or cannot be read, so its
        // check never loaded a build info).
        let Some(entry) = self.build_info_entry.as_mut() else {
            crate::core::go_nil_dereference()
        };
        if let Some(dts_time) = entry.dts_time {
            return dts_time;
        }
        // Go reads `t.buildInfoEntry.buildInfo.LatestChangedDtsFile` and
        // panics on a nil build info (an upstream build info that cannot be
        // read).
        let Some(build_info) = entry.build_info.as_ref() else {
            crate::core::go_nil_dereference()
        };
        // ts#64159: `incremental.ResolveBuildInfoFileName` against the build
        // info file's directory, not its path key's (buildtask.go:898-904).
        // An empty name (no .d.ts emitted yet) is a default library name: it
        // gives the default library directory, whose bundled `Stat` has the
        // zero mtime.
        let dts_time = orchestrator.get_m_time(&resolve_build_info_file_name(
            &build_info.latest_changed_dts_file,
            &get_directory_path(&entry.file_name),
            &CompilerHost::default_library_path(&*orchestrator.host()),
        ));
        entry.dts_time = Some(dts_time);
        dts_time
    }

    // Go: build/buildtask.go:909 (*BuildTask).storeOutputTimeStamp
    pub fn store_output_time_stamp(&self, orchestrator: &dyn BuildTaskOrchestrator) -> bool {
        orchestrator.command().compiler_options.watch.is_true()
            && !self.resolved().compiler_options().is_incremental()
    }

    // Go: build/buildtask.go:913 (*BuildTask).writeFile
    // PORT: see `new_task_write_file`.
}

/// The root info reader of an up-to-date check: the prefetched one, or the
/// one that the check made.
fn reader_of<'a>(
    prefetched: &'a Option<Arc<StatusPrefetch>>,
    owned: &'a Option<BuildInfoRootInfoReader>,
) -> &'a BuildInfoRootInfoReader {
    match prefetched {
        Some(prefetched) => &prefetched.root_info_reader,
        None => owned.as_ref().expect("the check made its root info reader"),
    }
}

// Go: build/buildtask.go:623 isContentMapperSupplementalBuildInfoPath (tsgo#4712)
// PORT: Go `strconv.Atoi(index) == nil` is `index.parse::<i64>().is_ok()`:
// both take an optional sign and decimal digits, and fail on an empty
// text and on overflow of a 64-bit int.
fn is_content_mapper_supplemental_build_info_path<'a>(
    input_path: &Path,
    roots: impl Iterator<Item = &'a Path>,
) -> bool {
    for root in roots {
        let Some(suffix) = input_path
            .as_str()
            .strip_prefix(root.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        let Some((index, extension)) = suffix.split_once('.') else {
            continue;
        };
        if extension.is_empty() {
            continue;
        }
        if index.parse::<i64>().is_ok()
            && contentmapper::is_supported_virtual_extension(&format!(".{extension}"))
        {
            return true;
        }
    }
    false
}

/// The build info that the emit wrote: its file name, Go
/// `WriteFileData.BuildInfo`, and the Go `Sys.Now()` of `onBuildInfoEmit`,
/// taken at the write.
type WrittenBuildInfo = Arc<Mutex<Option<(String, Arc<BuildInfo>, SystemTime)>>>;

// Go: build/buildtask.go:913 (*BuildTask).writeFile
// PORT: emit writes the source outputs on the checker threads, so the
// callback is `Send` and cannot hold the task, the `Rc` system or the `Rc`
// file system. Go writes through `orchestrator.host.FS()` (cachedvfs over
// bundled over osvfs); both wrappers pass a write of a real path to osvfs
// (cachedvfs.go:144 WriteFile), so this writes with the osvfs of the
// calling thread. The build info branch keeps what `onBuildInfoEmit` needs
// in `written`, and `compile_and_emit_finish` calls it after the emit. The
// watch-only `storeMTime` branch stores into the build host `m_times` at
// once (Go `SyncMap`), before the test `OnEmittedFiles` reads it.
// With `deferred` (`System::emit_writes_through_osvfs` is false) it writes
// through the system file system instead (see `DeferredWrites`). Each
// written path goes into `written_paths`, the build host's `written` (not
// in Go).
fn new_task_write_file(
    written: WrittenBuildInfo,
    deferred: Option<DeferredWrites>,
    store_output_time_stamp: bool,
    m_times: Arc<Mutex<FxHashMap<Path, Option<SystemTime>>>>,
    written_paths: Arc<WrittenPaths>,
    compare_paths_options: ComparePathsOptions,
) -> WriteFile {
    Arc::new(
        move |file_name: &str, text: &str, data: &mut WriteFileData| -> Result<(), String> {
            let path = || {
                to_path(
                    file_name,
                    &compare_paths_options.current_directory,
                    compare_paths_options.use_case_sensitive_file_names,
                )
            };
            written_paths.insert(path());
            match &deferred {
                None => osvfs_fs()
                    .write_file(file_name, text)
                    .map_err(|err| fs_error_text(&err))?,
                Some(deferred) => write_or_defer(deferred, file_name, text)?,
            }
            if let Some(build_info) = &data.build_info {
                *written.lock().unwrap_or_else(PoisonError::into_inner) = Some((
                    file_name.to_string(),
                    build_info.clone(),
                    task_write_file_now(),
                ));
            } else if store_output_time_stamp {
                // Store time stamps
                // Go: orchestrator.host.storeMTime(fileName, orchestrator.opts.Sys.Now())
                let m_time = task_write_file_now();
                m_times
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(path(), Some(m_time));
            }
            Ok(())
        },
    )
}

/// PORT: not in Go. The emit writes of a task whose system file system only
/// the orchestrator thread can reach (`System::emit_writes_through_osvfs`,
/// the API build orchestrator). Go's `writeFile` writes through
/// `orchestrator.host.FS()` from the emit goroutines. Here a write on the
/// orchestrator thread goes to that file system at once, after the waiting
/// ones. That is the build info, and every source output: the emit keeps
/// them and makes them on that thread (`flush_writes_on_this_thread`), so a
/// failed write gets Go's TS5033 in its file's result. A write on another
/// thread (a checker thread) waits in `writes`, and the next write on the
/// orchestrator thread, or the end of the task's emit, makes it. A waiting
/// write that fails is kept in `failed` for a TS5033 after the emit
/// (`compile_and_emit_finish`).
#[derive(Default)]
struct PendingWrites {
    writes: Vec<(String, String)>,
    failed: Vec<(String, String)>,
}

type DeferredWrites = Arc<Mutex<PendingWrites>>;

/// The `DeferredWrites` part of the task `writeFile`: writes through the
/// orchestrator system on the orchestrator thread, else keeps the write.
fn write_or_defer(deferred: &DeferredWrites, file_name: &str, text: &str) -> Result<(), String> {
    let fs = WRITE_FILE_SYS.with(|sys| sys.borrow().as_ref().map(|sys| sys.fs()));
    match fs {
        Some(fs) => {
            write_deferred(deferred, &*fs);
            fs.write_file(file_name, text)
                .map_err(|err| fs_error_text(&err))
        }
        None => {
            deferred
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .writes
                .push((file_name.to_string(), text.to_string()));
            Ok(())
        }
    }
}

/// Makes the waiting writes of `deferred` through `fs`, in order.
fn write_deferred(deferred: &DeferredWrites, fs: &dyn Fs) {
    let writes = std::mem::take(
        &mut deferred
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .writes,
    );
    for (file_name, text) in writes {
        if let Err(err) = fs.write_file(&file_name, &text) {
            deferred
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .failed
                .push((file_name, fs_error_text(&err)));
        }
    }
}

thread_local! {
    // Go `orchestrator.opts.Sys` of the task `writeFile`: the orchestrator
    // system, on the orchestrator thread, while a task emits (see
    // `task_write_file_now`).
    static WRITE_FILE_SYS: RefCell<Option<Rc<dyn System>>> = const { RefCell::new(None) };
}

// Go `orchestrator.opts.Sys.Now()` in the task `writeFile`.
// PORT: emit writes the source files' outputs on the checker threads,
// which cannot hold the `Rc` system; there it is the OS system's `Now`
// (`SystemTime::now`). The build info write runs on the orchestrator thread
// (incremental `emitBuildInfo`), so its time comes from the orchestrator
// system, in the same order as Go with the test `OnEmittedFiles` times.
fn task_write_file_now() -> SystemTime {
    WRITE_FILE_SYS
        .with(|sys| sys.borrow().as_ref().map(|sys| sys.now()))
        .unwrap_or_else(SystemTime::now)
}

/// Go `o.opts.Sys.Now().Sub(start)`.
fn elapsed(sys: &dyn System, start: SystemTime) -> std::time::Duration {
    sys.now().duration_since(start).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: build/buildtask_contentmapper_test.go:11 TestIsContentMapperSupplementalBuildInfoPath (tsgo#4712)
    #[test]
    fn test_is_content_mapper_supplemental_build_info_path() {
        let roots = [
            Path("/src/app.vue".to_string()),
            Path("/src/index.ts".to_string()),
        ];
        let path = |text: &str| Path(text.to_string());

        assert!(is_content_mapper_supplemental_build_info_path(
            &path("/src/app.vue.0.ts"),
            roots.iter()
        ));
        assert!(is_content_mapper_supplemental_build_info_path(
            &path("/src/app.vue.12.mts"),
            roots.iter()
        ));
        assert!(!is_content_mapper_supplemental_build_info_path(
            &path("/src/app.vue.ts"),
            roots.iter()
        ));
        assert!(!is_content_mapper_supplemental_build_info_path(
            &path("/src/app.vue.0.txt"),
            roots.iter()
        ));
        assert!(!is_content_mapper_supplemental_build_info_path(
            &path("/src/other.vue.0.ts"),
            roots.iter()
        ));
    }
}
