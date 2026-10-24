//! Port of execute/incremental/emitfileshandler.go.
//!
//! PORT: Go emits the affected files on a work group; here
//! `emit_files_incremental` sends them as one `emit_batch`, in the order Go
//! queues them. Each emit runs on the file's checker thread, and each
//! checker thread runs its files in that order. The `WriteFile` callback
//! runs on the checker threads, so what it reads from the snapshot is
//! copied into it, and what it records (Go `signatures`, `emitSignatures`,
//! `latestChangedDtsFiles` SyncMaps) goes to `EmitFilesShared` behind a
//! mutex. Go `ctx` is dropped. `start_emit_files` and `finish_emit_files`
//! split the emit of all affected files at the wait for the emit jobs, so
//! `Program::start_emit` can send them behind the check.

use super::affected_files::collect_all_affected_files;
use super::hash::FileInfo;
use super::hash::*;
use super::program::{Program, SignatureUpdateKind};
use super::snapshot::*;
use crate::emitter::emitter::EmitOnly;
use crate::emitter::program_emit::{
    EmitOptions, EmitResult, PendingEmitBatch, WriteFile, WriteFileData, combine_emit_results,
    emit, start_emit_batch,
};
use crate::frontend::prelude::*;
use crate::program::source_file_may_be_emitted;
use std::cell::Cell;
use std::sync::{Arc, Mutex, PoisonError};

// Go: incremental/emitfileshandler.go:15 emitUpdate
#[derive(Clone, Debug, Default)]
pub struct EmitUpdate {
    pub pending_kind: FileEmitKind,
    pub result: Option<EmitResult>,
    pub dts_errors_from_cache: bool,
}

/// The Go `emitFilesHandler` SyncMaps that the `WriteFile` callback writes.
#[derive(Debug, Default)]
pub struct EmitFilesShared {
    pub signatures: FxIndexMap<Path, String>,
    pub emit_signatures: FxIndexMap<Path, EmitSignature>,
    pub latest_changed_dts_files: FxIndexMap<Path, String>,
}

/// One file that `emit_files_incremental` emits: its path, its pending
/// emit kind, the kind it emits now, and the file.
type QueuedEmit = (Path, FileEmitKind, FileEmitKind, Node);

// Go: incremental/emitfileshandler.go:21 emitFilesHandler
pub struct EmitFilesHandler<'a> {
    program: &'a Program,
    is_for_dts_errors: bool,
    shared: Arc<Mutex<EmitFilesShared>>,
    deleted_pending_kinds: FxIndexSet<Path>,
    emit_updates: FxIndexMap<Path, EmitUpdate>,
    has_emit_diagnostics: bool,
    /// PORT: not in Go. Where the write callbacks of `get_emit_options`
    /// send the writes (`Writes`).
    writes: Writes,
}

/// Go `file.Path()` as a `tspath.Path`.
fn path_of(file: Node) -> Path {
    Path(source_file_info(file).path.clone())
}

/// Go `err.Error()` of a file system error.
// PORT: Go prints the `*os.PathError` as "op path: err", where `err` is the
// Go `syscall.Errno` text (`pprof::path_error`, which uses
// `fswatch::syscall::io_error_text` on every target). The other errors are
// the Go `io/fs` errors.
pub fn fs_error_text(err: &FsError) -> String {
    match err {
        FsError::Path { op, path, err } => crate::pprof::path_error(op, path, err).error(),
        FsError::Invalid => "invalid argument".to_string(),
        FsError::Permission => "permission denied".to_string(),
        FsError::Exist => "file already exists".to_string(),
        FsError::NotExist => "file does not exist".to_string(),
        FsError::Closed => "file already closed".to_string(),
        FsError::SkipAll => "skip everything and stop the walk".to_string(),
        FsError::SkipDir => "skip this directory".to_string(),
        FsError::Other(message) => message.clone(),
    }
}

impl<'a> EmitFilesHandler<'a> {
    #[must_use]
    pub fn new(program: &'a Program, is_for_dts_errors: bool) -> Self {
        EmitFilesHandler {
            program,
            is_for_dts_errors,
            shared: Arc::new(Mutex::new(EmitFilesShared::default())),
            deleted_pending_kinds: IndexSet::default(),
            emit_updates: IndexMap::default(),
            has_emit_diagnostics: false,
            writes: Writes::Direct,
        }
    }

    // Go: incremental/emitfileshandler.go:34 getPendingEmitKindForEmitOptions
    // Determining what all is pending to be emitted based on previous options or previous file emit flags
    fn get_pending_emit_kind_for_emit_options(
        &self,
        emit_kind: FileEmitKind,
        options: &EmitOptions,
    ) -> FileEmitKind {
        let mut pending_kind = get_pending_emit_kind(emit_kind, FileEmitKind::NONE);
        if options.emit_only == EmitOnly::Dts {
            pending_kind &= FileEmitKind::ALL_DTS;
        }
        if self.is_for_dts_errors {
            pending_kind &= FileEmitKind::DTS_ERRORS;
        }
        pending_kind
    }

    // Go: incremental/emitfileshandler.go:48 emitAllAffectedFiles
    // Emits the next affected file's emit result (EmitResult and sourceFiles emitted) or returns undefined if iteration is complete
    // The first of writeFile if provided, writeFile of BuilderProgramHost if provided, writeFile of compiler host
    // in that order would be used to write the files
    fn emit_all_affected_files(&mut self, options: EmitOptions) -> EmitResult {
        // Emit all affected files
        if self.program.snapshot.borrow().can_use_incremental_state() {
            let results = self.emit_files_incremental(&options);
            if self.is_for_dts_errors {
                if let Some(target_files) = &options.target_source_files {
                    // Result from cache
                    // #4699: Go `core.FlatMap` over the target files.
                    let mut snapshot = self.program.snapshot.borrow_mut();
                    let diagnostics = target_files
                        .iter()
                        .flat_map(|&target_file| {
                            snapshot
                                .emit_diagnostics_per_file
                                .get_mut(&path_of(target_file))
                                .expect("emit diagnostics of the target file")
                                .get_diagnostics(target_file)
                        })
                        .collect();
                    drop(snapshot);
                    let result = EmitResult {
                        emit_skipped: true,
                        diagnostics,
                        ..EmitResult::default()
                    };
                    self.update_has_emit_diagnostics(Some(&result));
                    return result;
                }
                for result in &results {
                    self.update_has_emit_diagnostics(Some(result));
                }
                combine_emit_results(results)
            } else {
                self.combine_results_and_emit_build_info(results, &options)
            }
        } else if !self.is_for_dts_errors {
            let mut result = self.emit_with_writes_here(
                || source_files().into_iter().map(path_of).collect(),
                options.write_file.as_ref(),
                |handler| emit(handler.get_emit_options(options.clone())),
            );
            self.update_has_emit_diagnostics(Some(&result));
            self.update_snapshot();
            self.emit_build_info(&options, &mut result);
            result
        } else {
            // #4699: Go nil targets ask for all files; else `core.FlatMap`
            // over the target files.
            let diagnostics = match &options.target_source_files {
                None => get_declaration_diagnostics(Node::NIL),
                Some(target_files) => target_files
                    .iter()
                    .flat_map(|&target_source_file| get_declaration_diagnostics(target_source_file))
                    .collect(),
            };
            let result = EmitResult {
                emit_skipped: true,
                diagnostics,
                ..EmitResult::default()
            };
            if !result.diagnostics.is_empty() {
                self.update_has_emit_diagnostics(Some(&result));
                self.program.snapshot.borrow_mut().has_emit_diagnostics = true;
            }
            result
        }
    }

