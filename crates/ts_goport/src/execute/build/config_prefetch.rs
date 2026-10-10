//! PORT: not in Go (perf). Go parses the configs of a build in parallel:
//! `createBuildTasks` queues the parse of each config and of its
//! references on a work group (orchestrator.go:174). Here the configs parse
//! on the orchestrator thread, because `ParsedCommandLine` is not `Send`.
//! Most of a parse is the match of the `include` specs against the file
//! system (Go `getFileNamesFromConfigSpecs`). So threads parse the configs
//! of the build ahead of the orchestrator, each on the OS file system of
//! its thread with its own caches, and keep the file names that each
//! config's specs match. The orchestrator's parse of a config takes them
//! (`BuildHost`'s `ParseConfigHost::get_file_names_from_config_specs`)
//! when it matches the same specs from the same base path with the same
//! extensions, and waits for a thread that is still matching them. The
//! file names are a function of these inputs and of the file system, and
//! the build writes nothing before its graph is made. A config that no
//! thread has started yet is the orchestrator's own: it matches the specs
//! itself and queues the references it finds.
//!
//! Go caches the lookups of each match (`GetAccessibleEntries`,
//! `Realpath`) in the build host's `cachedvfs` for the rest of the build,
//! and a later program reads them: for example the listing of a typeRoots
//! directory for its automatic type directives, after the build wrote
//! into that directory. So a thread records the lookups of its match, and
//! when the orchestrator takes the match, they go into the host's cache
//! (`BuildStatCache::add`), as if the orchestrator had made them. The
//! thread made them before the build wrote anything, as Go does.
//!
//! The same threads read the build info file of each task ahead of its
//! up-to-date check (`BuildInfoRead`; orchestrator.rs `BuildInfoPrefetch`).
//! Go reads it in the check, on the builder goroutine of the task
//! (buildtask.go `loadOrStoreBuildInfo`). A thread queues the read of the
//! build info file of a config when it has parsed it, and the orchestrator
//! queues the read of a config that it parsed (`queue_read`), so the reads
//! run while the graph is made. The threads take the queued configs first:
//! the orchestrator waits for each config before it makes the graph. When
//! the graph is made, the orchestrator keeps the reads that its checks can
//! use (`finish_reads`; the rules are in orchestrator.rs
//! `start_build_info_prefetch`) and drops the others. A read only reads,
//! and the build writes nothing before its graph is made. For those rules,
//! a thread also finds the key of the build info file of each config it
//! parsed (`PathKeys::build_info_key`) before the orchestrator can take the
//! config, so before the graph is made, as the orchestrator does for the
//! other keys (`build_info_key`).
//!
//! The pool starts a thread for each queued job that no thread is free
//! for, up to `MAX_PREFETCH_THREADS` and the cores. A new thread starts the
//! other new threads (`spawn_threads`), so the thread that queues pays for
//! one thread start.

use crate::execute::build::build_task::{StatusCheckOptions, StatusPrefetch};
use crate::execute::build::host::TscExtendedConfigCache;
use crate::execute::build::shared_outputs::PathKeys;
use crate::execute::incremental::build_info::BuildInfo;
use crate::execute::incremental::incremental::parse_build_info;
use crate::frontend::prelude::*;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// The most threads of a `PrefetchPool`. The threads only read, so their
/// count changes no output; it is not Go's `numRoutines`. On 8 cores with
/// 16 hardware threads, 12 or 16 threads made the noop of 48 small
/// projects 1 ms slower than 8 (noop1, mini-abf9): the orchestrator thread
/// shares a core with a prefetch thread.
const MAX_PREFETCH_THREADS: usize = 8;

/// What `get_file_names_from_config_specs` reads, other than the file
/// system: two matches with equal inputs give the same file names.
#[derive(Clone, PartialEq, Eq)]
struct MatchInputs {
    base_path: String,
    files: Vec<String>,
    include: Vec<String>,
    exclude: Vec<String>,
    /// `get_supported_extensions` and its `WithJsonIfResolveJsonModule`
    /// form: the only parts of the options that the match reads.
    extensions: Vec<Vec<String>>,
    extensions_with_json: Vec<Vec<String>>,
}

