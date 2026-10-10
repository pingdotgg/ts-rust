//! Port of the emit parts of Go `compiler/program.go` (`Emit`,
//! `CombineEmitResults`, `HandleNoEmitOptions`, the emit option and result
//! types). The emit host (`program::EmitHost`) and the output paths
//! (`program::get_output_paths_for_source_file`) are in `program.rs`, where
//! the declaration diagnostics use them too.

use crate::prelude::*;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::emitter::{DeclarationPrint, EmitOnly, Emitter, JsPrint, js_emit_needs_checker};
use crate::frontend::outputpaths::{ForceEmitPaths, OutputPaths};
use crate::frontend::tspath::{Path, has_extension, path_is_relative, to_path};
use crate::printer::emit_context::{ParseEmitNodes, PrintTables};
use crate::sourcemap::generator::RawSourceMap;

// Go: compiler/program.go:1860 WriteFileData
#[derive(Clone, Debug, Default)]
pub struct WriteFileData {
    pub source_map_url_pos: i32,
    // PORT: Go uses `any` to avoid an import cycle. It is `Arc` because the
    // write callback is thread safe. Only the build info write sets it
    // (incremental `emit_build_info`).
    pub build_info: Option<Arc<crate::execute::incremental::BuildInfo>>,
    pub diagnostics: Vec<Diagnostic>,
    pub skipped_dts_write: bool,
    // #4699
    pub source_file: Node,
}

// Go: compiler/program.go:1868 WriteFile
// PORT: Go `error` is `Result<(), String>`. Emit runs on the checker
// threads and the emit pool, so the callback is shared and thread safe. The
// callback may set `skipped_dts_write`.
pub type WriteFile =
    Arc<dyn Fn(&str, &str, &mut WriteFileData) -> Result<(), String> + Send + Sync>;

// Go: compiler/program.go:1870 EmitOptions
// PORT: Go `TargetSourceFiles` nil is `None`. A Go non-nil empty slice is
// `Some` of an empty list, which emits no file (#4699).
#[derive(Clone, Default)]
pub struct EmitOptions {
    /// Source files to emit. If `None`, emits all files
    pub target_source_files: Option<Vec<Node>>,
    pub emit_only: EmitOnly,
    // #4699
    pub force_emit: bool,
    pub write_file: Option<WriteFile>,
}

/// The `EmitOptions` fields that each file's emitter gets (Go `emitter`
/// `emitOnly`, `forceEmit` and `writeFile`), and the force flags that Go
/// `Program.Emit` computes from them.
#[derive(Clone)]
struct EmitterOptions {
    emit_only: EmitOnly,
    force_emit: bool,
    write_file: Option<WriteFile>,
}

impl EmitterOptions {
    fn of(options: &EmitOptions) -> Self {
        Self {
            emit_only: options.emit_only,
            force_emit: options.force_emit,
            write_file: options.write_file.clone(),
        }
    }

    /// Go `forceDtsEmit` in `Program.Emit` (#4699, #4849).
    fn force_dts_emit(&self) -> bool {
        self.emit_only == EmitOnly::BuilderSignature
            || self.force_emit && self.emit_only == EmitOnly::Dts
    }

    /// Go `forceJsEmit` in `Program.Emit` (#4699).
    fn force_js_emit(&self) -> bool {
        self.force_emit && self.emit_only == EmitOnly::Js
    }

    /// Go `outputpaths.ForceEmitPaths` in `Program.Emit` (#4699).
    fn force_emit_paths(&self) -> ForceEmitPaths {
        ForceEmitPaths {
            dts: self.force_dts_emit(),
            js: self.force_js_emit(),
            declaration_map: self.force_emit && self.emit_only == EmitOnly::Dts,
        }
    }
}

// Go: compiler/program.go:1877 EmitResult
#[derive(Clone, Debug, Default)]
pub struct EmitResult {
    pub emit_skipped: bool,
    /// Contains declaration emit diagnostics
    pub diagnostics: Vec<Diagnostic>,
    /// Array of files the compiler wrote to disk
    pub emitted_files: Vec<String>,
    /// Array of sourceMapData if compiler emitted sourcemaps
    pub source_maps: Vec<SourceMapEmitResult>,
}

// Go: compiler/program.go:1884 SourceMapEmitResult
#[derive(Clone, Debug, Default)]
pub struct SourceMapEmitResult {
    /// Input source file (which one can use on program to get the file), 1:1 mapping with the sourceMap.sources list
    pub input_source_file_names: Vec<String>,
    pub source_map: RawSourceMap,
    pub generated_file: String,
}

// Go: compiler/program.go:1890 Program.Emit
// PORT: Go queues one emit per file on a work group and takes a writer from
// a pool. Each file's emit runs on the thread of its checker (the emit
// resolver reaches that checker there), and the results combine in file
// order. Each emit makes its own text writer. When the emit pool is on
// (`program::emit_pool_enabled`), the JS part of a file whose transforms
// make no checker call runs on the pool instead, and the prints of the
// parts that run on a checker go to its twin (`start_emit_files_with_pool`).
// Go `ctx.Err()` checks are not ported: the port has no context here.
pub fn emit(options: EmitOptions) -> EmitResult {
    emit_with(options, |emit_file| emit_file())
}

/// `emit` with `wrap` around each file's emit, on the file's checker
/// thread. `goport` uses it to guard each file on its own. When a file's
/// emit is split, `wrap` is around each part: a panic in one part does not
/// stop the other.
pub fn emit_with(
    options: EmitOptions,
    wrap: fn(&dyn Fn() -> EmitResult) -> EmitResult,
) -> EmitResult {
    // collect results from emit, preserving input order
    combine_emit_results(emit_results_with(options, wrap))
}

/// `emit` without the final `combine_emit_results`: one result per file, in
/// file order, or the one result of `handle_no_emit_options`. The API emit
/// uses it to add its late write failures to the result of their file
/// (`add_write_failures`).
pub fn emit_file_results(options: EmitOptions) -> Vec<EmitResult> {
    emit_results_with(options, |emit_file| emit_file())
}

/// `emit_with` before it combines the results (`emit_file_results`).
fn emit_results_with(
    options: EmitOptions,
    wrap: fn(&dyn Fn() -> EmitResult) -> EmitResult,
) -> Vec<EmitResult> {
    let _trace = trace_emit();
    if !options.force_emit && options.emit_only != EmitOnly::BuilderSignature {
        // #4407: Go `HandleNoEmitOptions(ctx, p, options.TargetSourceFiles, nil)`.
        if let Some(result) = handle_no_emit_options(options.target_source_files.as_deref()) {
            return vec![result];
        }
    }

    let target = EmitterOptions::of(&options);
    let source_files = get_source_files_to_emit(
        options.target_source_files.as_deref(),
        target.force_dts_emit(),
        target.force_js_emit(),
    );
    let pooled = start_emit_files_with_pool(&source_files, |_| target.clone(), wrap)
        .map(PendingPoolEmit::wait);
    match pooled {
        Some(results) => results,
        None => run_emit_jobs(source_files, move |source_file| {
            wrap(&|| emit_source_file(source_file, &target))
        }),
    }
}