    /// The end of `emitAllAffectedFiles` with the incremental state, not
    /// for d.ts errors.
    fn combine_results_and_emit_build_info(
        &mut self,
        results: Vec<EmitResult>,
        options: &EmitOptions,
    ) -> EmitResult {
        // Combine results and update buildInfo
        let mut result = combine_emit_results(results);
        self.update_has_emit_diagnostics(Some(&result));
        self.emit_build_info(options, &mut result);
        result
    }

    // Go: incremental/emitfileshandler.go:103 updateHasEmitDiagnostics
    fn update_has_emit_diagnostics(&mut self, result: Option<&EmitResult>) {
        if result.is_some_and(|result| !result.diagnostics.is_empty()) {
            self.has_emit_diagnostics = true;
        }
    }

    // Go: incremental/emitfileshandler.go:109 emitBuildInfo
    fn emit_build_info(&self, options: &EmitOptions, result: &mut EmitResult) {
        if let Some(build_info_result) = self.program.emit_build_info(options) {
            result.diagnostics.extend(build_info_result.diagnostics);
            result.emitted_files.extend(build_info_result.emitted_files);
        }
    }

    // Go: incremental/emitfileshandler.go:117 emitFilesIncremental
    // PORT: perf. Go queues one job per file on a WorkGroup. Pass 1 is the
    // loop body before `wg.Queue`, in the order Go queues
    // (`queue_affected_files`). Pass 2 runs all jobs at once (`emit_batch`,
    // or one declaration diagnostics job per file); each checker thread runs
    // its files in pass 1 order, as the old one-file-at-a-time loop did.
    // Pass 3 is the job tail, in the same order
    // (`finish_emit_files_incremental`). `start_emit_files` runs pass 1 and
    // sends pass 2 early; `finish_emit_files` does the rest.
    fn emit_files_incremental(&mut self, options: &EmitOptions) -> Vec<EmitResult> {
        let queued = self.queue_affected_files(options);
        let results: Vec<EmitResult> = if !self.is_for_dts_errors {
            self.emit_with_writes_here(
                || queued.iter().map(|(path, ..)| path.clone()).collect(),
                options.write_file.as_ref(),
                |handler| handler.send_emit_batch(&queued, options).wait(),
            )
        } else {
            // Go `GetDeclarationDiagnostics(ctx, affectedFile)`. Send every
            // job first, then wait for each in order.
            let jobs: Vec<_> = queued
                .iter()
                .map(|&(.., affected_file)| send_declaration_diagnostics_job(affected_file))
                .collect();
            jobs.into_iter()
                .map(|job| EmitResult {
                    emit_skipped: true,
                    diagnostics: sort_and_deduplicate_diagnostics(job.wait()),
                    ..EmitResult::default()
                })
                .collect()
        };
        self.finish_emit_files_incremental(queued, results)
    }

    /// Pass 1 of `emit_files_incremental`: handles the affected files and
    /// returns the files to emit, in the order Go queues them.
    fn queue_affected_files(&mut self, options: &EmitOptions) -> Vec<QueuedEmit> {
        // Get all affected files
        collect_all_affected_files(self.program);

        let pending: Vec<(Path, FileEmitKind)> = self
            .program
            .snapshot
            .borrow()
            .affected_files_pending_emit
            .iter()
            .map(|(path, kind)| (path.clone(), *kind))
            .collect();

        let mut queued: Vec<QueuedEmit> = Vec::new();
        for (path, emit_kind) in pending {
            let affected_file = get_source_file_by_path(&path);
            if affected_file.is_nil() || !source_file_may_be_emitted(affected_file, false) {
                self.deleted_pending_kinds.insert(path);
                continue;
            }
            let pending_kind = self.get_pending_emit_kind_for_emit_options(emit_kind, options);
            if !pending_kind.is_empty() {
                queued.push((path, emit_kind, pending_kind, affected_file));
            }
        }
        queued
    }

    /// Pass 2 of `emit_files_incremental` (not for d.ts errors) without the
    /// wait: sends the emit of the `queued` files.
    fn send_emit_batch(&self, queued: &[QueuedEmit], options: &EmitOptions) -> PendingEmitBatch {
        let targets = queued
            .iter()
            .map(|&(_, _, pending_kind, affected_file)| {
                // Determine if we can do partial emit
                let mut emit_only = EmitOnly::All;
                if pending_kind.intersects(FileEmitKind::ALL_JS) {
                    emit_only = EmitOnly::Js;
                }
                if pending_kind.intersects(FileEmitKind::ALL_DTS) {
                    if emit_only == EmitOnly::Js {
                        emit_only = EmitOnly::All;
                    } else {
                        emit_only = EmitOnly::Dts;
                    }
                }
                self.get_emit_options(EmitOptions {
                    // #4699: Go `core.SingleElementSlice(affectedFile)`; the
                    // file is not nil.
                    target_source_files: Some(vec![affected_file]),
                    emit_only,
                    write_file: options.write_file.clone(),
                    ..EmitOptions::default()
                })
            })
            .collect();
        start_emit_batch(targets)
    }

    /// PORT: not in Go. Runs `emit`, which emits files (`order` gives
    /// them in emit order), and returns its result. Go's emitter gets each
    /// write's error at the write and puts its TS5033 into the result of
    /// the file. Under `flush_writes_on_this_thread` (the API build) only
    /// this thread reaches the file system. So the emit keeps its writes
    /// (`Writes::Buffer`), then this thread makes them (`flush_writes`).
    /// After a failed write the files emit again with the results of the
    /// flush (`replay_after_flush`), as `finish_emit_files` does for an
    /// early emit. Else `emit` writes at once.
    fn emit_with_writes_here<R>(
        &mut self,
        order: impl FnOnce() -> Vec<Path>,
        write_file: Option<&WriteFile>,
        emit: impl Fn(&Self) -> R,
    ) -> R {
        if !FLUSH_ON_THIS_THREAD.get() {
            return emit(self);
        }
        let buffer = WriteBuffer::default();
        self.writes = Writes::Buffer(buffer.clone());
        let mut result = emit(self);
        let writes = std::mem::take(&mut *buffer.lock().unwrap_or_else(PoisonError::into_inner));
        if self.replay_after_flush(writes, &order(), write_file) {
            result = emit(self);
        }
        self.writes = Writes::Direct;
        result
    }