impl MatchInputs {
    /// The inputs of a match. `None` without options (the match panics
    /// there, and then the orchestrator's own match panics too).
    fn of(
        specs: &ConfigFileSpecs,
        base_path: &str,
        options: Option<&CompilerOptions>,
        extra_extensions: &[String],
    ) -> Option<Self> {
        let options = options?;
        let extensions = get_supported_extensions(options, extra_extensions);
        let extensions_with_json = get_supported_extensions_with_json_if_resolve_json_module(
            Some(options),
            extensions.clone(),
        );
        Some(MatchInputs {
            base_path: base_path.to_string(),
            files: specs.validated_files_spec.clone(),
            include: specs.validated_include_specs.clone(),
            exclude: specs.validated_exclude_specs.clone(),
            extensions,
            extensions_with_json,
        })
    }
}

/// The file names that a thread matched for one config.
struct MatchedFileNames {
    inputs: MatchInputs,
    file_names: Vec<String>,
    literal_file_names_len: i32,
    /// The cached lookups that the match made (`RecordingFs`).
    lookups: StatCache,
}

enum SlotState {
    /// No thread has started the config.
    Queued,
    /// A thread parses the config.
    Running,
    /// The thread's match, `None` when the parse matched nothing (no
    /// config file, no options) or panicked.
    Done(Option<MatchedFileNames>),
    /// The thread's work after the parse (`parse_config`) panicked with
    /// this payload. The orchestrator panics with it when it takes the
    /// slot.
    Panicked(Box<dyn std::any::Any + Send>),
    /// The orchestrator took the slot.
    Taken,
}

struct Slot {
    state: Mutex<SlotState>,
    done: Condvar,
}

/// One build info file that a thread reads ahead of the up-to-date check
/// of its task: the file, the root files of the task (`resolved.FileNames()`)
/// and the options that the check reads.
#[derive(PartialEq)]
pub(crate) struct BuildInfoRead {
    pub(crate) name: String,
    pub(crate) input_files: Vec<String>,
    pub(crate) check: StatusCheckOptions,
}

impl BuildInfoRead {
    /// The read for the check of the task of `config`. None when the
    /// config names no build info file, and for a solution (Go
    /// `upToDateStatusTypeSolution`), whose check reads nothing.
    pub(crate) fn of(config: &ParsedCommandLine) -> Option<Self> {
        let name = config.get_build_info_file_name();
        let solution = config.file_names().is_empty() && config.has_project_references();
        (!name.is_empty() && !solution).then(|| BuildInfoRead {
            name,
            input_files: config.file_names().to_vec(),
            check: StatusCheckOptions::new(config.compiler_options()),
        })
    }
}

/// What a thread made for one build info file: Go `ReadBuildInfo`'s result
/// (`None` when the file cannot be read or parsed) and the check parts of
/// a parsed build info.
pub(crate) type BuildInfoResult = (Option<BuildInfo>, Option<StatusPrefetch>);

/// The result of one read: `None` until a thread has read the file, then
/// `Some(None)` when the thread panicked, else `Some(Some(result))`.
#[derive(Default)]
pub(crate) struct BuildInfoSlot {
    result: Mutex<Option<Option<BuildInfoResult>>>,
    done: Condvar,
}