/// PORT: not in Go. The API emit (`handle_emit`) writes the outputs after
/// the emit, on its own thread, because its file system is not shared with
/// the emit threads. In Go the write runs in the emitter, and its TS5033
/// goes into that file's diagnostics, which `emitter.emit` sorts. This adds
/// each failure `(output file name, diagnostic)` to the per-file result
/// (`emit_file_results`) that emitted the file, removes the file from its
/// emitted files and sorts its diagnostics again. A failure that no result
/// emitted goes last.
pub fn add_write_failures(results: &mut Vec<EmitResult>, failures: Vec<(String, Diagnostic)>) {
    let mut touched = vec![false; results.len()];
    let mut unmatched = Vec::new();
    for (file_name, diagnostic) in failures {
        let index = results
            .iter()
            .position(|result| result.emitted_files.contains(&file_name));
        match index {
            Some(index) => {
                let result = &mut results[index];
                result.emitted_files.retain(|emitted| *emitted != file_name);
                result.diagnostics.push(diagnostic);
                touched[index] = true;
            }
            None => unmatched.push(diagnostic),
        }
    }
    for (result, touched) in results.iter_mut().zip(touched) {
        if touched {
            let mut diagnostics = DiagnosticsCollection::default();
            for diagnostic in std::mem::take(&mut result.diagnostics) {
                diagnostics.add(diagnostic);
            }
            result.diagnostics = diagnostics.get_diagnostics();
        }
    }
    if !unmatched.is_empty() {
        results.push(EmitResult {
            diagnostics: unmatched,
            ..EmitResult::default()
        });
    }
}

/// `emit` for many targets at once, one result per target in input order.
pub fn emit_batch(targets: Vec<EmitOptions>) -> Vec<EmitResult> {
    start_emit_batch(targets).wait()
}

/// `emit_batch` without the wait (`start_emit_batch_with`).
pub fn start_emit_batch(targets: Vec<EmitOptions>) -> PendingEmitBatch {
    start_emit_batch_with(targets, |emit_file| emit_file())
}

/// `emit_with` for many targets at once, one result per target in input
/// order. Each result is the same as `emit_with` of that target alone.
///
/// Go `emitFilesIncremental` calls `Program.Emit` for each pending file
/// inside a work group, so the files emit in parallel. `emit_files_incremental`
/// builds one `EmitOptions` per pending file (one target file each, from
/// `get_emit_options`) and calls this once instead of `emit` per file.
/// Each checker thread runs its files in `targets` order, also with
/// `--singleThreaded`.
///
/// Each target must name one source file, and no file can be in the batch
/// twice. With `noEmitOnError` each target runs through `emit_with`, one at
/// a time, because the diagnostics check must come first. With `noEmit`
/// each target also runs through `emit_with` (#4407).
pub fn emit_batch_with(
    targets: Vec<EmitOptions>,
    wrap: fn(&dyn Fn() -> EmitResult) -> EmitResult,
) -> Vec<EmitResult> {
    start_emit_batch_with(targets, wrap).wait()
}

/// An `emit_batch_with` whose jobs are sent and not waited for yet
/// (`start_emit_batch_with`).
pub struct PendingEmitBatch(PendingBatch);

enum PendingBatch {
    /// `noEmitOnError`: every target emitted before the start returned, one
    /// result per target.
    Done(Vec<EmitResult>),
    /// The jobs of the files, and for each target whether it has a file.
    Sent {
        has_file: Vec<bool>,
        files: PendingFiles,
    },
}

/// The jobs of the files of a batch, one result per file in file order.
enum PendingFiles {
    Pool(PendingPoolEmit),
    Checkers(PendingCheckerJobs<EmitResult>),
}

impl PendingEmitBatch {
    /// Waits for every job and returns one result per target, in target
    /// order: the result of `emit_batch_with`.
    #[must_use]
    pub fn wait(self) -> Vec<EmitResult> {
        let (has_file, files) = match self.0 {
            PendingBatch::Done(results) => return results,
            PendingBatch::Sent { has_file, files } => (has_file, files),
        };
        let mut results = match files {
            PendingFiles::Pool(pool) => pool.wait(),
            PendingFiles::Checkers(jobs) => jobs.wait(),
        }
        .into_iter();

        // `combine_emit_results` of one result is that result.
        has_file
            .into_iter()
            .map(|has_file| {
                if has_file {
                    results.next().expect("one emit result per batch file")
                } else {
                    combine_emit_results(Vec::new())
                }
            })
            .collect()
    }
}

/// The first half of `emit_batch_with`: sends the jobs of every target to
/// the emit pool and the checker threads, and returns without waiting.
/// Each checker thread runs them after the jobs sent to it before (the
/// early emit: `execute::incremental::Program::start_emit`).
/// With `noEmitOnError` or `noEmit` it emits every target before it
/// returns.
pub fn start_emit_batch_with(
    targets: Vec<EmitOptions>,
    wrap: fn(&dyn Fn() -> EmitResult) -> EmitResult,
) -> PendingEmitBatch {
    // #4407: with `noEmit`, `handle_no_emit_options` returns a result for
    // a target too.
    if options().no_emit_on_error.is_true() || options().no_emit.is_true() {
        return PendingEmitBatch(PendingBatch::Done(
            targets
                .into_iter()
                .map(|target| emit_with(target, wrap))
                .collect(),
        ));
    }

    // Without `noEmit` and `noEmitOnError`, `handle_no_emit_options`
    // returns None, so `emit_with` of one target is only the emit of its 0
    // or 1 file.
    let mut files = Vec::new();
    let mut has_file = Vec::with_capacity(targets.len());
    let mut file_targets: FxHashMap<Node, EmitterOptions> = FxHashMap::default();
    for target in targets {
        let target_files = target.target_source_files.as_deref();
        debug_assert!(
            target_files.is_some_and(|files| files.len() == 1),
            "emit_batch target without one file"
        );
        let emitter_options = EmitterOptions::of(&target);
        let file = get_source_files_to_emit(
            target_files,
            emitter_options.force_dts_emit(),
            emitter_options.force_js_emit(),
        )
        .first()
        .copied();
        has_file.push(file.is_some());
        match file {
            Some(file) => {
                let previous = file_targets.insert(file, emitter_options);
                debug_assert!(previous.is_none(), "file is in the emit batch twice");
                files.push(file);
            }
            // Go `Program.Emit` still traces a target that emits no file.
            None => drop(trace_emit()),
        }
    }

    // PORT: the batch runs in `targets` order on each checker thread, also with
    // `--singleThreaded`. Go `emitFilesIncremental` queues the files in SyncMap
    // Range order, which is random, and its single-threaded work group runs them
    // last-queued-first. The port keeps the order of the one-file loop that the
    // batch replaces, so each checker does the same work in the same order.
    // With the pool, no trace is written (`emit_pool_enabled`), so there is
    // no trace event per target.
    let pooled = start_emit_files_with_pool(&files, |file| file_targets[&file].clone(), wrap);
    let jobs = match pooled {
        Some(pool) => PendingFiles::Pool(pool),
        None => {
            // The closure gets only the file, so it finds the file's target here.
            let file_targets = Arc::new(file_targets);
            PendingFiles::Checkers(send_on_checker_threads_for_files(
                &files,
                move |source_file| {
                    // One Go `Program.Emit` trace event per target, on the emit thread.
                    let _trace = trace_emit();
                    let target = &file_targets[&source_file];
                    wrap(&|| emit_source_file(source_file, target))
                },
            ))
        }
    };
    PendingEmitBatch(PendingBatch::Sent {
        has_file,
        files: jobs,
    })
}