    /// PORT: not in Go. Makes the buffered `writes` of an emit of the
    /// files in `order` (`flush_writes`). After a failed write it gets the
    /// handler ready for the same emit again: each write gives the result
    /// of the flush (`Writes::Replay`), and `shared` is empty (the
    /// callbacks of the buffered emit filled it). Then it returns true, and
    /// the caller emits again.
    fn replay_after_flush(
        &mut self,
        writes: Vec<BufferedWrite>,
        order: &[Path],
        write_file: Option<&WriteFile>,
    ) -> bool {
        // Go: emitfileshandler.go:124 `h.program.program.SingleThreaded()`.
        let single_threaded = self.program.program().single_threaded();
        let Some(failures) = flush_writes(writes, order, write_file, single_threaded) else {
            return false;
        };
        *self.shared.lock().expect("emit files lock") = EmitFilesShared::default();
        self.writes = Writes::Replay(Arc::new(failures));
        true
    }

    /// Pass 3 of `emit_files_incremental` with the pass 2 `results` of the
    /// `queued` files, then the rest of Go `emitFilesIncremental`.
    fn finish_emit_files_incremental(
        &mut self,
        queued: Vec<QueuedEmit>,
        results: Vec<EmitResult>,
    ) -> Vec<EmitResult> {
        debug_assert_eq!(results.len(), queued.len(), "one result per queued file");
        for ((path, emit_kind, pending_kind, _), result) in queued.into_iter().zip(results) {
            self.update_has_emit_diagnostics(Some(&result));

            // Update the pendingEmit for the file
            self.emit_updates.insert(
                path,
                EmitUpdate {
                    pending_kind: get_pending_emit_kind(emit_kind, pending_kind),
                    result: Some(result),
                    dts_errors_from_cache: false,
                },
            );
        }

        // Get updated errors that were not included in affected files emit
        let emit_diagnostic_paths: Vec<Path> = self
            .program
            .snapshot
            .borrow()
            .emit_diagnostics_per_file
            .keys()
            .cloned()
            .collect();
        for path in emit_diagnostic_paths {
            if !self.emit_updates.contains_key(&path) {
                let affected_file = get_source_file_by_path(&path);
                if affected_file.is_nil() || !source_file_may_be_emitted(affected_file, false) {
                    self.deleted_pending_kinds.insert(path);
                    continue;
                }
                let mut snapshot = self.program.snapshot.borrow_mut();
                let pending_kind = snapshot
                    .affected_files_pending_emit
                    .get(&path)
                    .copied()
                    .unwrap_or_default();
                let diagnostics = snapshot
                    .emit_diagnostics_per_file
                    .get_mut(&path)
                    .expect("emit diagnostics entry")
                    .get_diagnostics(affected_file);
                drop(snapshot);
                self.emit_updates.insert(
                    path,
                    EmitUpdate {
                        pending_kind,
                        result: Some(EmitResult {
                            emit_skipped: true,
                            diagnostics,
                            ..EmitResult::default()
                        }),
                        dts_errors_from_cache: true,
                    },
                );
            }
        }

        self.update_snapshot()
    }

    // Go: incremental/emitfileshandler.go:196 getEmitOptions
    // PORT: the callback runs on the checker thread of the emitted file. It
    // gets copies of the file infos and old emit signatures of the files that
    // the emit can write (Go reads them from the snapshot, which does not
    // change during emit). #4699: Go reads them for `data.SourceFile`, not
    // for the target file. Go
    // `h.program.host.GetMTime`/`SetMTime` go through the compiler host file
    // system; the incremental `Host` is not thread-safe, so the callback
    // uses this thread's OS file system (`osvfs_fs`), which is what the
    // compiler host wraps. Without `options.WriteFile`, Go writes with the
    // compiler host file system; the port follows `program::EmitHost`,
    // whose write fails without a callback.
    // PORT: perf. With `Writes::Buffer` or `Writes::Replay` (see `Writes`),
    // the callback keeps each write (and the `differsOnlyInMap` time
    // revert) for `flush_writes`, or gives the flush's result, also without
    // declarations, where Go passes `options` through.
    fn get_emit_options(&self, options: EmitOptions) -> EmitOptions {
        let writer = OutputWriter {
            write_file: options.write_file.clone(),
            writes: self.writes.clone(),
        };
        let snapshot = self.program.snapshot.borrow();
        if !snapshot.options.get_emit_declarations() {
            if matches!(writer.writes, Writes::Direct) {
                return options;
            }
            return EmitOptions {
                write_file: Some(Arc::new(
                    move |file_name: &str, text: &str, data: &mut WriteFileData| {
                        writer.write(file_name, text, data, false, false)
                    },
                )),
                ..options
            };
        }
        let can_use_incremental_state = snapshot.can_use_incremental_state();
        // Only the incremental state reads the files. The emit writes only
        // files of `target_source_files` (Go nil is all files).
        let files: FxHashMap<Path, DtsWriteFile> = if can_use_incremental_state {
            let copy = |file: Node| {
                let path = path_of(file);
                let entry = DtsWriteFile {
                    old_emit_signature: snapshot.emit_signatures.get(&path).cloned(),
                    file_info: snapshot.file_infos.get(&path).cloned(),
                };
                (path, entry)
            };
            match &options.target_source_files {
                Some(target_files) => target_files.iter().copied().map(copy).collect(),
                None => source_files().into_iter().map(copy).collect(),
            }
        } else {
            FxHashMap::default()
        };
        let context = DtsWriteContext {
            composite: snapshot.options.composite.is_true(),
            build: snapshot.options.build.is_true(),
            hash_with_text: snapshot.hash_with_text,
            files,
            shared: Arc::clone(&self.shared),
        };
        drop(snapshot);
        let write_file: WriteFile = Arc::new(
            move |file_name: &str, text: &str, data: &mut WriteFileData| {
                let mut differs_only_in_map = false;
                // PORT: not in Go. True when the signature below read
                // `data.diagnostics` (see `flush_writes`).
                let mut signature_read_diagnostics = false;
                if is_declaration_file_name(file_name) && can_use_incremental_state {
                    let mut emit_signature = String::new();
                    // #4699: the file of `data.SourceFile`.
                    let path = path_of(data.source_file);
                    let file = context
                        .files
                        .get(&path)
                        .expect("the emitted file is an emit target");
                    let info = file
                        .file_info
                        .as_ref()
                        .expect("file info of the emitted file");
                    if info.signature == info.version {
                        signature_read_diagnostics = true;
                        let signature = compute_signature_with_diagnostics(
                            data.source_file,
                            text,
                            data,
                            context.hash_with_text,
                        );
                        // With d.ts diagnostics they are also part of the signature so emitSignature will be different from it since its just hash of d.ts
                        if data.diagnostics.is_empty() {
                            emit_signature = signature.clone();
                        }
                        if signature != info.version {
                            // Update it
                            context
                                .shared
                                .lock()
                                .expect("emit files lock")
                                .signatures
                                .insert(path.clone(), signature);
                        }
                    }

                    // Store d.ts emit hash so later can be compared to check if d.ts has changed.
                    // Currently we do this only for composite projects since these are the only projects that can be referenced by other projects
                    // and would need their d.ts change time in --build mode
                    if skip_dts_output_of_composite(
                        &context,
                        &path,
                        file,
                        file_name,
                        text,
                        data,
                        emit_signature,
                        &mut differs_only_in_map,
                    ) {
                        return Ok(());
                    }
                }

                writer.write(
                    file_name,
                    text,
                    data,
                    differs_only_in_map,
                    signature_read_diagnostics,
                )
            },
        );
        EmitOptions {
            target_source_files: options.target_source_files,
            emit_only: options.emit_only,
            // #4407
            force_emit: options.force_emit,
            write_file: Some(write_file),
        }
    }