impl BuildInfoSlot {
    /// The result, once: waits for the thread that reads the file. None
    /// when the read panicked; then the caller reads.
    pub(crate) fn take(&self) -> Option<BuildInfoResult> {
        let mut result = lock(&self.result);
        loop {
            if let Some(read) = result.take() {
                return read;
            }
            result = self
                .done
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A queued build info read.
#[derive(Clone)]
struct ReadJob {
    read: Arc<BuildInfoRead>,
    slot: Arc<BuildInfoSlot>,
    /// The key of the file (`PathKeys::build_info_key`) that a thread found
    /// before the graph was made; None when the orchestrator queued the
    /// read.
    key: Option<Option<String>>,
}

struct Queue {
    /// Configs (file name and path) that no thread has taken, in the
    /// order they were found. Threads take them before reads.
    configs: VecDeque<(String, Path, Arc<Slot>)>,
    /// Build info reads that no thread has taken.
    reads: VecDeque<ReadJob>,
    /// Each read that a thread runs or that is queued, by the path of its
    /// task's config.
    reads_by_config: FxHashMap<Path, ReadJob>,
    /// No config is queued any more (the graph is made).
    configs_closed: bool,
    /// No job is queued any more: a thread ends when the queues are empty.
    closed: bool,
    /// Set by `finish_reads`: the checks wait for the queued reads, so
    /// dropping the pool keeps them.
    reads_final: bool,
    /// The threads that started or are starting.
    threads: usize,
    /// The threads that run a job.
    busy: usize,
}

/// The parse options of the build: what `BuildHost::get_resolved_project_reference`
/// passes to `get_parsed_command_line_of_config_file_path`.
struct ParseOptions {
    compiler_options: CompilerOptions,
    command_line_raw: Option<IndexMap<String, CompilerOptionsValue>>,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Every config that was queued, by path.
    slots: Mutex<FxHashMap<Path, Arc<Slot>>>,
    /// None for a pool that only reads build info files.
    options: Option<ParseOptions>,
    compare_paths_options: ComparePathsOptions,
    /// True when a thread queues the build info read of each config that
    /// it parses.
    reads_build_info: bool,
    /// `MAX_PREFETCH_THREADS`, at most the cores.
    max_threads: usize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The threads that read ahead of the orchestrator: the configs of one
/// graph and the build info files of its tasks. Dropping it closes the
/// queue; each thread ends after its job.
pub struct PrefetchPool {
    shared: Arc<Shared>,
}

impl PrefetchPool {
    fn new(
        options: Option<ParseOptions>,
        compare_paths_options: &ComparePathsOptions,
        reads_build_info: bool,
    ) -> Self {
        PrefetchPool {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue {
                    configs: VecDeque::new(),
                    reads: VecDeque::new(),
                    reads_by_config: FxHashMap::default(),
                    configs_closed: false,
                    closed: false,
                    reads_final: false,
                    threads: 0,
                    busy: 0,
                }),
                ready: Condvar::new(),
                slots: Mutex::new(FxHashMap::default()),
                options,
                compare_paths_options: compare_paths_options.clone(),
                reads_build_info,
                max_threads: MAX_PREFETCH_THREADS.min(crate::program::available_cores()),
            }),
        }
    }

    /// A pool that parses `configs` and the configs they reference, and
    /// reads the build info file of each when `reads_build_info`. None
    /// when no thread starts.
    pub fn start(
        compiler_options: CompilerOptions,
        command_line_raw: Option<IndexMap<String, CompilerOptionsValue>>,
        compare_paths_options: &ComparePathsOptions,
        reads_build_info: bool,
        configs: &[String],
    ) -> Option<Self> {
        let options = ParseOptions {
            compiler_options,
            command_line_raw,
        };
        let pool = PrefetchPool::new(Some(options), compare_paths_options, reads_build_info);
        queue_configs(&pool.shared, configs);
        let runs = lock(&pool.shared.queue).threads > 0;
        runs.then_some(pool)
    }

    /// A pool that only reads build info files (`finish_reads`).
    pub fn start_reads(compare_paths_options: &ComparePathsOptions) -> Self {
        PrefetchPool::new(None, compare_paths_options, false)
    }

    /// Queues the parse of each config in `configs` that was not queued
    /// before.
    pub fn queue(&self, configs: &[String]) {
        queue_configs(&self.shared, configs);
    }

    /// Queues the build info read of the config at `path` that the
    /// orchestrator parsed (`config`), when the pool reads build info files
    /// and no read is queued for it.
    pub fn queue_read(&self, path: &Path, config: &ParsedCommandLine) {
        if !self.shared.reads_build_info
            || lock(&self.shared.queue).reads_by_config.contains_key(path)
        {
            return;
        }
        let Some(read) = BuildInfoRead::of(config) else {
            return;
        };
        let spawn = {
            let mut queue = lock(&self.shared.queue);
            let Some(job) = add_read(&mut queue, path, read, None) else {
                return;
            };
            queue.reads.push_back(job);
            threads_to_start(&self.shared, &mut queue)
        };
        self.shared.ready.notify_one();
        spawn_threads(&self.shared, spawn);
    }

    /// No config is queued any more: the graph is made.
    pub fn close_configs(&self) {
        let mut queue = lock(&self.shared.queue);
        queue.configs_closed = true;
        queue.configs.clear();
    }

    /// The reads of the checks of a build: `reads`, in build order, each
    /// with the path of its task's config. A read that a thread queued for
    /// the same config with equal inputs is kept, and a new one is queued
    /// for each other read. The other queued reads are dropped. The kept
    /// reads that no thread has started run in build order. No job is
    /// queued after this, and each thread ends when the queue is empty.
    /// Gives each read's slot by file name, or None when no thread runs.
    pub(crate) fn finish_reads(
        &self,
        reads: Vec<(Path, BuildInfoRead)>,
    ) -> Option<FxHashMap<String, Arc<BuildInfoSlot>>> {
        let mut slots = FxHashMap::default();
        let spawn = {
            let mut queue = lock(&self.shared.queue);
            let not_started: FxHashSet<*const BuildInfoSlot> = queue
                .reads
                .drain(..)
                .map(|job| Arc::as_ptr(&job.slot))
                .collect();
            let queued = std::mem::take(&mut queue.reads_by_config);
            for (path, read) in reads {
                let job = match queued.get(&path) {
                    Some(job) if *job.read == read => {
                        if not_started.contains(&Arc::as_ptr(&job.slot)) {
                            queue.reads.push_back(job.clone());
                        }
                        job.clone()
                    }
                    _ => {
                        let job = ReadJob {
                            read: Arc::new(read),
                            slot: Arc::default(),
                            key: None,
                        };
                        queue.reads.push_back(job.clone());
                        job
                    }
                };
                slots.insert(job.read.name.clone(), job.slot);
            }
            queue.configs.clear();
            queue.configs_closed = true;
            queue.closed = true;
            queue.reads_final = true;
            threads_to_start(&self.shared, &mut queue)
        };
        self.shared.ready.notify_all();
        spawn_threads(&self.shared, spawn);
        let runs = lock(&self.shared.queue).threads > 0;
        runs.then_some(slots)
    }

    /// The key of the build info file `name` (`PathKeys::build_info_key`)
    /// that a thread found for the config at `path`, when it found one for
    /// that name.
    pub(crate) fn build_info_key(&self, path: &Path, name: &str) -> Option<Option<String>> {
        let queue = lock(&self.shared.queue);
        let job = queue.reads_by_config.get(path)?;
        if job.read.name != name {
            return None;
        }
        job.key.clone()
    }

    /// The match of a thread for the config at `path`, when its inputs
    /// equal those of the orchestrator's match (`inputs`). Waits for the
    /// thread that parses the config. A config that no thread has started
    /// becomes the orchestrator's: None, and no thread starts it. When the
    /// thread's work after the parse panicked, panics with its payload.
    fn take(&self, path: &Path, inputs: &MatchInputs) -> Option<MatchedFileNames> {
        let slot = lock(&self.shared.slots).get(path).cloned()?;
        let mut state = lock(&slot.state);
        loop {
            match std::mem::replace(&mut *state, SlotState::Taken) {
                SlotState::Queued => {
                    drop(state);
                    lock(&self.shared.queue)
                        .configs
                        .retain(|(_, queued, _)| queued != path);
                    return None;
                }
                SlotState::Taken => return None,
                SlotState::Running => {
                    *state = SlotState::Running;
                    state = slot
                        .done
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                SlotState::Done(matched) => {
                    return matched.filter(|matched| matched.inputs == *inputs);
                }
                SlotState::Panicked(payload) => {
                    drop(state);
                    std::panic::resume_unwind(payload);
                }
            }
        }
    }

    /// Go `getFileNamesFromConfigSpecs` for the orchestrator's parse of
    /// the config `config_file_name`: the file names that a thread
    /// matched, whose lookups go into `stats`, the cache of `fs`; or else
    /// its own match on `fs`.
    #[allow(clippy::too_many_arguments)]
    pub fn get_file_names_from_config_specs(
        &self,
        config_file_name: &str,
        config_file_specs: &ConfigFileSpecs,
        base_path: &str,
        options: Option<&CompilerOptions>,
        extra_extensions: &[String],
        fs: &dyn Fs,
        stats: &BuildStatCache,
    ) -> (Vec<String>, i32) {
        let path = to_path(
            config_file_name,
            &self.shared.compare_paths_options.current_directory,
            self.shared
                .compare_paths_options
                .use_case_sensitive_file_names,
        );
        if let Some(inputs) =
            MatchInputs::of(config_file_specs, base_path, options, extra_extensions)
            && let Some(matched) = self.take(&path, &inputs)
        {
            stats.add(&matched.lookups);
            return (matched.file_names, matched.literal_file_names_len);
        }
        get_file_names_from_config_specs(
            config_file_specs,
            base_path,
            options,
            fs,
            extra_extensions,
        )
    }
}

impl Drop for PrefetchPool {
    fn drop(&mut self) {
        let mut queue = lock(&self.shared.queue);
        queue.configs_closed = true;
        queue.closed = true;
        queue.configs.clear();
        if !queue.reads_final {
            queue.reads.clear();
        }
        drop(queue);
        self.shared.ready.notify_all();
    }
}

/// The threads to start for the queued jobs that no thread is free for,
/// counted as started.
fn threads_to_start(shared: &Shared, queue: &mut Queue) -> usize {
    let free = queue.threads - queue.busy;
    let jobs = queue.configs.len() + queue.reads.len();
    let spawn = jobs
        .saturating_sub(free)
        .min(shared.max_threads.saturating_sub(queue.threads));
    queue.threads += spawn;
    spawn
}

/// Starts `count` threads that `threads_to_start` counted: one here, which
/// starts the others, half of them each on two threads, before its first
/// job. A thread that cannot start leaves its jobs to the others.
fn spawn_threads(shared: &Arc<Shared>, count: usize) {
    if count == 0 {
        return;
    }
    // wasm32-wasip1 has no threads, so no thread can start. Saying so
    // leaves the threads out of the wasm module.
    if cfg!(target_family = "wasm") {
        lock(&shared.queue).threads -= count;
        return;
    }
    let thread_shared = shared.clone();
    // A config parse and the JSON parse of a build info file are recursive
    // (nested JSON values, `extends` chains), so the thread gets the Go
    // stack size, as a parse worker does.
    let spawned = std::thread::Builder::new()
        .name("goport-prefetch".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(move || {
            let others = count - 1;
            spawn_threads(&thread_shared, others - others / 2);
            spawn_threads(&thread_shared, others / 2);
            run_thread(&thread_shared);
        });
    if spawned.is_err() {
        lock(&shared.queue).threads -= count;
    }
}

/// Queues the parse of each config in `configs` that was not queued
/// before.
fn queue_configs(shared: &Arc<Shared>, configs: &[String]) {
    let mut added = 0;
    let spawn = {
        let mut slots = lock(&shared.slots);
        let mut queue = lock(&shared.queue);
        if queue.configs_closed {
            return;
        }
        for config in configs {
            let path = to_path(
                config,
                &shared.compare_paths_options.current_directory,
                shared.compare_paths_options.use_case_sensitive_file_names,
            );
            if slots.contains_key(&path) {
                continue;
            }
            let slot = Arc::new(Slot {
                state: Mutex::new(SlotState::Queued),
                done: Condvar::new(),
            });
            slots.insert(path.clone(), slot.clone());
            queue.configs.push_back((config.clone(), path, slot));
            added += 1;
        }
        threads_to_start(shared, &mut queue)
    };
    for _ in 0..added {
        shared.ready.notify_one();
    }
    spawn_threads(shared, spawn);
}

/// Adds `read` for the config at `path` to the reads (`reads_by_config`),
/// unless a read is there for it or no job is queued any more. Gives the
/// added read, which the caller runs or queues. `key` as in `ReadJob`.
fn add_read(
    queue: &mut Queue,
    path: &Path,
    read: BuildInfoRead,
    key: Option<Option<String>>,
) -> Option<ReadJob> {
    if queue.closed || queue.reads_by_config.contains_key(path) {
        return None;
    }
    let job = ReadJob {
        read: Arc::new(read),
        slot: Arc::default(),
        key,
    };
    queue.reads_by_config.insert(path.clone(), job.clone());
    Some(job)
}

enum Job {
    Config(String, Path, Arc<Slot>),
    Read(ReadJob),
}

/// A pool thread: runs queued jobs, configs first, until the pool closes
/// and the queue is empty.
fn run_thread(shared: &Arc<Shared>) {
    // Go: sys.FS() is bundled.WrapFS(osvfs.FS()), and the build host
    // caches it (`cachedvfs.From`). Made at the first config.
    let mut config_host: Option<(ThreadConfigHost, TscExtendedConfigCache)> = None;
    let mut thread = ThreadEnd {
        shared,
        busy: false,
    };
    loop {
        let job = {
            let mut queue = lock(&shared.queue);
            if thread.busy {
                queue.busy -= 1;
                thread.busy = false;
            }
            loop {
                let job = match queue.configs.pop_front() {
                    Some((config, path, slot)) => Some(Job::Config(config, path, slot)),
                    None => queue.reads.pop_front().map(Job::Read),
                };
                if let Some(job) = job {
                    queue.busy += 1;
                    break job;
                }
                if queue.closed {
                    return;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        thread.busy = true;
        match job {
            Job::Config(config, path, slot) => {
                let Some(options) = &shared.options else {
                    continue;
                };
                let (host, extended_config_cache) = config_host.get_or_insert_with(|| {
                    (
                        ThreadConfigHost {
                            fs: cachedvfs_from(crate::frontend::bundled::wrap_fs(
                                crate::frontend::vfs::osvfs_fs(),
                            )),
                            current_directory: shared
                                .compare_paths_options
                                .current_directory
                                .clone(),
                            matched: RefCell::new(None),
                        },
                        TscExtendedConfigCache::default(),
                    )
                });
                parse_config(
                    shared,
                    options,
                    host,
                    extended_config_cache,
                    config,
                    path,
                    &slot,
                );
            }
            Job::Read(job) => read_build_info(shared, &job),
        }
    }
}

/// A pool thread's count in the queue. A panic outside the `catch_unwind`
/// of a job (the config host of `run_thread`) ends the thread: it then
/// counts as ended (`Queue::threads`), and its job as done (`Queue::busy`),
/// so `threads_to_start` starts threads in its place. Its config slot stays
/// `Queued`, and the orchestrator parses that config (`PrefetchPool::take`).
struct ThreadEnd<'a> {
    shared: &'a Shared,
    /// The thread runs a job, which `Queue::busy` counts.
    busy: bool,
}

impl Drop for ThreadEnd<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut queue = lock(&self.shared.queue);
            queue.threads -= 1;
            if self.busy {
                queue.busy -= 1;
            }
        }
    }
}

/// Parses `config` for its slot. Before the orchestrator can take the
/// slot, queues its references, and its build info read with the key of
/// the file. It starts no thread for the read: this thread is free for it
/// after the parse. When that work panics, the slot keeps the payload, and
/// the orchestrator panics with it (`take`), as when it found the key on
/// its own thread. So the slot does not stay `Running`, and the
/// orchestrator does not wait for it forever.
fn parse_config(
    shared: &Arc<Shared>,
    options: &ParseOptions,
    host: &ThreadConfigHost,
    extended_config_cache: &TscExtendedConfigCache,
    config: String,
    path: Path,
    slot: &Slot,
) {
    {
        let mut state = lock(&slot.state);
        if !matches!(*state, SlotState::Queued) {
            return;
        }
        *state = SlotState::Running;
    }
    // A parse that panics is left to the orchestrator, which panics on it
    // too.
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        host.matched.borrow_mut().take();
        let (parsed, _) = get_parsed_command_line_of_config_file_path(
            &config,
            path.clone(),
            Some(&options.compiler_options),
            options.command_line_raw.as_ref(),
            host,
            Some(extended_config_cache),
        );
        parsed.map(|parsed| {
            let read = shared
                .reads_build_info
                .then(|| BuildInfoRead::of(&parsed))
                .flatten();
            (parsed.resolved_project_reference_paths().to_vec(), read)
        })
    }));
    let matched = match &parsed {
        Ok(_) => host
            .matched
            .borrow_mut()
            .take()
            .filter(|(name, _)| *name == config)
            .map(|(_, matched)| matched),
        Err(_) => None,
    };
    let queued = match parsed {
        Ok(Some((references, read))) => {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                tests::panic_after_parse(&config);
                queue_configs(shared, &references);
                if let Some(read) = read {
                    let fs = crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs());
                    let key = PathKeys::new(&fs, &shared.compare_paths_options)
                        .build_info_key(&read.name);
                    let mut queue = lock(&shared.queue);
                    if let Some(job) = add_read(&mut queue, &path, read, Some(key)) {
                        queue.reads.push_back(job);
                        drop(queue);
                        shared.ready.notify_one();
                    }
                }
            }))
        }
        _ => Ok(()),
    };
    *lock(&slot.state) = match queued {
        Ok(()) => SlotState::Done(matched),
        Err(payload) => SlotState::Panicked(payload),
    };
    slot.done.notify_all();
}