/// PORT: not in Go (perf). True when `tsc -p` or `tsc -b` with an
/// incremental program may send the emit of the current program right
/// behind its check (`execute::incremental::Program::start_emit`): each
/// checker then emits when its own check ends, and the emit pool runs
/// during the check. Go waits for the whole check before it emits. Each
/// checker still
/// gets the same jobs in the same order, and all state that emit writes is
/// per thread, per checker, per emit, loading thread only or a pure cache,
/// except the file system: a check can probe files, and the emit writes
/// files and makes directories. Go never lets a check see the outputs of
/// this emit. These rules keep every check-time probe away from them
/// (perf10 `design.md` section 4):
///
/// - F1: the module resolution is not node16 or nodenext, or no checked
///   file has a relative module name without an extension (imports, module
///   augmentations, synthetic imports). This removes every import extension
///   probe (`Checker::get_suggested_import_extension`, TS2834 and TS2835).
/// - F2: no program file is inside `outDir` or `declarationDir`.
/// - F3: neither has a `node_modules` path segment. F2 and F3 keep the
///   probes for module specifiers in type text (`modulespecifiers::host`
///   `get_package_json_info_for_directory`,
///   `modulespecifiers::util::try_get_any_file_from_path`) away from the
///   outputs and the directories that the emit makes.
/// - F4: `preserveSymlinks` is off, so the program file paths of F2 are the
///   paths that the probes reach.
/// - No `outFile`.
///
/// A new file system read on a checker thread must be added to this list
/// and to the rules. The rules are in two parts: the options
/// (`early_emit_options_allow`, which also covers F4 and `outFile`) and the
/// program files (`check_cannot_see_outputs`, F1 to F3). Loading thread
/// only.
#[must_use]
pub fn emit_can_start_with_check() -> bool {
    early_emit_options_allow() && check_cannot_see_outputs()
}

/// The option part of `emit_can_start_with_check`. False with `noEmit`,
/// `noEmitOnError` (the emit needs every diagnostic first),
/// `--singleThreaded`, a trace, `preserveSymlinks` (F4), `outFile`, and
/// `GOPORT_EARLY_EMIT=0`. Then `tsc -p` keeps Go's order exactly: it does
/// not start the check early either. `tsc -b` starts each check early in
/// any case. When these rules and `check_cannot_see_outputs` allow it, it
/// also starts the emit behind the check and keeps the writes until the
/// task finishes (`buffer_early_emit_writes`); else it emits in Go's order.
#[must_use]
pub fn early_emit_options_allow() -> bool {
    let options = options();
    early_emit_enabled()
        && !options.no_emit.is_true()
        && !options.no_emit_on_error.is_true()
        && !single_threaded()
        && crate::tracing::get().is_none()
        && !options.preserve_symlinks.is_true()
        && options.out_file.is_empty()
}

/// The program file part of `emit_can_start_with_check`: F1, F2 and F3.
/// It reads every program file, so `start_emit` runs it after
/// `start_check` sent the check.
#[must_use]
pub fn check_cannot_see_outputs() -> bool {
    let options = options();
    let output_dirs: Vec<Path> = [&options.out_dir, &options.declaration_dir]
        .into_iter()
        .filter(|dir| !dir.is_empty())
        .map(|dir| {
            to_path(
                dir,
                get_current_directory(),
                use_case_sensitive_file_names(),
            )
        })
        .collect();
    // F3
    if output_dirs
        .iter()
        .any(|dir| dir.0.split('/').any(|part| part == "node_modules"))
    {
        return false;
    }
    // F2
    let files = source_files();
    if !output_dirs.is_empty()
        && files.iter().any(|&file| {
            let path = Path(source_file_info(file).path.clone());
            output_dirs.iter().any(|dir| dir.contains_path(&path))
        })
    {
        return false;
    }
    // F1
    let resolution = options.get_module_resolution_kind();
    if resolution != ModuleResolutionKind::NODE16 && resolution != ModuleResolutionKind::NODE_NEXT {
        return true;
    }
    !files.into_iter().any(|file| {
        if skip_type_checking(file, false) {
            return false;
        }
        let info = source_file_info(file);
        let synthetic = [
            get_import_helpers_import_specifier(&info.path),
            get_jsx_runtime_import_specifier(&info.path).1,
        ];
        info.imports
            .iter()
            .chain(&info.module_augmentations)
            .chain(synthetic.iter().filter(|name| name.is_some()))
            .any(|name| {
                let name = name.text();
                path_is_relative(name) && !has_extension(name)
            })
    })
}

/// False when `GOPORT_EARLY_EMIT` is `0`: the emit then waits for the whole
/// check, as in Go (`emit_can_start_with_check`). For tests and timing on
/// one binary. Read once.
fn early_emit_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("GOPORT_EARLY_EMIT").as_deref() != Ok("0"))
}

/// Go `tr.Push(tracing.PhaseEmit, "emit", nil, true)` at the start of
/// `Program.Emit`. The event ends when the returned value drops.
fn trace_emit() -> Option<crate::tracing::Pop> {
    crate::tracing::get().map(|tr| tr.push(crate::tracing::Phase::Emit, "emit", Vec::new(), true))
}

/// Runs `job(file)` for each file on the file's checker thread and returns
/// the results in file order. Each checker thread runs its jobs in the
/// order they are sent (FIFO).
fn run_emit_jobs(
    files: Vec<Node>,
    job: impl Fn(Node) -> EmitResult + Send + Sync + 'static,
) -> Vec<EmitResult> {
    // Go `core.singleThreadedWorkGroup` runs the queued emits
    // last-queued-first (core/workgroup.go:67). With one checker thread the
    // jobs run in the order they are sent, so send them reversed.
    let last_queued_first = single_threaded();
    let mut queued = files;
    if last_queued_first {
        queued.reverse();
    }
    let mut results = run_on_checker_threads_for_files(&queued, job);
    if last_queued_first {
        results.reverse();
    }
    results
}

/// The `wrap` of `emit_with` and `emit_batch_with`.
type Wrap = fn(&dyn Fn() -> EmitResult) -> EmitResult;

/// Where one file's emit runs when the emit pool is on (`file_emit`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileEmit {
    /// All of it on the file's checker thread, as with the pool off.
    OnChecker,
    /// The JS part on the emit pool, the d.ts part on the checker thread.
    Split,
    /// All of it on the emit pool: it has a JS part and no d.ts part.
    OnPool,
}

/// PORT: not in Go. Where the emit of `source_file` with `target` runs.
/// A JS part goes to the emit pool when its transforms make no checker call
/// (`js_emit_needs_checker`). The d.ts part always needs the checker.
/// A forced emit (#4699, the API) runs on the checker thread, so a split
/// emit never has `force_emit` and its parts get the paths of the whole
/// emit.
fn file_emit(source_file: Node, target: &EmitterOptions) -> FileEmit {
    if target.force_emit {
        return FileEmit::OnChecker;
    }
    let emit_only = target.emit_only;
    let options = options();
    // With `noEmit` the JS part only sets `emit_skipped`: not worth a job.
    // #4712: a content-mapped file has no JS output path
    // (`get_output_paths_for_file`).
    let has_js = matches!(emit_only, EmitOnly::All | EmitOnly::Js)
        && !options.emit_declaration_only.is_true()
        && !options.no_emit.is_true()
        && source_file_content_mapper(source_file).is_empty();
    if !has_js || js_emit_needs_checker(source_file) {
        return FileEmit::OnChecker;
    }
    // `get_output_paths_for_file` gives a d.ts path only with declarations
    // on, and never to a JSON file.
    let has_dts = emit_only == EmitOnly::All
        && options.get_emit_declarations()
        && !is_json_source_file(source_file);
    if has_dts {
        FileEmit::Split
    } else {
        FileEmit::OnPool
    }
}