    // Go: incremental/emitfileshandler.go:286 updateSnapshot
    fn update_snapshot(&mut self) -> Vec<EmitResult> {
        let mut snapshot = self.program.snapshot.borrow_mut();
        if snapshot.can_use_incremental_state() {
            let shared = std::mem::take(&mut *self.shared.lock().expect("emit files lock"));
            for (file, signature) in shared.signatures {
                let info = snapshot
                    .file_infos
                    .get_mut(&file)
                    .expect("updateSnapshot: file info");
                info.signature = signature;
                if let Some(testing_data) = &self.program.testing_data {
                    testing_data
                        .borrow_mut()
                        .updated_signature_kinds
                        .insert(file.clone(), SignatureUpdateKind::StoredAtEmit);
                }
                snapshot.build_info_emit_pending = true;
            }
            for (file, signature) in shared.emit_signatures {
                snapshot.emit_signatures.insert(file, signature);
                snapshot.build_info_emit_pending = true;
            }
            for file in &self.deleted_pending_kinds {
                snapshot.affected_files_pending_emit.shift_remove(file);
                snapshot.build_info_emit_pending = true;
            }
            // Always use correct order when to collect the result
            let mut results = Vec::new();
            for file in source_files() {
                let path = path_of(file);
                if let Some(latest_changed_dts_file) = shared.latest_changed_dts_files.get(&path) {
                    snapshot.latest_changed_dts_file = latest_changed_dts_file.clone();
                    snapshot.build_info_emit_pending = true;
                    snapshot.has_changed_dts_file = true;
                }
                if let Some(update) = self.emit_updates.get(&path) {
                    if !update.dts_errors_from_cache {
                        if update.pending_kind.is_empty() {
                            snapshot.affected_files_pending_emit.shift_remove(&path);
                        } else {
                            snapshot
                                .affected_files_pending_emit
                                .insert(path.clone(), update.pending_kind);
                        }
                        snapshot.build_info_emit_pending = true;
                    }
                    if let Some(result) = &update.result {
                        results.push(result.clone());
                        if !result.diagnostics.is_empty() {
                            snapshot.emit_diagnostics_per_file.insert(
                                path.clone(),
                                DiagnosticsOrBuildInfoDiagnosticsWithFileName {
                                    diagnostics: Some(result.diagnostics.clone()),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
            return results;
        } else if self.has_emit_diagnostics {
            snapshot.has_emit_diagnostics = true;
        }
        Vec::new()
    }
}

/// What Go `getEmitOptions`' `WriteFile` closure and
/// `skipDtsOutputOfComposite` read through `h`, copied for the files that
/// the emit can write.
struct DtsWriteContext {
    composite: bool,
    build: bool,
    hash_with_text: bool,
    /// The snapshot entries of each file, by path.
    files: FxHashMap<Path, DtsWriteFile>,
    shared: Arc<Mutex<EmitFilesShared>>,
}

/// The snapshot entries of one file in `DtsWriteContext`.
struct DtsWriteFile {
    old_emit_signature: Option<EmitSignature>,
    file_info: Option<FileInfo>,
}

/// PORT: not in Go. Where the `get_emit_options` callback sends a write.
#[derive(Clone)]
enum Writes {
    /// Go: `write_output` at once.
    Direct,
    /// Into the buffer, for `flush_writes`: an early emit
    /// (`buffer_early_emit_writes`, perf), or an emit whose writes only this
    /// thread can make (`flush_writes_on_this_thread`).
    Buffer(WriteBuffer),
    /// The emit again after a failed flush (`replay_after_flush`): the result
    /// that `flush_writes` had for the file name, with no write.
    Replay(Arc<FlushFailures>),
}

/// Where the `get_emit_options` callback writes: `write_file` (Go
/// `options.WriteFile`), as `writes` says.
struct OutputWriter {
    write_file: Option<WriteFile>,
    writes: Writes,
}

impl OutputWriter {
    /// `signature_read_diagnostics`: the callback computed the d.ts
    /// signature from `data.diagnostics` (see `flush_writes`).
    fn write(
        &self,
        file_name: &str,
        text: &str,
        data: &mut WriteFileData,
        differs_only_in_map: bool,
        signature_read_diagnostics: bool,
    ) -> Result<(), String> {
        match &self.writes {
            Writes::Buffer(buffer) => {
                buffer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(BufferedWrite {
                        source: path_of(data.source_file),
                        file_name: file_name.to_string(),
                        text: text.to_string(),
                        data: data.clone(),
                        differs_only_in_map,
                        signature_read_diagnostics,
                    });
                Ok(())
            }
            Writes::Replay(failures) => match failures.get(file_name) {
                None => Ok(()),
                Some(Some(err)) => Err(err.clone()),
                // The flush left it out. Go's callback panics before this
                // write, so does the port's (`diagnostic_to_string_builder`).
                Some(None) => write_output(
                    self.write_file.as_ref(),
                    file_name,
                    text,
                    data,
                    differs_only_in_map,
                ),
            },
            Writes::Direct => write_output(
                self.write_file.as_ref(),
                file_name,
                text,
                data,
                differs_only_in_map,
            ),
        }
    }
}

// Go: the end of the `getEmitOptions` WriteFile callback
// (incremental/emitfileshandler.go:231): the write, and with
// `differsOnlyInMap` the revert of the file's modified time.
fn write_output(
    write_file: Option<&WriteFile>,
    file_name: &str,
    text: &str,
    data: &mut WriteFileData,
    differs_only_in_map: bool,
) -> Result<(), String> {
    let mut a_time = None;
    if differs_only_in_map {
        a_time = osvfs_fs().stat(file_name).and_then(|info| info.mod_time());
    }
    let mut err = match write_file {
        Some(write_file) => write_file(file_name, text, data),
        None => Err(format!("no WriteFile callback for {file_name}")),
    };
    if err.is_ok() && differs_only_in_map {
        // Revert the time to original one
        err = osvfs_fs()
            .chtimes(file_name, None, a_time)
            .map_err(|err| fs_error_text(&err));
    }
    err
}

/// PORT: not in Go. The writes of an emit that `tsc -b` starts before the
/// task's turn to write (`buffer_early_emit_writes`, perf), or of an emit
/// in the API build (`flush_writes_on_this_thread`).
type WriteBuffer = Arc<Mutex<Vec<BufferedWrite>>>;

/// One write in a `WriteBuffer`: the source file whose output it is, the
/// arguments of `write_output`, and `signature_read_diagnostics` (see
/// `OutputWriter::write`).
struct BufferedWrite {
    source: Path,
    file_name: String,
    text: String,
    data: WriteFileData,
    differs_only_in_map: bool,
    signature_read_diagnostics: bool,
}

/// What `flush_writes` did not write, by file name: the error text of each
/// failed write, and `None` for each write that it left out. Every other
/// write succeeded, or was not made because the replay panics first
/// (`flush_client_writes`).
type FlushFailures = FxHashMap<String, Option<String>>;

/// PORT: not in Go. How the writes of the API build reach the client when
/// each one is a `writeFile` call (`--callbacks writeFile`). Go's emit
/// goroutines make the calls, so which outputs exist when a d.ts signature
/// panics (`diagnostic_to_string_builder`) depends on the calls
/// (`flush_client_writes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientWrites {
    /// The calls run at the same time (`--async`, Go `AsyncConn.Call`).
    Concurrent,
    /// One call at a time: Go `SyncConn.Call` (msgpack) holds a mutex for
    /// each call (ipc/conn_sync.go:170).
    Serialized,
}

thread_local! {
    /// True while `buffer_early_emit_writes` runs its `start`.
    static BUFFER_EARLY_EMIT_WRITES: Cell<bool> = const { Cell::new(false) };
    /// True while `flush_writes_on_this_thread` runs its `f`.
    static FLUSH_ON_THIS_THREAD: Cell<bool> = const { Cell::new(false) };
    /// The `client_writes` of `flush_writes_on_this_thread` while it runs.
    static CLIENT_WRITES: Cell<Option<ClientWrites>> = const { Cell::new(None) };
}

/// PORT: not in Go (perf). Runs `start`, a `Program::start_emit` call, so
/// that the emit that it starts keeps its writes in memory
/// (`WriteBuffer`) until `finish_emit_files` writes them (`flush_writes`).
/// `tsc -b` starts a task's emit when it makes the task's program, so each
/// program emits right after its check, and its outputs reach the file
/// system only when the orchestrator finishes the task
/// (`BuildTask::compile_and_emit_start`; `build_all_tasks` gives the
/// order).
pub(crate) fn buffer_early_emit_writes(start: impl FnOnce()) {
    /// Puts back the flag of the caller when `start` ends, also when it
    /// panics (`tsc -b` keeps a task's panic and goes on). So a nested call
    /// does not end the buffering of the call around it.
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            BUFFER_EARLY_EMIT_WRITES.set(self.0);
        }
    }
    let _reset = Reset(BUFFER_EARLY_EMIT_WRITES.replace(true));
    start();
}

/// PORT: not in Go. Runs `f` so that `flush_writes` writes on this thread
/// only, and so that an emit that did not start early keeps its writes
/// for `flush_writes` too (`EmitFilesHandler::emit_with_writes_here`). The
/// API build runs a task's emit under it: only its orchestrator thread
/// reaches the file system (`System::emit_writes_through_osvfs`), so a
/// write on another thread would wait (build_task.rs `DeferredWrites`), and
/// the emit would not get its result. `client_writes` is
/// `System::client_writes`.
pub(crate) fn flush_writes_on_this_thread<R>(
    client_writes: Option<ClientWrites>,
    f: impl FnOnce() -> R,
) -> R {
    /// Puts back the values of the caller when `f` ends, also on a panic.
    struct Reset(bool, Option<ClientWrites>);
    impl Drop for Reset {
        fn drop(&mut self) {
            FLUSH_ON_THIS_THREAD.set(self.0);
            CLIENT_WRITES.set(self.1);
        }
    }
    let _reset = Reset(
        FLUSH_ON_THIS_THREAD.replace(true),
        CLIENT_WRITES.replace(client_writes),
    );
    f()
}

/// The most threads that `flush_writes` writes on.
const MAX_FLUSH_THREADS: usize = 8;

/// Writes the buffered `writes` of an emit of the files in `order` (their
/// paths, in emit order) with `write_file` (`write_output`), each once. Go
/// writes each file's outputs on the goroutine that emits it: the source
/// map, then the JS, then the declaration map, then the declaration; a
/// failed write does not stop the others. Here the files' outputs go in
/// that order, the files in `order`, on up to `MAX_FLUSH_THREADS` threads
/// (1 under `flush_writes_on_this_thread`). `single_threaded` is the
/// emitting program's `--singleThreaded`. Returns what it did not write
/// when a write failed (`FlushFailures`), else `None`.
///
/// After a failed write of a file, Go's d.ts callback has the TS5033 in
/// `data.Diagnostics`. When the callback computes the signature from them
/// (`signature_read_diagnostics`), Go panics there
/// (`diagnostic_to_string_builder`) before the write, so this leaves the
/// write out. In the API build with client calls the writes go as
/// `flush_client_writes` says.
fn flush_writes(
    mut writes: Vec<BufferedWrite>,
    order: &[Path],
    write_file: Option<&WriteFile>,
    single_threaded: bool,
) -> Option<FlushFailures> {
    let order: FxHashMap<&Path, usize> = order
        .iter()
        .enumerate()
        .map(|(index, path)| (path, index))
        .collect();
    // A file's JS and declaration can emit on two threads, each in order.
    let key = |write: &BufferedWrite| {
        (
            order.get(&write.source).copied(),
            is_declaration_output(write),
        )
    };
    crate::gostd::slices::stable_sort_by(&mut writes, |a, b| key(a).cmp(&key(b)));
    if let Some(calls) = CLIENT_WRITES.get()
        && !single_threaded
    {
        return flush_client_writes(writes, calls, write_file);
    }
    let files: Vec<&mut [BufferedWrite]> =
        writes.chunk_by_mut(|a, b| a.source == b.source).collect();
    let threads = if FLUSH_ON_THIS_THREAD.get() {
        1
    } else {
        MAX_FLUSH_THREADS
            .min(crate::program::available_cores())
            .min(files.len().div_ceil(16))
            .max(1)
    };
    let files = Mutex::new(files.into_iter());
    let failures = Mutex::new(FlushFailures::default());
    let work = || {
        loop {
            let next = files.lock().unwrap_or_else(PoisonError::into_inner).next();
            let Some(file) = next else {
                break;
            };
            let mut failed = false;
            for write in file {
                let result = if failed && write.signature_read_diagnostics {
                    None
                } else {
                    match write_output(
                        write_file,
                        &write.file_name,
                        &write.text,
                        &mut write.data,
                        write.differs_only_in_map,
                    ) {
                        Ok(()) => continue,
                        Err(err) => Some(err),
                    }
                };
                failed = true;
                failures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(write.file_name.clone(), result);
            }
        }
    };
    std::thread::scope(|scope| {
        // Port-only threads: the caller writes too, so a thread that cannot
        // start leaves its files to the threads that run (no exit, as for
        // the parse and config prefetch threads).
        for _ in 1..threads {
            if std::thread::Builder::new()
                .spawn_scoped(scope, &work)
                .is_err()
            {
                break;
            }
        }
        work();
    });
    let failures = failures
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    (!failures.is_empty()).then_some(failures)
}

/// True for a declaration output or its map.
fn is_declaration_output(write: &BufferedWrite) -> bool {
    let output = write.file_name.as_str();
    is_declaration_file_name(output.strip_suffix(".map").unwrap_or(output))
}

/// `flush_writes` in the API build when each write is a client `writeFile`
/// call (`calls`), without `--singleThreaded`. `writes` are in file order,
/// each file's outputs in Go's order. Go's emit goroutines make the calls
/// in parallel. A d.ts signature that reads the TS5033 of an earlier
/// failed write of its file (`signature_read_diagnostics`) panics and ends
/// the process during the other goroutines' calls, so Go makes some of
/// their calls and writes some of their outputs, by timing. This makes
/// calls and writes that Go N' often makes (gopanicout1, gopanicout2 and
/// gopanicout3 `go-model.md`, the client's call set and the written files
/// are one of Go's results in 30 to 190 runs per shape):
/// - The outputs go in rounds: the first output of each file that comes
///   before its declarations, then the second..., then the declaration
///   outputs in rounds. A file's goroutine writes its JS and maps before
///   its declarations.
/// - After a failed write of a file whose d.ts then panics (doomed), with
///   `Concurrent` calls: the rest of the outputs before the declarations
///   and, for a failed declaration output, the rest of its round (the
///   other files' calls are in flight), then the doomed file's own
///   outputs. Go's panic needs no more answers, and another file's next
///   output needs one more.
/// - With `Serialized` calls (`flush_serialized_client_writes`): in the
///   round of the failure the next file's output is written too, and in
///   the first round every file's output is called; then the doomed
///   file's own outputs. A failed OS write after a `useOS` answer dooms
///   its file one round later.
/// - No other write is made: the replay panics at the doomed d.ts, and the
///   process ends (`program_emit::wait_emit_jobs`).
///
/// Without a doomed file every write is made, in rounds.
fn flush_client_writes(
    writes: Vec<BufferedWrite>,
    calls: ClientWrites,
    write_file: Option<&WriteFile>,
) -> Option<FlushFailures> {
    // (declaration, the output's place among its file's outputs of that
    // kind, the file), and the write.
    let mut rounds = Vec::with_capacity(writes.len());
    let mut file = 0;
    let mut place = [0; 2];
    let mut previous: Option<Path> = None;
    for write in writes {
        if previous.as_ref() != Some(&write.source) {
            if previous.is_some() {
                file += 1;
            }
            place = [0; 2];
            previous = Some(write.source.clone());
        }
        let declaration = is_declaration_output(&write);
        rounds.push(((declaration, place[usize::from(declaration)], file), write));
        place[usize::from(declaration)] += 1;
    }
    crate::gostd::slices::stable_sort_by(&mut rounds, |a, b| a.0.cmp(&b.0));
    let mut flush = ClientFlush {
        // The files whose d.ts signature reads `data.diagnostics`.
        reads_diagnostics: rounds
            .iter()
            .filter(|(_, write)| write.signature_read_diagnostics)
            .map(|&((.., file), _)| file)
            .collect(),
        write_file,
        failed: FxHashSet::default(),
        doomed: None,
        failures: FlushFailures::default(),
    };
    match calls {
        ClientWrites::Concurrent => {
            let mut doomed_round = None;
            for ((declaration, place, file), write) in rounds {
                let made = match (flush.doomed, doomed_round) {
                    (Some(doomed), Some(round)) => {
                        !declaration || file == doomed || (declaration, place) == round
                    }
                    _ => true,
                };
                if made && flush.write(file, write) {
                    doomed_round = Some((declaration, place));
                }
            }
        }
        ClientWrites::Serialized => flush_serialized_client_writes(rounds, &mut flush),
    }
    (!flush.failures.is_empty()).then_some(flush.failures)
}

/// The state of `flush_client_writes`.
struct ClientFlush<'a> {
    reads_diagnostics: FxHashSet<usize>,
    write_file: Option<&'a WriteFile>,
    /// The files with a failed output.
    failed: FxHashSet<usize>,
    /// The file whose d.ts panics.
    doomed: Option<usize>,
    failures: FlushFailures,
}

impl ClientFlush<'_> {
    /// Makes the write of an output of `file` and records a failure (`fail`).
    /// True when this failure dooms the file.
    fn write(&mut self, file: usize, mut write: BufferedWrite) -> bool {
        let result = if self.failed.contains(&file) && write.signature_read_diagnostics {
            None
        } else {
            match write_output(
                self.write_file,
                &write.file_name,
                &write.text,
                &mut write.data,
                write.differs_only_in_map,
            ) {
                Ok(()) => return false,
                Err(err) => Some(err),
            }
        };
        self.fail(
            file,
            write.file_name,
            result,
            write.signature_read_diagnostics,
        )
    }

    /// Records a write of an output of `file` that failed with `result`, or
    /// that was left out (`None`). True when this failure dooms the file: a
    /// failed output before the file's d.ts (not the d.ts itself, whose
    /// signature comes before its write, `signature_read_diagnostics`).
    fn fail(
        &mut self,
        file: usize,
        file_name: String,
        result: Option<String>,
        signature_read_diagnostics: bool,
    ) -> bool {
        let dooms = self.doomed.is_none()
            && result.is_some()
            && !signature_read_diagnostics
            && self.reads_diagnostics.contains(&file);
        if dooms {
            self.doomed = Some(file);
        }
        self.failed.insert(file);
        self.failures.insert(file_name, result);
        dooms
    }
}

/// `flush_client_writes` with `Serialized` calls. Go `SyncConn.Call` takes
/// one mutex for each call (ipc/conn_sync.go:170), so the emit goroutines
/// take turns. The OS writes of a round wait until the round's calls end
/// (`defer_os_writes`). After a failure that dooms a file in the round,
/// the next file (after the doomed one, in file order, the first after the
/// last) writes its output of the round and the process ends:
/// - in the first round every file's first output is called, and only the
///   doomed file's and the next file's are written;
/// - in a later round only the next file's output is called after the
///   failure, and the earlier writes of the round are lost.
///
/// Then only the doomed file's own outputs are made.
///
/// A failed OS write after a `useOS` answer (gopanicout3 `go-model.md`) is
/// slower than a failed call: Go tries the write again after it makes the
/// directory (vfs/osvfs/os.go:161 `writeFileEnsuringDir`). So it dooms its
/// file after the round's OS writes, and in the next round the doomed
/// file's output is called and written, and the next file's output is
/// called without its write. Then only the doomed file's own outputs.
fn flush_serialized_client_writes(
    rounds: Vec<((bool, usize, usize), BufferedWrite)>,
    flush: &mut ClientFlush<'_>,
) {
    let files = rounds
        .iter()
        .map(|&((.., file), _)| file + 1)
        .max()
        .unwrap_or(0);
    let mut rounds = rounds.into_iter().peekable();
    let mut first_round = true;
    // A failed OS write of the last round doomed its file.
    let mut doomed_by_os_write = false;
    while let Some(&((declaration, place, _), _)) = rounds.peek() {
        let mut round = Vec::new();
        while let Some(((_, _, file), write)) =
            rounds.next_if(|&((d, p, _), _)| (d, p) == (declaration, place))
        {
            round.push((file, write));
        }
        if let Some(doomed) = flush.doomed {
            for (file, write) in round {
                if file == doomed {
                    flush.write(file, write);
                } else if doomed_by_os_write && file == (doomed + 1) % files {
                    // The process ends before this OS write.
                    drop(defer_os_writes(|| flush.write(file, write)));
                }
            }
            doomed_by_os_write = false;
            continue;
        }
        let mut held = Vec::new();
        for (file, write) in round {
            let called = match flush.doomed {
                None => true,
                Some(doomed) => first_round || file == (doomed + 1) % files,
            };
            if called {
                let name = write.file_name.clone();
                let signature_read_diagnostics = write.signature_read_diagnostics;
                let os_writes = defer_os_writes(|| flush.write(file, write));
                held.push((file, name, signature_read_diagnostics, os_writes));
            }
        }
        // The doom of a failed call of the round.
        let doomed_by_call = flush.doomed;
        for (file, name, signature_read_diagnostics, os_writes) in held {
            let written =
                doomed_by_call.is_none_or(|doomed| file == doomed || file == (doomed + 1) % files);
            if !written {
                continue;
            }
            for os_write in os_writes {
                if let Err(err) = os_write() {
                    doomed_by_os_write |= flush.fail(
                        file,
                        name.clone(),
                        Some(fs_error_text(&err)),
                        signature_read_diagnostics,
                    );
                }
            }
        }
        first_round = false;
    }
}

/// An OS write that `defer_os_writes` holds back.
pub type DeferredOsWrite = Box<dyn FnOnce() -> Result<(), FsError>>;

thread_local! {
    /// The OS writes that the running `defer_os_writes` holds back.
    static DEFERRED_OS_WRITES: std::cell::RefCell<Option<Vec<DeferredOsWrite>>> =
        const { std::cell::RefCell::new(None) };
}

/// PORT: not in Go. Runs `f`, a client `writeFile` call, and returns the
/// OS write that the client asked for (`useOS`) instead of making it
/// (`os_write_or_defer`): `flush_serialized_client_writes` makes it only
/// when Go's goroutine would have written before the panic ends the
/// process.
fn defer_os_writes(f: impl FnOnce() -> bool) -> Vec<DeferredOsWrite> {
    /// Puts back the deferral of the caller when `f` ends, also when it
    /// panics (as `buffer_early_emit_writes`). After a caught panic, a
    /// `useOS` write on this thread is made at once, and the writes that
    /// `f` held are dropped: Go's panic ends the process before them.
    struct Restore(Option<Vec<DeferredOsWrite>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let outer = self.0.take();
            DEFERRED_OS_WRITES.with(|deferred| *deferred.borrow_mut() = outer);
        }
    }
    let restore =
        Restore(DEFERRED_OS_WRITES.with(|deferred| deferred.borrow_mut().replace(Vec::new())));
    f();
    let writes = DEFERRED_OS_WRITES
        .with(|deferred| deferred.borrow_mut().take())
        .unwrap_or_default();
    drop(restore);
    writes
}