/// Reads and parses the build info file of `job` on this thread's OS file
/// system, as the host does (`ReadBuildInfo`: read the file, then
/// `parse_build_info`). Then makes the check parts (`StatusPrefetch`) with
/// the mtimes of the task's TypeScript sources (`read_m_times`), unless
/// the build info shows that the check returns before it reads them
/// (errors, pending emit: `StatusCheckOptions::reads_input_times`), as Go
/// reads no input mtime there.
fn read_build_info(shared: &Shared, job: &ReadJob) {
    let read = &job.read;
    // A read that panics is left to the task, which panics on it too.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let fs = crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs());
        let (data, ok) = fs.read_file(&read.name);
        let build_info = if ok { parse_build_info(&data) } else { None };
        let status = build_info
            .as_ref()
            .filter(|build_info| read.check.reads_input_times(build_info))
            .map(|build_info| {
                let mut status = StatusPrefetch::new(
                    build_info,
                    &read.name,
                    &read.input_files,
                    &shared.compare_paths_options,
                );
                status.read_m_times(&*fs, is_wrapped_os_fs(&fs), &read.input_files);
                status
            });
        (build_info, status)
    }));
    *lock(&job.slot.result) = Some(result.ok());
    job.slot.done.notify_all();
}

/// The parse config host of a pool thread: records the match of the
/// config it parses.
struct ThreadConfigHost {
    fs: Rc<dyn Fs>,
    current_directory: String,
    /// The config name and match of the last `get_file_names_from_config_specs`.
    matched: RefCell<Option<(String, MatchedFileNames)>>,
}