/// The part of one file's emit that runs on its checker thread when the
/// emit pool is on (`start_emit_files_with_pool`).
struct CheckerPart {
    target: EmitterOptions,
    /// The JS part on the emit pool when the file's emit is split; this is
    /// then the d.ts part.
    js_part: Option<PoolJs>,
}

/// The JS part of a split file on the emit pool, as its d.ts part sees it.
struct PoolJs {
    job: EmitPoolJob<EmitResult>,
    /// ts#64649: what the JS transforms wrote on parse-tree nodes
    /// (`emit_source_file_on_pool`). The pool job sends it after its
    /// transforms, or drops the sender when it ran none (skipped, blocked) or
    /// panicked.
    emit_nodes: Receiver<ParseEmitNodes>,
}

impl PoolJs {
    /// Waits until the JS transforms ended, and returns what they wrote on
    /// parse-tree nodes, or None when they did not run.
    fn js_emit_nodes(&self) -> Option<ParseEmitNodes> {
        self.emit_nodes.recv().ok()
    }
}

/// PORT: not in Go. Starts `emit_with` and `emit_batch_with` of `files`
/// (each with its `target`: `emit_only`, `force_emit` and `write_file`) with the emit
/// pool, and returns without waiting. None, before any work, when the pool
/// is off (`emit_pool_enabled`), or when it would get no file and the
/// twins are off (`dts_twin_mode`): the caller then runs every file on its
/// checker thread as before.
///
/// First the JS part of each file that `file_emit` moves goes to the emit
/// pool, in file order. Then the rest goes to the checker threads exactly
/// as with the pool off: each checker thread gets its files in file order,
/// with the whole emit or the d.ts part, so each checker gets the same
/// checker calls in the same order. The JS part makes no checker call.
/// A d.ts part waits for the JS transforms of its file before its own
/// transforms (ts#64649, `PoolJs::js_emit_nodes`), and for its JS part
/// before it writes (`Emitter::js_part`), so the outputs of a file are
/// written in Go's order. The pool jobs never wait for a checker, and they
/// are sent first, so the wait always ends. With the twins on, a checker
/// thread runs only the transforms of its parts, and its twin prints and
/// writes them (`emit_declaration_part`, `emit_on_twin`).
fn start_emit_files_with_pool(
    files: &[Node],
    target: impl Fn(Node) -> EmitterOptions,
    wrap: Wrap,
) -> Option<PendingPoolEmit> {
    if files.is_empty() || !emit_pool_enabled() {
        return None;
    }
    // The rule reads the binder data. Sending a job binds too, but the rule
    // runs first.
    bind_all();
    let ways: Vec<FileEmit> = files
        .iter()
        .map(|&file| file_emit(file, &target(file)))
        .collect();
    let twin_mode = dts_twin_mode();
    if twin_mode == DtsTwinMode::Off && ways.iter().all(|&way| way == FileEmit::OnChecker) {
        return None;
    }

    let mut pool_jobs = Vec::new();
    let mut js_emit_nodes = Vec::new();
    for (&file, &way) in files.iter().zip(&ways) {
        if way == FileEmit::OnChecker {
            continue;
        }
        let mut target = target(file);
        let sender = (way == FileEmit::Split).then(|| {
            target.emit_only = EmitOnly::Js;
            let (sender, receiver) = sync_channel(1);
            js_emit_nodes.push(receiver);
            sender
        });
        pool_jobs.push(move || wrap(&|| emit_source_file_on_pool(file, &target, sender.as_ref())));
    }
    let mut pool_jobs = send_emit_pool_jobs(pool_jobs).into_iter();
    let mut js_emit_nodes = js_emit_nodes.into_iter();

    let mut checker_files = Vec::new();
    let mut checker_parts = FxHashMap::default();
    let mut on_pool = Vec::new();
    for (index, (&file, &way)) in files.iter().zip(&ways).enumerate() {
        if way == FileEmit::OnPool {
            on_pool.push((index, pool_jobs.next().expect("a pool job per pool file")));
            continue;
        }
        let js_part = (way == FileEmit::Split).then(|| PoolJs {
            job: pool_jobs.next().expect("a pool job per split file"),
            emit_nodes: js_emit_nodes
                .next()
                .expect("a JS emit node channel per split file"),
        });
        checker_files.push(file);
        checker_parts.insert(
            file,
            CheckerPart {
                target: target(file),
                js_part,
            },
        );
    }

    // The closure gets only the file, so it takes the file's part here.
    let checker_parts = Mutex::new(checker_parts);
    let checker = catch_unwind(AssertUnwindSafe(|| {
        send_on_checker_threads_for_files(&checker_files, move |source_file| {
            let part = checker_parts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&source_file)
                .expect("one checker part per file");
            match part.js_part {
                Some(js_part) => emit_declaration_part(source_file, part.target, js_part, wrap),
                None if twin_mode != DtsTwinMode::Off && twin_can_print(&part.target) => {
                    emit_on_twin(source_file, &part.target, wrap, twin_mode)
                }
                None => CheckerOutput::Done(wrap(&|| emit_source_file(source_file, &part.target))),
            }
        })
    }));
    Some(PendingPoolEmit {
        file_count: files.len(),
        checker,
        on_pool,
    })
}

/// What the job of one file on its checker thread returns when the emit
/// pool is on.
enum CheckerOutput {
    /// The file's emit result.
    Done(EmitResult),
    /// The twin of the checker prints the file's parts and merges them
    /// (`emit_declaration_part`, `emit_on_twin`).
    Twin(EmitPoolJob<EmitResult>),
}

/// The jobs that `start_emit_files_with_pool` sent.
struct PendingPoolEmit {
    file_count: usize,
    /// The checker thread jobs, or the payload of a panic while they were
    /// sent.
    checker: std::thread::Result<PendingCheckerJobs<CheckerOutput>>,
    /// The files whose whole emit runs on the pool, by file position.
    on_pool: Vec<(usize, EmitPoolJob<EmitResult>)>,
}