/// The OS write of a `writeFile` call that the client answered with
/// `useOS` (api/callbackfs.go:415 `WriteFile`): made now, or held back
/// while `defer_os_writes` runs (then `Ok`).
pub fn os_write_or_defer(
    os_write: impl FnOnce() -> Result<(), FsError> + 'static,
) -> Result<(), FsError> {
    let os_write = DEFERRED_OS_WRITES.with(|deferred| match deferred.borrow_mut().as_mut() {
        Some(writes) => {
            writes.push(Box::new(os_write));
            None
        }
        None => Some(os_write),
    });
    match os_write {
        Some(os_write) => os_write(),
        None => Ok(()),
    }
}

// Go: incremental/emitfileshandler.go:252 skipDtsOutputOfComposite
// Compare to existing computed signature and store it or handle the changes in d.ts map option from before
// returning undefined means that, we dont need to emit this d.ts file since its contents didnt change
// PORT: Go `file` is `path` (its path) and `entry` (its snapshot entries).
fn skip_dts_output_of_composite(
    context: &DtsWriteContext,
    path: &Path,
    entry: &DtsWriteFile,
    output_file_name: &str,
    text: &str,
    data: &mut WriteFileData,
    mut new_signature: String,
    differs_only_in_map: &mut bool,
) -> bool {
    if !context.composite {
        return false;
    }
    let mut old_signature = String::new();
    let old_signature_format = entry.old_emit_signature.as_ref();
    if let Some(format) = old_signature_format {
        if !format.signature.is_empty() {
            old_signature = format.signature.clone();
        } else {
            old_signature = format
                .signature_with_different_options
                .as_ref()
                .expect("signatureWithDifferentOptions")[0]
                .clone();
        }
    }
    if new_signature.is_empty() {
        new_signature = compute_hash(
            get_text_handling_source_map_for_signature(text, data),
            context.hash_with_text,
        );
    }
    let mut shared = context.shared.lock().expect("emit files lock");
    // Dont write dts files if they didn't change
    if new_signature == old_signature {
        // If the signature was encoded as string the dts map options match so nothing to do
        if old_signature_format.is_some_and(|format| format.signature == old_signature) {
            data.skipped_dts_write = true;
            return true;
        } else {
            // Mark as differsOnlyInMap so that we can reverse the timestamp with --build so that
            // the downstream projects dont detect this as change in d.ts file
            *differs_only_in_map = context.build;
        }
    } else {
        shared
            .latest_changed_dts_files
            .insert(path.clone(), output_file_name.to_string());
    }
    shared.emit_signatures.insert(
        path.clone(),
        EmitSignature {
            signature: new_signature,
            signature_with_different_options: None,
        },
    );
    false
}