impl ParseConfigHost for ThreadConfigHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }

    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }

    fn get_file_names_from_config_specs(
        &self,
        config_file_name: &str,
        config_file_specs: &ConfigFileSpecs,
        base_path: &str,
        options: Option<&CompilerOptions>,
        extra_extensions: &[String],
    ) -> (Vec<String>, i32) {
        let recording = RecordingFs {
            fs: &*self.fs,
            lookups: StatCache::default(),
        };
        let (file_names, literal_file_names_len) = get_file_names_from_config_specs(
            config_file_specs,
            base_path,
            options,
            &recording,
            extra_extensions,
        );
        *self.matched.borrow_mut() =
            MatchInputs::of(config_file_specs, base_path, options, extra_extensions).map(
                |inputs| {
                    (
                        config_file_name.to_string(),
                        MatchedFileNames {
                            inputs,
                            file_names: file_names.clone(),
                            literal_file_names_len,
                            lookups: recording.lookups,
                        },
                    )
                },
            );
        (file_names, literal_file_names_len)
    }
}
/// The file system of a pool thread's match: `fs`, with each cached
/// lookup (Go `cachedvfs`: all but `Stat`) recorded in `lookups`.
struct RecordingFs<'a> {
    fs: &'a dyn Fs,
    lookups: StatCache,
}