impl PendingPoolEmit {
    /// Waits for every job and returns one result per file, in file order.
    /// Every pool and twin job ends before a panic goes on.
    fn wait(self) -> Vec<EmitResult> {
        let checker_results = self.checker.map(|jobs| {
            jobs.wait_all()
                .into_iter()
                .map(|output| match output? {
                    CheckerOutput::Done(result) => Ok(result),
                    CheckerOutput::Twin(job) => job.join(),
                })
                .collect::<Vec<_>>()
        });
        let on_pool: Vec<(usize, std::thread::Result<EmitResult>)> = self
            .on_pool
            .into_iter()
            .map(|(index, job)| (index, job.join()))
            .collect();
        // The first panic of a checker file, in file order, goes on first.
        let mut checker_results = checker_results
            .unwrap_or_else(|payload| resume_unwind(payload))
            .into_iter()
            .map(|result| result.unwrap_or_else(|payload| resume_unwind(payload)))
            .collect::<Vec<_>>()
            .into_iter();
        let mut on_pool = on_pool.into_iter().peekable();
        (0..self.file_count)
            .map(
                |index| match on_pool.next_if(|(pool_index, _)| *pool_index == index) {
                    Some((_, result)) => result.unwrap_or_else(|payload| resume_unwind(payload)),
                    None => checker_results.next().expect("one result per checker file"),
                },
            )
            .collect()
    }
}

/// The JS part of a file's emit when the d.ts part runs in another emitter
/// (`Emitter::js_part`): a job on the emit pool, or the result of the JS
/// print that ran before it on the twin (`TwinFile::run`).
pub struct PoolJsPart {
    job: Option<EmitPoolJob<EmitResult>>,
    result: Option<std::thread::Result<EmitResult>>,
}

impl PoolJsPart {
    /// Waits for the JS part to end. Only the first call waits.
    pub fn wait(&mut self) {
        if let Some(job) = self.job.take() {
            self.result = Some(job.join());
        }
    }

    /// The diagnostics of the JS part, after `wait`. None when it panicked.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match &self.result {
            Some(Ok(result)) => &result.diagnostics,
            _ => &[],
        }
    }

    /// Waits, then takes the result, or the payload of the JS part's panic.
    fn take_result(&mut self) -> std::thread::Result<EmitResult> {
        self.wait();
        self.result.take().expect("the JS part is taken once")
    }
}

/// The d.ts part of a split file's emit, on its checker thread, merged with
/// the JS part `js_part` from the emit pool. `wrap` is around the d.ts part
/// only (the pool job wraps the JS part). Both parts end before a panic of
/// either goes on. With d.ts twins on (`dts_twin_mode`), the checker runs
/// the declaration transforms and its twin the print and the merge.
fn emit_declaration_part(
    source_file: Node,
    target: EmitterOptions,
    js_part: PoolJs,
    wrap: Wrap,
) -> CheckerOutput {
    let target = EmitterOptions {
        emit_only: EmitOnly::Dts,
        ..target
    };
    let mode = dts_twin_mode();
    if mode == DtsTwinMode::Off {
        return CheckerOutput::Done(emit_declaration_part_here(
            source_file,
            &target,
            js_part,
            wrap,
        ));
    }

    let twin_print: RefCell<Option<TwinPrint>> = RefCell::new(None);
    let dts = catch_unwind(AssertUnwindSafe(|| {
        wrap(&|| {
            let mut emitter = new_emitter(new_emit_host(source_file), source_file, &target, None);
            match emitter.transform_declaration_part(js_part.js_emit_nodes()) {
                Some(print) => {
                    *twin_print.borrow_mut() = Some(TwinPrint::declaration(
                        emitter,
                        print,
                        mode == DtsTwinMode::Check,
                    ));
                    EmitResult::default()
                }
                None => {
                    emitter.writer = None;
                    emitter.emit_result
                }
            }
        })
    }));
    let js_part = js_part.job;
    match (dts, twin_print.into_inner()) {
        (Ok(_), Some(twin_print)) => {
            CheckerOutput::Twin(send_dts_twin_job(move || twin_print.run(js_part, wrap)))
        }
        // Nothing to print, or a panic: the part ends here.
        (dts, _) => {
            let js = PoolJsPart {
                job: Some(js_part),
                result: None,
            }
            .take_result();
            let dts = dts.unwrap_or_else(|payload| resume_unwind(payload));
            let js = js.unwrap_or_else(|payload| resume_unwind(payload));
            CheckerOutput::Done(merge_emit_parts(js, dts))
        }
    }
}

/// PORT: not in Go (perf). True when the twin of a checker can print the
/// parts of a file's emit with `target` (`emit_on_twin`). A forced emit
/// (#4699, the API) and a builder signature emit stay on the checker
/// thread.
fn twin_can_print(target: &EmitterOptions) -> bool {
    !target.force_emit
        && matches!(
            target.emit_only,
            EmitOnly::All | EmitOnly::Js | EmitOnly::Dts
        )
}

/// PORT: not in Go (perf). The whole emit of a file on its checker thread
/// (`FileEmit::OnChecker`), with the twins on (`mode` is not `Off`). The
/// checker runs the JS transforms, then the declaration transforms, as
/// before, and its twin prints and writes the JS part, then the d.ts part
/// (`TwinFile::run`), so the outputs of the file are written in Go's order
/// (map, JS, declaration map, d.ts). The prints make no checker call, so
/// each checker gets the same calls in the same order.
///
/// The file emits as two parts, a JS part (`emit_only` is `Js`) and a d.ts
/// part (`Dts`), as a split file does (`emit_declaration_part`), and the
/// d.ts part's writes get the diagnostics of the JS part too. `wrap` is
/// around the transforms on the checker and around the prints on the twin:
/// a panic in either gives the file `wrap`'s result, as the emit of the
/// whole file on the checker does.
///
/// A panic in the declaration transforms goes on in the twin's `wrap`
/// after the twin printed and wrote the JS part, so the file's map and JS
/// are written, as in Go (`emitJSFile` writes before `emitDeclarationFile`
/// starts) and on the checker with the twins off.
///
/// One case is different from Go and from the twins off: a panic in the
/// JS print on the twin. In Go, that panic stops the file's emit before its
/// declaration transforms start. Here the checker ran them before the twin
/// printed. So the checker made their checker calls: they can add to its
/// caches and make types, and later files on that checker see these. A
/// panic or an unported hit in these transforms also shows: the panic hook
/// prints it and the unported counts get it. The writes are the same: the
/// file has no d.ts output, and its JS outputs are what the JS print wrote
/// before its panic. To keep Go's calls in this case, the checker must wait
/// for each JS print before the declaration transforms. That takes away
/// what the twin gains. With `DtsTwinMode::Check` the checker also prints
/// the JS part before the declaration transforms, so a panic in that print
/// comes first, as with the twins off.
fn emit_on_twin(
    source_file: Node,
    target: &EmitterOptions,
    wrap: Wrap,
    mode: DtsTwinMode,
) -> CheckerOutput {
    let check = mode == DtsTwinMode::Check;
    let twin_file: RefCell<Option<TwinFile>> = RefCell::new(None);
    let here = wrap(&|| {
        let host = new_emit_host(source_file);
        let part_target = |emit_only| EmitterOptions {
            emit_only,
            ..target.clone()
        };
        // ts#64649: what the JS transforms wrote on parse-tree nodes, for
        // the d.ts part (`Emitter::transform_declaration_part`).
        let mut js_emit_nodes = None;
        let js = matches!(target.emit_only, EmitOnly::All | EmitOnly::Js).then(|| {
            let mut emitter =
                new_emitter(host.clone(), source_file, &part_target(EmitOnly::Js), None);
            match emitter.transform_js_part() {
                Some(print) => {
                    js_emit_nodes = Some(print.emit_context.export_parse_emit_nodes());
                    TwinPart::Print(TwinPrint::js(emitter, print, check))
                }
                None => TwinPart::Done(emitter.emit_result),
            }
        });
        let dts = matches!(target.emit_only, EmitOnly::All | EmitOnly::Dts).then(|| {
            catch_unwind(AssertUnwindSafe(|| {
                let mut emitter =
                    new_emitter(host.clone(), source_file, &part_target(EmitOnly::Dts), None);
                match emitter.transform_declaration_part(js_emit_nodes) {
                    Some(print) => TwinPart::Print(TwinPrint::declaration(emitter, print, check)),
                    None => TwinPart::Done(emitter.emit_result),
                }
            }))
            .unwrap_or_else(TwinPart::Panicked)
        });
        let file = TwinFile {
            js: js.unwrap_or_default(),
            dts: dts.unwrap_or_default(),
        };
        match file.done() {
            Ok(result) => result,
            Err(file) => {
                *twin_file.borrow_mut() = Some(file);
                EmitResult::default()
            }
        }
    });
    match twin_file.into_inner() {
        Some(file) => CheckerOutput::Twin(send_dts_twin_job(move || {
            let file = RefCell::new(Some(file));
            wrap(&|| {
                file.borrow_mut()
                    .take()
                    .expect("a twin file runs once")
                    .run()
            })
        })),
        None => CheckerOutput::Done(here),
    }
}