// Go: incremental/emitfileshandler.go:338 emitFiles
#[must_use]
pub fn emit_files(program: &Program, options: EmitOptions, is_for_dts_errors: bool) -> EmitResult {
    let mut emit_handler = EmitFilesHandler::new(program, is_for_dts_errors);

    // Single file emit - do direct from program
    if !is_for_dts_errors && options.target_source_files.is_some() {
        let emit_options = emit_handler.get_emit_options(options);
        let result = emit(emit_options);
        emit_handler.update_has_emit_diagnostics(Some(&result));
        emit_handler.update_snapshot();
        return result;
    }

    // Emit only affected files if using builder for emit
    emit_handler.emit_all_affected_files(options)
}

/// PORT: not in Go (perf). The emit that `start_emit_files` sent:
/// `emit_files` of all affected files (no target file, `EmitOnly::All`)
/// up to the wait for the emit jobs, and the handler state that the rest
/// needs.
pub(crate) struct StartedEmit {
    shared: Arc<Mutex<EmitFilesShared>>,
    deleted_pending_kinds: FxIndexSet<Path>,
    queued: Vec<QueuedEmit>,
    batch: PendingEmitBatch,
    /// The `WriteFile` of the start. `finish_emit_files` must get the same.
    write_file: Option<WriteFile>,
    /// The writes of the emit, with `buffer_early_emit_writes`.
    buffer: Option<WriteBuffer>,
    /// `program::bind_thread_fingerprint` of this thread after the start.
    /// The emit pool starts from a copy of this thread's state, so between
    /// the start and the finish this thread must not change it, else the
    /// pool would start from other state than without the early start.
    /// `None` with `buffer_early_emit_writes`: `tsc -b` makes other
    /// programs on this thread meanwhile, and their state is not the
    /// state of this program.
    fingerprint: Option<(usize, (u64, u64), u64, usize)>,
}