impl Fs for RecordingFs<'_> {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.lookups.file_exists(path, || self.fs.file_exists(path))
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
        self.lookups
            .directory_exists(path, || self.fs.directory_exists(path))
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.lookups
            .entries(path, || self.fs.get_accessible_entries(path))
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.fs.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        self.lookups.realpath(path, || self.fs.realpath(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configs whose work after the parse panics (`panic_after_parse`).
    static PANIC_AFTER_PARSE: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Test-only panic hook of `parse_config`: panics after the parse of a
    /// config in `PANIC_AFTER_PARSE`, where the key and queue calls can
    /// panic.
    pub(super) fn panic_after_parse(config: &str) {
        if lock(&PANIC_AFTER_PARSE).iter().any(|name| name == config) {
            panic!("test panic after the parse of {config}");
        }
    }

    // A panic in a thread's work after the parse (build info key, queued
    // references) reaches the orchestrator's take as a panic. Before, the
    // slot stayed Running and `tsc -b` waited for it forever.
    #[test]
    fn panic_after_parse_reaches_take() {
        let dir =
            std::env::temp_dir().join(format!("ts_goport_prefetch_panic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), "export {};").unwrap();
        std::fs::write(dir.join("tsconfig.json"), r#"{"files": ["a.ts"]}"#).unwrap();
        // The compiler takes normalized paths: a Windows temp dir has backslashes.
        let normalized =
            |p: &std::path::Path| crate::frontend::tspath::normalize_slashes(p.to_str().unwrap());
        let config = normalized(&dir.join("tsconfig.json"));
        lock(&PANIC_AFTER_PARSE).push(config.clone());
        let compare_paths_options = ComparePathsOptions {
            use_case_sensitive_file_names: true,
            current_directory: normalized(&dir),
        };
        let path = to_path(&config, &compare_paths_options.current_directory, true);
        let pool = PrefetchPool::start(
            CompilerOptions::default(),
            None,
            &compare_paths_options,
            false,
            std::slice::from_ref(&config),
        )
        .expect("a prefetch thread starts");
        // The orchestrator parses a config that no thread has started, so
        // the take waits until a thread has started it.
        let slot = lock(&pool.shared.slots).get(&path).cloned().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while matches!(*lock(&slot.state), SlotState::Queued) {
            assert!(
                std::time::Instant::now() < deadline,
                "no thread started the config"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let inputs = MatchInputs {
            base_path: String::new(),
            files: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            extensions: Vec::new(),
            extensions_with_json: Vec::new(),
        };
        // A take that waits forever fails the test after the timeout.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let taken = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.take(&path, &inputs).is_some()
            }));
            let _ = sender.send(taken.map_err(|payload| {
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default()
            }));
        });
        let taken = receiver.recv_timeout(std::time::Duration::from_secs(60));
        lock(&PANIC_AFTER_PARSE).retain(|name| *name != config);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            taken,
            Ok(Err(format!("test panic after the parse of {config}")))
        );
    }

    // A pool thread that a panic outside the catch_unwind of a job ends
    // (the config host of `run_thread`) counts as ended, and its job as
    // done. Before, it stayed counted as a busy thread, and a pool whose
    // threads all ended so started no thread for the build info reads,
    // whose checks then waited forever.
    #[test]
    fn thread_ended_by_a_panic_is_not_counted() {
        let pool = PrefetchPool::new(None, &ComparePathsOptions::default(), false);
        {
            let mut queue = lock(&pool.shared.queue);
            queue.threads = 1;
            queue.busy = 1;
        }
        let shared = pool.shared.clone();
        let ended = std::thread::spawn(move || {
            let _thread = ThreadEnd {
                shared: &shared,
                busy: true,
            };
            panic!("test panic in the config host");
        })
        .join();
        assert!(ended.is_err());
        let queue = lock(&pool.shared.queue);
        assert_eq!((queue.threads, queue.busy), (0, 0));
    }
}