/// One part of a file's emit in `emit_on_twin`.
enum TwinPart {
    /// The part ended on the checker thread (no print), with this result.
    Done(EmitResult),
    /// The twin prints the part.
    Print(TwinPrint),
    /// The transforms of the d.ts part panicked on the checker thread with
    /// this payload. The panic goes on after the JS part is printed.
    Panicked(Box<dyn std::any::Any + Send>),
}

impl Default for TwinPart {
    /// A part that the target does not emit.
    fn default() -> Self {
        TwinPart::Done(EmitResult::default())
    }
}

/// The two parts of a file's emit that `emit_on_twin` sends to the twin.
struct TwinFile {
    js: TwinPart,
    dts: TwinPart,
}

impl TwinFile {
    /// The merged result when no part has a print, else the file. When the
    /// JS part has no print and the d.ts transforms panicked, the panic goes
    /// on here.
    fn done(self) -> Result<EmitResult, TwinFile> {
        match self {
            TwinFile {
                js: TwinPart::Done(js),
                dts: TwinPart::Done(dts),
            } => Ok(merge_emit_parts(js, dts)),
            TwinFile {
                js: TwinPart::Done(_),
                dts: TwinPart::Panicked(payload),
            } => resume_unwind(payload),
            file => Err(file),
        }
    }

    /// On the twin: prints and writes the JS part, then the d.ts part, and
    /// returns their merged result. A panic of the d.ts transforms goes on
    /// after the JS part is written.
    fn run(self) -> EmitResult {
        let js = match self.js {
            TwinPart::Done(result) => result,
            TwinPart::Print(print) => print.print(None),
            TwinPart::Panicked(payload) => resume_unwind(payload),
        };
        let js_part = Rc::new(RefCell::new(PoolJsPart {
            job: None,
            result: Some(Ok(js)),
        }));
        let dts = match self.dts {
            TwinPart::Done(result) => result,
            TwinPart::Print(print) => print.print(Some(js_part.clone())),
            TwinPart::Panicked(payload) => resume_unwind(payload),
        };
        let js = js_part
            .borrow_mut()
            .take_result()
            .unwrap_or_else(|payload| resume_unwind(payload));
        merge_emit_parts(js, dts)
    }
}

/// `emit_declaration_part` with the whole d.ts part on the checker thread.
fn emit_declaration_part_here(
    source_file: Node,
    target: &EmitterOptions,
    js_part: PoolJs,
    wrap: Wrap,
) -> EmitResult {
    let PoolJs { job, emit_nodes } = js_part;
    let js_part = Rc::new(RefCell::new(PoolJsPart {
        job: Some(job),
        result: None,
    }));
    let dts = catch_unwind(AssertUnwindSafe(|| {
        wrap(&|| {
            let mut emitter = new_emitter(
                new_emit_host(source_file),
                source_file,
                target,
                Some(js_part.clone()),
            );
            // `PoolJs::js_emit_nodes`
            if let Some(print) = emitter.transform_declaration_part(emit_nodes.recv().ok()) {
                emitter.finish_declaration_part(print);
            }
            emitter.writer = None;
            emitter.emit_result
        })
    }));
    let js = js_part.borrow_mut().take_result();
    let dts = dts.unwrap_or_else(|payload| resume_unwind(payload));
    let js = js.unwrap_or_else(|payload| resume_unwind(payload));
    merge_emit_parts(js, dts)
}

/// Where the parts whose transforms run on a checker thread print: the d.ts
/// part of a split file, and the JS and d.ts parts of a file whose JS part
/// needs the checker (`GOPORT_DTS_TWIN`, `GOPORT_DTS_TWIN_CHECK`,
/// `set_dts_twin_mode`). The twins are on only with the emit pool
/// (`program::emit_pool_enabled`: not with `--singleThreaded` or a trace).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DtsTwinMode {
    /// On the checker thread, after its transforms.
    Off,
    /// On the twin of the checker (`program::send_dts_twin_job`).
    On,
    /// On the twin, and on the checker too: the twin panics when its text
    /// differs from the checker's (tests and checks).
    Check,
}

/// The mode that `set_dts_twin_mode` set: 0 for none, else the mode + 1.
static DTS_TWIN_MODE_SET: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Sets the d.ts twin mode of this process, or with `None` goes back to the
/// environment (`dts_twin_mode`). Tests use it.
pub fn set_dts_twin_mode(mode: Option<DtsTwinMode>) {
    let value = match mode {
        None => 0,
        Some(DtsTwinMode::Off) => 1,
        Some(DtsTwinMode::On) => 2,
        Some(DtsTwinMode::Check) => 3,
    };
    DTS_TWIN_MODE_SET.store(value, std::sync::atomic::Ordering::Relaxed);
}

/// PORT: not in Go (perf). The d.ts twin mode: `set_dts_twin_mode`, else
/// `Off` when `GOPORT_DTS_TWIN=0`, `Check` when `GOPORT_DTS_TWIN_CHECK=1`,
/// else `On`. The environment is read once.
pub fn dts_twin_mode() -> DtsTwinMode {
    static ENV: OnceLock<DtsTwinMode> = OnceLock::new();
    match DTS_TWIN_MODE_SET.load(std::sync::atomic::Ordering::Relaxed) {
        1 => DtsTwinMode::Off,
        2 => DtsTwinMode::On,
        3 => DtsTwinMode::Check,
        _ => *ENV.get_or_init(|| {
            if std::env::var("GOPORT_DTS_TWIN").as_deref() == Ok("0") {
                DtsTwinMode::Off
            } else if std::env::var("GOPORT_DTS_TWIN_CHECK").as_deref() == Ok("1") {
                DtsTwinMode::Check
            } else {
                DtsTwinMode::On
            }
        }),
    }
}