/// PORT: not in Go (perf). The first half of `emit_files(program, options,
/// false)` for `Program::start_emit`: pass 1 of
/// `emit_files_incremental`, then the emit jobs are sent without a wait.
/// Each checker thread runs them after the jobs sent to it before (the
/// check). The caller has the
/// incremental state (`can_use_incremental_state`), and `options` name no
/// target file and emit all.
///
/// Pass 1 reads the pending emit set, the file infos, the emit signatures
/// and the options, and `get_emit_options` copies them into the write
/// callbacks. The check commit (`Program::commit_semantic_diagnostics`)
/// changes none of them, so the jobs are the jobs that `emit_files` would
/// send after the check. When `start_check` sent a check, the
/// affected-file walk (`collect_all_affected_files`) ran there, and its run
/// here is a no-op. When it did not (`noCheck`, or syntactic, program or
/// global diagnostics), the walk runs here first, as Go's runs in `Emit`.
pub(crate) fn start_emit_files(program: &Program, options: EmitOptions) -> StartedEmit {
    debug_assert!(
        program.snapshot.borrow().can_use_incremental_state()
            && options.target_source_files.is_none()
            && options.emit_only == EmitOnly::All,
        "start_emit_files: only the emit of all affected files starts early"
    );
    let mut handler = EmitFilesHandler::new(program, false);
    let buffer = BUFFER_EARLY_EMIT_WRITES.get().then(WriteBuffer::default);
    if let Some(buffer) = &buffer {
        handler.writes = Writes::Buffer(buffer.clone());
    }
    let queued = handler.queue_affected_files(&options);
    let batch = handler.send_emit_batch(&queued, &options);
    StartedEmit {
        shared: handler.shared,
        deleted_pending_kinds: handler.deleted_pending_kinds,
        queued,
        batch,
        write_file: options.write_file,
        fingerprint: buffer
            .is_none()
            .then(crate::program::bind_thread_fingerprint),
        buffer,
    }
}

/// PORT: not in Go (perf). The second half of `emit_files(program,
/// options, false)` for the emit that `start_emit_files` sent: waits for
/// the emit jobs, then does the rest of `emit_files_incremental` and
/// `emitAllAffectedFiles` (the snapshot update and the build info).
/// `options` must be the options of the start.
///
/// With `buffer_early_emit_writes` it writes the emit's outputs first
/// (`flush_writes`). When a write fails, Go's emitter sees the error at
/// the write: the file gets a TS5033 diagnostic instead of the output in
/// `EmittedFiles`, and the declaration signature takes the diagnostic. So
/// then the files emit again from the start state, and each write gives
/// the flush's result without a write (`Writes::Replay`; Go writes each
/// file once). Their results replace the buffered emit's.
pub(crate) fn finish_emit_files(
    program: &Program,
    started: StartedEmit,
    options: &EmitOptions,
) -> EmitResult {
    debug_assert!(
        options.target_source_files.is_none()
            && options.emit_only == EmitOnly::All
            && match (&options.write_file, &started.write_file) {
                (Some(write_file), Some(started_write_file)) => {
                    Arc::ptr_eq(write_file, started_write_file)
                }
                (None, None) => true,
                _ => false,
            },
        "finish_emit_files: the emit options differ from the started ones"
    );
    if let Some(fingerprint) = started.fingerprint {
        debug_assert_eq!(
            crate::program::bind_thread_fingerprint(),
            fingerprint,
            "the loading thread changed its synthetic nodes, ids or lazy JSDoc during the early emit"
        );
    }
    let mut handler = EmitFilesHandler {
        program,
        is_for_dts_errors: false,
        shared: started.shared,
        deleted_pending_kinds: started.deleted_pending_kinds,
        emit_updates: IndexMap::default(),
        has_emit_diagnostics: false,
        writes: Writes::Direct,
    };
    let mut results = started.batch.wait();
    if let Some(buffer) = started.buffer {
        let writes = std::mem::take(&mut *buffer.lock().unwrap_or_else(PoisonError::into_inner));
        let order: Vec<Path> = started
            .queued
            .iter()
            .map(|(path, ..)| path.clone())
            .collect();
        if handler.replay_after_flush(writes, &order, options.write_file.as_ref()) {
            results = handler.send_emit_batch(&started.queued, options).wait();
        }
    }
    let results = handler.finish_emit_files_incremental(started.queued, results);
    handler.combine_results_and_emit_build_info(results, options)
}

#[cfg(test)]
mod tests {
    use super::{
        BUFFER_EARLY_EMIT_WRITES, buffer_early_emit_writes, defer_os_writes, os_write_or_defer,
    };
    use crate::frontend::vfs::FsError;
    use std::cell::Cell;
    use std::rc::Rc;

    /// followups42: a panic inside `defer_os_writes` ends the deferral. A
    /// caller that catches it (`tsc -b` keeps a task's panic and goes on)
    /// would else hold every later `useOS` write on this thread for a
    /// round that never ends.
    #[test]
    fn defer_os_writes_ends_on_a_panic() {
        let result = std::panic::catch_unwind(|| {
            defer_os_writes(|| {
                let held = os_write_or_defer(|| Err(FsError::Closed));
                assert!(held.is_ok(), "the write is held");
                panic!("the call panics");
            })
        });
        assert!(result.is_err(), "the panic reaches the caller");
        let made = Rc::new(Cell::new(false));
        let write = made.clone();
        let result = os_write_or_defer(move || {
            write.set(true);
            Ok(())
        });
        assert!(result.is_ok() && made.get(), "the write is made at once");
    }

    /// k2gaps1: a panic inside the buffered start ends the buffering. `tsc -b`
    /// keeps a task's panic and goes on, so a flag left on would make every
    /// later emit on this thread keep its writes in a buffer.
    #[test]
    fn buffer_early_emit_writes_ends_on_a_panic() {
        let result = std::panic::catch_unwind(|| {
            buffer_early_emit_writes(|| {
                assert!(BUFFER_EARLY_EMIT_WRITES.get(), "the start buffers");
                panic!("the start panics");
            });
        });
        assert!(result.is_err(), "the panic reaches the caller");
        assert!(
            !BUFFER_EARLY_EMIT_WRITES.get(),
            "the buffering ended with the panic"
        );
    }

    /// followups39: a nested call puts back the flag of the call around it,
    /// also when the nested start panics, so the outer start still buffers.
    #[test]
    fn buffer_early_emit_writes_keeps_the_outer_buffering() {
        buffer_early_emit_writes(|| {
            buffer_early_emit_writes(|| {
                assert!(BUFFER_EARLY_EMIT_WRITES.get(), "the inner start buffers");
            });
            assert!(
                BUFFER_EARLY_EMIT_WRITES.get(),
                "the outer start buffers after the inner call"
            );
            let result = std::panic::catch_unwind(|| {
                buffer_early_emit_writes(|| panic!("the inner start panics"));
            });
            assert!(result.is_err(), "the panic reaches the outer start");
            assert!(
                BUFFER_EARLY_EMIT_WRITES.get(),
                "the outer start buffers after the inner panic"
            );
        });
        assert!(
            !BUFFER_EARLY_EMIT_WRITES.get(),
            "the buffering ended with the outer call"
        );
    }
}