/// The JS or d.ts print of a file part that its checker thread hands to its
/// twin (`emit_declaration_part`, `emit_on_twin`): the emitter state after
/// the transforms, the transformed tree with a copy of the synthetic nodes
/// it reaches (`PrintPack`), and the side tables of its emit context.
struct TwinPrint {
    emit_only: EmitOnly,
    emitter_diagnostics: DiagnosticsCollection,
    paths: OutputPaths,
    source_file: Node,
    emit_result: EmitResult,
    force_emit: bool,
    write_file: Option<WriteFile>,
    /// The transformed SourceFile.
    root: Node,
    tree: TwinTree,
    tables: PrintTables,
    pack: PrintPack,
    /// `DtsTwinMode::Check`: the writes of the print on the checker, in
    /// order (file name and text).
    expected: Option<Vec<(String, String)>>,
}

/// The kind of tree that a `TwinPrint` prints.
enum TwinTree {
    /// A JS file (`JsPrint`).
    Js,
    /// A d.ts file (`DeclarationPrint`).
    Declaration {
        /// `DeclarationPrint::content_mapped_source`: a parsed SourceFile,
        /// which every thread reads.
        content_mapped_source: Node,
        emit_declaration_map: bool,
    },
}

/// The JS prints sent to twins in this process (`js_twin_print_count`).
static JS_TWIN_PRINTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The number of JS prints sent to twins in this process so far. Tests use
/// it to see that the twins printed JS files.
pub fn js_twin_print_count() -> usize {
    JS_TWIN_PRINTS.load(std::sync::atomic::Ordering::Relaxed)
}

impl TwinPrint {
    /// The print of `emitter` after `transform_js_part` returned `print`.
    /// With `check`, it prints on this thread too and keeps the writes,
    /// without writing.
    fn js(emitter: Emitter, print: JsPrint, check: bool) -> Self {
        let root = print.source_file;
        let (tables, pack) = export_print(&print.emit_context, &[root], check);
        let expected =
            check.then(|| record_print(&emitter, |recorder| recorder.finish_js_part(print)));
        JS_TWIN_PRINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self::from_parts(emitter, root, TwinTree::Js, tables, pack, expected)
    }

    /// The print of `emitter` after `transform_declaration_part` returned
    /// `print`. With `check`, it prints on this thread too and keeps the
    /// writes, without writing.
    fn declaration(emitter: Emitter, print: DeclarationPrint, check: bool) -> Self {
        let root = print.source_file;
        let tree = TwinTree::Declaration {
            content_mapped_source: print.content_mapped_source,
            emit_declaration_map: print.emit_declaration_map,
        };
        // The walk skips a parsed `content_mapped_source`.
        let (tables, pack) = export_print(
            &print.emit_context,
            &[root, print.content_mapped_source],
            check,
        );
        let expected = check
            .then(|| record_print(&emitter, |recorder| recorder.finish_declaration_part(print)));
        Self::from_parts(emitter, root, tree, tables, pack, expected)
    }

    fn from_parts(
        emitter: Emitter,
        root: Node,
        tree: TwinTree,
        tables: PrintTables,
        pack: PrintPack,
        expected: Option<Vec<(String, String)>>,
    ) -> Self {
        let Emitter {
            emit_only,
            emitter_diagnostics,
            paths,
            source_file,
            emit_result,
            force_emit,
            write_file,
            ..
        } = emitter;
        TwinPrint {
            emit_only,
            emitter_diagnostics,
            paths,
            source_file,
            emit_result,
            force_emit,
            write_file,
            root,
            tree,
            tables,
            pack,
            expected,
        }
    }

    /// On the d.ts twin: prints and writes the d.ts part of a split file,
    /// waits for the JS part `js_part` on the emit pool, and returns their
    /// merged result, as `emit_declaration_part_here` does on the checker
    /// thread.
    fn run(self, js_part: EmitPoolJob<EmitResult>, wrap: Wrap) -> EmitResult {
        let js_part = Rc::new(RefCell::new(PoolJsPart {
            job: Some(js_part),
            result: None,
        }));
        let print = RefCell::new(Some(self));
        let dts = catch_unwind(AssertUnwindSafe(|| {
            wrap(&|| {
                print
                    .borrow_mut()
                    .take()
                    .expect("a d.ts print runs once")
                    .print(Some(js_part.clone()))
            })
        }));
        let js = js_part.borrow_mut().take_result();
        let dts = dts.unwrap_or_else(|payload| resume_unwind(payload));
        let js = js.unwrap_or_else(|payload| resume_unwind(payload));
        merge_emit_parts(js, dts)
    }

    /// On the twin: the print and the writes of the part. A d.ts part gets
    /// its file's JS part `js_part` (`Emitter::js_part`).
    fn print(self, js_part: Option<Rc<RefCell<PoolJsPart>>>) -> EmitResult {
        install_print_pack(self.pack);
        let emit_context =
            crate::printer::emit_context::EmitContext::from_print_tables(self.tables);
        let expected = self
            .expected
            .map(|expected| Arc::new(Mutex::new(expected.into_iter())));
        let write_file = match &expected {
            None => self.write_file,
            Some(expected) => Some(checked_write_file(self.write_file, Arc::clone(expected))),
        };
        let new_line = options().new_line.get_new_line_character();
        let mut emitter = Emitter {
            host: new_emit_host_without_checker(),
            emit_only: self.emit_only,
            emitter_diagnostics: self.emitter_diagnostics,
            writer: Some(Rc::new(RefCell::new(new_text_writer(new_line, 0)))),
            paths: self.paths,
            source_file: self.source_file,
            emit_result: self.emit_result,
            force_emit: self.force_emit,
            write_file,
            js_part,
        };
        match self.tree {
            TwinTree::Js => emitter.finish_js_part(JsPrint::new(self.root, emit_context)),
            TwinTree::Declaration {
                content_mapped_source,
                emit_declaration_map,
            } => emitter.finish_declaration_part(DeclarationPrint::new(
                self.root,
                content_mapped_source,
                emit_declaration_map,
                emit_context,
            )),
        }
        emitter.writer = None;
        // The twin keeps the nodes of one print at a time.
        release_print_packs();
        if let Some(expected) = expected {
            let rest: Vec<String> = expected
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .by_ref()
                .map(|(name, _)| name)
                .collect();
            assert!(
                rest.is_empty(),
                "twin check: the twin did not write {rest:?}"
            );
        }
        emitter.emit_result
    }
}

/// The side tables of `emit_context` after its transforms, and a copy of
/// the synthetic nodes that `roots` and the tables reach (`PrintPack`).
/// With `check` the tables are copied, so the checker can print too; else
/// they move out.
fn export_print(
    emit_context: &EmitContext,
    roots: &[Node],
    check: bool,
) -> (PrintTables, PrintPack) {
    let tables = if check {
        emit_context.clone_print_tables()
    } else {
        emit_context.take_print_tables()
    };
    let mut roots = roots.to_vec();
    tables.for_each_value_node(|n| roots.push(n));
    let pack = export_print_pack(&roots, |n, more| {
        tables.for_each_emit_node_value(n, |n| more.push(n));
    });
    (tables, pack)
}

/// `DtsTwinMode::Check` on the checker thread: runs `finish` (the rest of
/// the part's emit) on an emitter like `emitter` on this thread, and
/// returns its writes in order, without writing them.
fn record_print(emitter: &Emitter, finish: impl FnOnce(&mut Emitter)) -> Vec<(String, String)> {
    let writes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = Arc::clone(&writes);
    let record: WriteFile = Arc::new(move |name: &str, text: &str, _data: &mut WriteFileData| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name.to_string(), text.to_string()));
        Ok(())
    });
    let target = EmitterOptions {
        emit_only: emitter.emit_only,
        force_emit: emitter.force_emit,
        write_file: Some(record),
    };
    let mut recorder = new_emitter(emitter.host.clone(), emitter.source_file, &target, None);
    finish(&mut recorder);
    std::mem::take(&mut *writes.lock().unwrap_or_else(PoisonError::into_inner))
}

/// `DtsTwinMode::Check` on the d.ts twin: `write_file` (or the emit host
/// write when it is None) after a check that each write is the next one in
/// `expected`, the writes of the same print on the checker thread. It
/// panics at the first difference.
fn checked_write_file(
    write_file: Option<WriteFile>,
    expected: Arc<Mutex<std::vec::IntoIter<(String, String)>>>,
) -> WriteFile {
    Arc::new(move |name: &str, text: &str, data: &mut WriteFileData| {
        let next = expected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next();
        match next {
            Some((expected_name, expected_text))
                if expected_name == name && expected_text == text => {}
            Some((expected_name, expected_text)) => {
                let at = expected_text
                    .bytes()
                    .zip(text.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(expected_text.len().min(text.len()));
                panic!(
                    "twin check: the twin wrote {name} ({} bytes), the checker {expected_name} ({} bytes); they differ at byte {at}",
                    text.len(),
                    expected_text.len()
                );
            }
            None => panic!("twin check: the checker did not write {name}"),
        }
        match &write_file {
            Some(write_file) => write_file(name, text, data),
            None => crate::printer::EmitHost::write_file(
                new_emit_host_without_checker().as_ref(),
                name,
                text,
            ),
        }
    })
}

/// Go `emitter.emit` of one file from its two parts, JS first, as Go runs
/// them. Go keeps the diagnostics of both parts in one collection; each
/// part's are sorted (`get_diagnostics`), and the merge sorts them together.
fn merge_emit_parts(js: EmitResult, dts: EmitResult) -> EmitResult {
    let mut diagnostics = DiagnosticsCollection::default();
    for diagnostic in js.diagnostics.into_iter().chain(dts.diagnostics) {
        diagnostics.add(diagnostic);
    }
    let mut emitted_files = js.emitted_files;
    emitted_files.extend(dts.emitted_files);
    let mut source_maps = js.source_maps;
    source_maps.extend(dts.source_maps);
    EmitResult {
        emit_skipped: js.emit_skipped || dts.emit_skipped,
        diagnostics: diagnostics.get_diagnostics(),
        emitted_files,
        source_maps,
    }
}

/// The JS part of a file's emit on the emit pool (`emit_only` is `Js`), or
/// its whole emit when it has no d.ts part: `emit_source_file` with a host
/// that has no checker. A JS part sends what its transforms wrote on
/// parse-tree nodes to `js_emit_nodes`, for the file's d.ts part
/// (ts#64649, `Emitter::transform_declaration_part`), before its print, so
/// the d.ts part can start. When it runs no transforms (skipped, blocked)
/// or panics, it sends nothing, and the sender drops when the job ends.
fn emit_source_file_on_pool(
    source_file: Node,
    target: &EmitterOptions,
    js_emit_nodes: Option<&SyncSender<ParseEmitNodes>>,
) -> EmitResult {
    let host = new_emit_host_without_checker();
    let Some(js_emit_nodes) = js_emit_nodes else {
        return emit_source_file_with(host, source_file, target);
    };
    let mut emitter = new_emitter(host, source_file, target, None);
    if let Some(print) = emitter.transform_js_part() {
        // The channel holds one value, and only this job sends. The d.ts
        // part may have dropped the receiver after a panic.
        let _ = js_emit_nodes.try_send(print.emit_context.export_parse_emit_nodes());
        emitter.finish_js_part(print);
    }
    emitter.writer = None;
    emitter.emit_result
}

/// The body of the Go `wg.Queue` closure in `Program.Emit`.
fn emit_source_file(source_file: Node, target: &EmitterOptions) -> EmitResult {
    emit_source_file_with(new_emit_host(source_file), source_file, target)
}

/// `emit_source_file` with the emit host.
fn emit_source_file_with(
    host: Rc<crate::program::EmitHost>,
    source_file: Node,
    target: &EmitterOptions,
) -> EmitResult {
    let mut emitter = new_emitter(host, source_file, target, None);
    emitter.emit();
    emitter.writer = None;
    emitter.emit_result
}

/// The emitter of `source_file` with `target`, before its emit.
fn new_emitter(
    host: Rc<crate::program::EmitHost>,
    source_file: Node,
    target: &EmitterOptions,
    js_part: Option<Rc<RefCell<PoolJsPart>>>,
) -> Emitter {
    let new_line = options().new_line.get_new_line_character();
    let writer: Rc<RefCell<dyn EmitTextWriter>> =
        Rc::new(RefCell::new(new_text_writer(new_line, 0)));
    writer.borrow_mut().clear();
    let paths =
        get_output_paths_for_source_file(source_file, host.as_ref(), target.force_emit_paths());
    Emitter {
        host,
        emit_only: target.emit_only,
        emitter_diagnostics: DiagnosticsCollection::default(),
        writer: Some(writer),
        paths,
        source_file,
        emit_result: EmitResult::default(),
        force_emit: target.force_emit,
        write_file: target.write_file.clone(),
        js_part,
    }
}

// Go: compiler/program.go:1962 CombineEmitResults
pub fn combine_emit_results(results: Vec<EmitResult>) -> EmitResult {
    let mut result = EmitResult::default();
    for emit_result in results {
        if emit_result.emit_skipped {
            result.emit_skipped = true;
        }
        result.diagnostics.extend(emit_result.diagnostics);
        result.emitted_files.extend(emit_result.emitted_files);
        result.source_maps.extend(emit_result.source_maps);
    }
    result
}

// Go: compiler/program.go:1999 HandleNoEmitOptions
// HandleNoEmitOptions mirrors tsc's handleNoEmitOptions.
// PORT: #4407 replaced Go `HandleNoEmitOnError`. This is the plain program
// form, for `Program.Emit`, which passes a nil `emitBuildInfo`, so it has no
// such parameter. The incremental program has its own form
// (`execute::incremental::program`).
pub fn handle_no_emit_options(files: Option<&[Node]>) -> Option<EmitResult> {
    let options = options();
    if !options.no_emit.is_true() {
        if !options.no_emit_on_error.is_true() {
            return None; // NoEmit is false and NoEmitOnError is also false, so we can proceed with normal emit
        }

        let diagnostics = get_diagnostics_of_any_program(
            files,
            true,
            &mut get_bind_diagnostics,
            &mut get_semantic_diagnostics,
            &mut get_global_diagnostics,
            &mut get_declaration_diagnostics,
            // ts#64452: Go `program.(*Program)` holds for the plain program.
            true,
        );
        if diagnostics.is_empty() {
            return None; // NoEmitOnError is enabled, but no diagnostics were found, so we can proceed with emitting
        }
        return Some(EmitResult {
            diagnostics,
            emit_skipped: true,
            ..EmitResult::default()
        });
    }
    if files.is_some() {
        return Some(EmitResult {
            emit_skipped: true,
            ..EmitResult::default()
        });
    }
    Some(EmitResult::default())
}
