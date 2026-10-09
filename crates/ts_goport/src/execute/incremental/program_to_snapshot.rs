//! Port of execute/incremental/programtosnapshot.go.
//!
//! PORT: Go `*compiler.Program` is the current program (`prog()`), read
//! through the `program.rs` free functions. Go runs the per-file work of
//! `computeProgramFileChanges` on a work group; here the checker part of
//! every file is sent to its checker thread first
//! (`start_referenced_files_job`), and the rest runs in file order.

use super::checker_access::*;
use super::hash::FileInfo;
use super::hash::*;
use super::program::Program;
use super::snapshot::*;
use crate::frontend::prelude::*;
use std::sync::Arc;

// Go: incremental/programtosnapshot.go:16 programToSnapshot
#[must_use]
pub fn program_to_snapshot(
    old_program: Option<&Program>,
    hash_with_text: bool,
) -> Rc<RefCell<Snapshot>> {
    let program = prog();
    if let Some(old_program) = old_program {
        if old_program
            .program
            .is_some_and(|old| std::ptr::eq(old, program))
        {
            return old_program.snapshot.clone();
        }
    }
    let mut snapshot = Snapshot::new(options());
    snapshot.hash_with_text = hash_with_text;
    snapshot.check_pending = options().no_check.is_true();
    let mut to = ToProgramSnapshot {
        old_program,
        snapshot,
        global_file_removed: false,
    };

    if to.snapshot.can_use_incremental_state() {
        to.reuse_from_old_program();
        to.compute_program_file_changes();
        to.handle_file_delete();
        to.handle_global_scope_change();
        to.handle_pending_emit();
        to.handle_pending_check();
    }
    Rc::new(RefCell::new(to.snapshot))
}

// Go: incremental/programtosnapshot.go:42 toProgramSnapshot
// PORT: Go `program` is the current program and is not a field.
struct ToProgramSnapshot<'a> {
    old_program: Option<&'a Program>,
    snapshot: Snapshot,
    global_file_removed: bool,
}

impl ToProgramSnapshot<'_> {
    // Go: incremental/programtosnapshot.go:49 reuseFromOldProgram
    fn reuse_from_old_program(&mut self) {
        if let Some(old_program) = self.old_program {
            let old_snapshot = old_program.snapshot.borrow();
            if self.snapshot.options.composite.is_true() {
                self.snapshot.latest_changed_dts_file =
                    old_snapshot.latest_changed_dts_file.clone();
            }
            // Copy old snapshot's changed files set
            for key in &old_snapshot.changed_files_set {
                self.snapshot.changed_files_set.insert(key.clone());
            }
            for (key, emit_kind) in &old_snapshot.affected_files_pending_emit {
                self.snapshot
                    .affected_files_pending_emit
                    .insert(key.clone(), *emit_kind);
            }
            self.snapshot.build_info_emit_pending = old_snapshot.build_info_emit_pending;
            self.snapshot.has_errors_from_old_state = old_snapshot.has_errors;
            self.snapshot.has_semantic_errors_from_old_state = old_snapshot.has_semantic_errors;
            self.snapshot.package_jsons_from_old_state =
                old_snapshot.package_jsons.clone().unwrap_or_default();
            self.snapshot.missing_package_jsons_from_old_state = old_snapshot
                .missing_package_jsons
                .clone()
                .unwrap_or_default();
        } else {
            self.snapshot.build_info_emit_pending = self.snapshot.options.is_incremental();
        }
    }

    // Go: incremental/programtosnapshot.go:73 computeProgramFileChanges
    fn compute_program_file_changes(&mut self) {
        let old_snapshot_ref = self.old_program.map(|old| old.snapshot.borrow());
        let old_snapshot = old_snapshot_ref.as_deref();
        let can_copy_semantic_diagnostics = old_snapshot.is_some_and(|old| {
            !compiler_options_affect_semantic_diagnostics(Some(old.options), Some(options()))
        });
        // We can only reuse emit signatures (i.e. .d.ts signatures) if the .d.ts file is unchanged,
        // which will eg be depedent on change in options like declarationDir and outDir options are unchanged.
        // We need to look in oldState.compilerOptions, rather than oldCompilerOptions (i.e.we need to disregard useOldState) because
        // oldCompilerOptions can be undefined if there was change in say module from None to some other option
        // which would make useOldState as false since we can now use reference maps that are needed to track what to emit, what to check etc
        // but that option change does not affect d.ts file name so emitSignatures should still be reused.
        let can_copy_emit_signatures = self.snapshot.options.composite.is_true()
            && old_snapshot.is_some_and(|old| {
                !compiler_options_affect_declaration_path(Some(old.options), Some(options()))
            });
        let copy_declaration_file_diagnostics = can_copy_semantic_diagnostics
            && old_snapshot.is_some_and(|old| {
                self.snapshot.options.skip_lib_check.is_true()
                    == old.options.skip_lib_check.is_true()
            });
        let copy_lib_file_diagnostics = copy_declaration_file_diagnostics
            && old_snapshot.is_some_and(|old| {
                self.snapshot.options.skip_default_lib_check.is_true()
                    == old.options.skip_default_lib_check.is_true()
            });

        // PORT: perf. Go looks up each referenced path of each unchanged
        // file in the new program, to find a referenced file that the new
        // program deleted. Only a path of the old snapshot can match, so
        // those paths are looked up once here: when the new program has all
        // of them (a watch rebuild of changed file texts), no file can have
        // a deleted reference and the loop below skips the lookups.
        let old_file_deleted = old_snapshot.is_some_and(|old| {
            old.file_infos
                .keys()
                .any(|path| get_source_file_by_path(path).is_nil())
        });

        let files = source_files();
        // PORT: perf. Go hashes each file text in the file's WorkGroup job.
        // Here another thread hashes the texts while the files bind and the
        // checkers start (`start_text_hashes`), and the loop takes each hash
        // in file order. The hash values do not depend on the thread.
        let versions = start_text_hashes(&files, self.snapshot.hash_with_text);
        // PORT: perf. Go runs this loop in a WorkGroup. `getReferencedFiles`
        // runs on the file's checker thread (`start_referenced_files_job`),
        // so the jobs of every file are sent first and run in parallel; the
        // loop takes each result in file order. Each checker still gets its files in
        // the same order. Binding comes first, as in the first iteration of
        // the loop (`file_affects_global_scope`).
        bind_all();
        // PORT: perf. Each job gets the file's set of the old snapshot, and
        // returns that set when the new set is the same (see
        // `ReferencedFileSet::finish`).
        let mut reference_jobs: std::collections::VecDeque<_> = files
            .iter()
            .map(|&file| {
                let old = old_snapshot.and_then(|old| {
                    old.referenced_map
                        .get_references_arc(source_file_info(file).path.as_str())
                });
                start_referenced_files_job(file, old)
            })
            .collect();
        for file in files {
            let file_path = Path(source_file_info(file).path.clone());
            let version = versions.recv().expect("one text hash per file");
            let implied_node_format = get_source_file_meta_data(&file_path).implied_node_format;
            let affects_global_scope = file_affects_global_scope(file);
            let mut signature = String::new();
            // PORT: Go stores the `newReferences` pointer and still reads it
            // below. `Arc` shares the set in the same way, without a copy.
            let new_references = reference_jobs
                .pop_front()
                .expect("one referenced files job per file")
                .wait();
            if let Some(new_references) = &new_references {
                self.snapshot
                    .referenced_map
                    .store_references(file_path.clone(), Arc::clone(new_references));
            }
            if let Some(old_snapshot) = old_snapshot {
                if let Some(old_file_info) = old_snapshot.file_infos.get(&file_path) {
                    signature = old_file_info.signature.clone();
                    if old_file_info.version != version
                        || old_file_info.affects_global_scope != affects_global_scope
                        || old_file_info.implied_node_format != implied_node_format
                    {
                        self.snapshot.add_file_to_change_set(file_path.clone());
                    } else if !same_references(
                        new_references.as_deref(),
                        old_snapshot.referenced_map.get_references(&file_path),
                    ) {
                        // Referenced files changed
                        self.snapshot.add_file_to_change_set(file_path.clone());
                    } else if let Some(new_references) =
                        new_references.as_ref().filter(|_| old_file_deleted)
                    {
                        for ref_path in new_references.iter() {
                            if get_source_file_by_path(ref_path).is_nil()
                                && old_snapshot.file_infos.contains_key(ref_path)
                            {
                                // Referenced file was deleted in the new program
                                self.snapshot.add_file_to_change_set(file_path.clone());
                                break;
                            }
                        }
                    }
                } else {
                    self.snapshot.add_file_to_change_set(file_path.clone());
                }
                if !self.snapshot.changed_files_set.contains(&file_path) {
                    if let Some(emit_diagnostics) =
                        old_snapshot.emit_diagnostics_per_file.get(&file_path)
                    {
                        self.snapshot.emit_diagnostics_per_file.insert(
                            file_path.clone(),
                            repopulate_diagnostics_of_file(emit_diagnostics, file),
                        );
                    }
                    if can_copy_semantic_diagnostics
                        && (!source_file_info(file).is_declaration_file
                            || copy_declaration_file_diagnostics)
                        && (!is_source_file_default_library(&file_path)
                            || copy_lib_file_diagnostics)
                    {
                        // Unchanged file copy diagnostics
                        if let Some(diagnostics) =
                            old_snapshot.semantic_diagnostics_per_file.get(&file_path)
                        {
                            self.snapshot.semantic_diagnostics_per_file.insert(
                                file_path.clone(),
                                repopulate_diagnostics_of_file(diagnostics, file),
                            );
                        }
                    }
                }
                if can_copy_emit_signatures {
                    if let Some(old_emit_signature) = old_snapshot.emit_signatures.get(&file_path) {
                        self.snapshot.emit_signatures.insert(
                            file_path.clone(),
                            old_emit_signature.get_new_emit_signature(
                                old_snapshot.options,
                                self.snapshot.options,
                            ),
                        );
                    }
                }
            } else {
                let emit_kind = get_file_emit_kind(self.snapshot.options);
                self.snapshot
                    .add_file_to_affected_files_pending_emit(file_path.clone(), emit_kind);
                signature = version.clone();
            }
            self.snapshot.file_infos.insert(
                file_path,
                FileInfo {
                    version,
                    signature,
                    affects_global_scope,
                    implied_node_format,
                },
            );
        }
        // PORT: not in Go (`Snapshot::held_file_versions`). Each entry of the
        // two maps is a copy so far. The old snapshot and the old program
        // hold these versions until after this call (the watcher releases
        // the old program after `new_program`), so each is alive here.
        let snapshot = &self.snapshot;
        let held = crate::ast::diagnostic_file_versions(
            snapshot
                .semantic_diagnostics_per_file
                .values()
                .chain(snapshot.emit_diagnostics_per_file.values())
                .filter_map(|entry| entry.diagnostics.as_deref())
                .flatten(),
        );
        self.snapshot.held_file_versions = held;
    }

    // Go: incremental/programtosnapshot.go:162-179 handleFileDelete
    // PORT: Go ranges over the old `fileInfos` `SyncMap` and stops at the
    // first gone file. If that file affects global scope, all files change.
    // Else Go only sets `buildInfoEmitPending`, and unchanged files keep
    // their old semantic diagnostics and emit. The `SyncMap` order is random
    // per process, so with `gone` files of which `gone_global` affect global
    // scope, Go takes the global branch with chance `gone_global / gone`
    // (measured on pin 673a5f17d713: 1 of 3 gone files global, 66 of 200
    // runs; 1 of 4, 57 of 200; 3 of 4, 144 of 200; 1 of 93, 6 of 200). The
    // port takes Go's more likely answer, so each port answer is also a Go
    // answer: the global branch when more than half of the gone files affect
    // global scope, else only `build_info_emit_pending`. A dependency change
    // that removes many module `.d.ts` files and one global file does not
    // check again (realworld3 docusaurus: 93 gone files, 1 global).
    // Tie (exactly half of the gone files global): the port takes the global
    // branch. Go takes it slightly less than half of the time (1 of 2, 89 of
    // 200 runs; 346 of 740 runs over all tie cases measured), so neither
    // answer is a clear Go majority. The global branch is the TypeScript JS
    // answer (`forEachEntry` without `outFile` takes it when any gone file
    // affects global scope), it checks again instead of keeping diagnostics
    // that can be stale, and `tsctests::file_delete` keeps it for `lib`
    // es2016 to es2015.
    // Libs: a file with no statements does not affect global scope
    // (`fileAffectsGlobalScope`). At pin 673a5f17d713 these bundled libs have
    // no statements: `lib.d.ts`, `lib.es6.d.ts`, `lib.es20XX.d.ts`,
    // `lib.es20XX.full.d.ts`, `lib.esnext.d.ts` and `lib.esnext.full.d.ts`
    // (only `/// <reference lib>` lines), and `lib.dom.iterable.d.ts`,
    // `lib.dom.asynciterable.d.ts`, `lib.webworker.iterable.d.ts` and
    // `lib.webworker.asynciterable.d.ts` (only comments). So a `target`
    // change mostly removes global libs and checks again (es2022 to es2021:
    // 6 of 8 gone libs global), but a `lib` change can go either way. `lib`
    // from es2022, dom, dom.iterable and dom.asynciterable to es2022 removes
    // 1 global lib of 3 and does not check again: a use of `document` keeps
    // its old diagnostics and gets no TS2584 (Go: the same in 43 of 60 runs,
    // TS2584 in 17 of 60). Removing only dom and dom.iterable is a tie and
    // checks again.
    // TypeScript JS also sets `buildInfoEmitPending` for each gone file that
    // does not affect global scope and comes before the first global one.
    // The global branch here does not set `build_info_emit_pending`. This
    // differs from JS only when the program has no file other than default
    // libs, because `add_file_to_change_set` sets the flag for each other
    // file.
    fn handle_file_delete(&mut self) {
        let Some(old_program) = self.old_program else {
            return;
        };
        let mut gone = 0usize;
        let mut gone_global = 0usize;
        for (file_path, old_info) in &old_program.snapshot.borrow().file_infos {
            if !self.snapshot.file_infos.contains_key(file_path) {
                gone += 1;
                gone_global += usize::from(old_info.affects_global_scope);
            }
        }
        if gone > 0 && 2 * gone_global >= gone {
            // If the global file is removed, add all files as changed
            let files = self
                .snapshot
                .get_all_files_excluding_default_library_file(Node::NIL)
                .to_vec();
            for file in files {
                self.snapshot
                    .add_file_to_change_set(Path(source_file_info(file).path.clone()));
            }
            self.global_file_removed = true;
        } else if gone > 0 {
            self.snapshot.build_info_emit_pending = true;
        }
    }

    // Go: incremental/programtosnapshot.go:182 handleGlobalScopeChange
    // PORT: Go ranges over a `SyncMap` (random order) and stops at the first
    // file that lost global scope; the result does not depend on the order.
    fn handle_global_scope_change(&mut self) {
        let Some(old_program) = self.old_program else {
            return;
        };
        if self.global_file_removed {
            return;
        }
        let mut global_scope_lost = false;
        for (file_path, old_info) in &old_program.snapshot.borrow().file_infos {
            if !old_info.affects_global_scope {
                continue;
            }
            if let Some(new_info) = self.snapshot.file_infos.get(file_path) {
                if !new_info.affects_global_scope {
                    global_scope_lost = true;
                    break;
                }
            }
        }
        if global_scope_lost {
            let files = self
                .snapshot
                .get_all_files_excluding_default_library_file(Node::NIL)
                .to_vec();
            for file in files {
                self.snapshot
                    .add_file_to_change_set(Path(source_file_info(file).path.clone()));
            }
        }
    }

    // Go: incremental/programtosnapshot.go:204 handlePendingEmit
    fn handle_pending_emit(&mut self) {
        if let Some(old_program) = self.old_program {
            if self.global_file_removed {
                return;
            }
            let old_options = old_program.snapshot.borrow().options;
            // If options affect emit, then we need to do complete emit per compiler options
            // otherwise only the js or dts that needs to emitted because its different from previously emitted options
            let pending_emit_kind =
                if compiler_options_affect_emit(Some(old_options), Some(self.snapshot.options)) {
                    get_file_emit_kind(self.snapshot.options)
                } else {
                    get_pending_emit_kind_with_options(self.snapshot.options, old_options)
                };
            if pending_emit_kind != FileEmitKind::NONE {
                // Add all files to affectedFilesPendingEmit since emit changed
                for file in source_files() {
                    let file_path = Path(source_file_info(file).path.clone());
                    // Add to affectedFilesPending emit only if not changed since any changed file will do full emit
                    if !self.snapshot.changed_files_set.contains(&file_path) {
                        self.snapshot
                            .add_file_to_affected_files_pending_emit(file_path, pending_emit_kind);
                    }
                }
                self.snapshot.build_info_emit_pending = true;
            }
        }
    }

    // Go: incremental/programtosnapshot.go:227 handlePendingCheck
    fn handle_pending_check(&mut self) {
        if let Some(old_program) = self.old_program {
            if self.snapshot.semantic_diagnostics_per_file.len() != source_files().len()
                && old_program.snapshot.borrow().check_pending != self.snapshot.check_pending
            {
                self.snapshot.build_info_emit_pending = true;
            }
        }
    }
}

/// Go `newReferences.Equals(oldReferences)`: both absent, or the same
/// paths in any order.
// PORT: perf. A file whose imports did not change gets the same paths in
// the same order as in the old snapshot, so an equal order is checked
// first, without a hash of each path.
fn same_references(a: Option<&FxIndexSet<Path>>, b: Option<&FxIndexSet<Path>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            std::ptr::eq(a, b) || (a.len() == b.len() && (a.iter().eq(b.iter()) || a == b))
        }
        (a, b) => a.is_none() && b.is_none(),
    }
}

/// Starts the Go `t.snapshot.computeHash(versionText)` of each file in
/// `files`. The receiver gives the hashes in file order as they are ready.
// PORT: perf. The hashes run on one new thread, so that they overlap the
// bind and the checker start in `compute_program_file_changes` (the texts
// are `'static`, or owned for a content-mapped file). With `--singleThreaded` they run here, before the bind.
// One thread is enough: it reads each text once (about 16 MB for Effect),
// which takes much less time than the bind.
fn start_text_hashes(files: &[Node], hash_with_text: bool) -> std::sync::mpsc::Receiver<String> {
    let texts: Vec<std::borrow::Cow<'static, str>> = files
        .iter()
        .map(|&file| {
            // Go: incremental/programtosnapshot.go:94 (tsgo#4712): a
            // content-mapped file's version text is its original text and
            // its transform identity.
            if source_file_content_mapper(file).is_empty() {
                // A freeable file version's text is not `'static`
                // (`FileText`), so it is copied.
                let text = source_file_text(file);
                match text.as_static() {
                    Some(text) => std::borrow::Cow::Borrowed(text),
                    None => std::borrow::Cow::Owned(text.to_string()),
                }
            } else {
                std::borrow::Cow::Owned(format!(
                    "{}\x00{}",
                    source_file_original_text(file),
                    source_file_content_mapper_transform_identity(file)
                ))
            }
        })
        .collect();
    let (sender, receiver) = std::sync::mpsc::channel();
    let hash_texts = move || {
        for text in texts {
            // A send fails only when the loop stopped (it panicked).
            if sender.send(compute_hash(&text, hash_with_text)).is_err() {
                return;
            }
        }
    };
    // wasm has one thread.
    if single_threaded() || cfg!(target_family = "wasm") {
        hash_texts();
    } else {
        crate::core::GoThread::new()
            .name("goport-text-hash".to_string())
            .spawn(hash_texts);
    }
    receiver
}

// Go: incremental/programtosnapshot.go:235 fileAffectsGlobalScope
#[must_use]
pub fn file_affects_global_scope(file: Node) -> bool {
    // PORT: Go `binder.BindSourceFile(file)`. The port binds every file
    // at once; `bind_all` does nothing when they are bound.
    bind_all();
    // if file contains anything that augments to global scope we need to build them as if
    // they are global files as well as module
    if source_file_info(file)
        .module_augmentations
        .iter()
        .any(|augmentation| is_global_scope_augmentation(augmentation.parent()))
    {
        return true;
    }

    if is_external_or_common_js_module(file) || is_json_source_file(file) {
        return false;
    }

    // For script files that contains only ambient external modules, although they are not actually external module files,
    // they can only be consumed via importing elements from them. Regular script files cannot consume them. Therefore,
    // there are no point to rebuild all script files if these special files have changed. However, if any statement
    // in the file is not ambient external module, we treat it as a regular script file.
    file.statements()
        .iter()
        .any(|stmt| !is_module_with_string_literal_name(stmt))
}

// Go: incremental/programtosnapshot.go:260 addReferencedFilesFromSymbol
// PORT: the symbol belongs to the checker, so its arena is a parameter.
fn add_referenced_files_from_symbol(
    checker: &Checker,
    file: Node,
    referenced_files: &mut ReferencedFileSet,
    symbol: SymbolId,
) {
    if symbol.is_nil() {
        return;
    }
    for &declaration in checker.sym(symbol).declarations.iter() {
        let file_of_decl = get_source_file_of_node(declaration);
        if file_of_decl.is_nil() {
            continue;
        }
        if file != file_of_decl {
            referenced_files.add_file(file_of_decl);
        }
    }
}

// Go: incremental/programtosnapshot.go:276 addReferencedFilesFromImportLiteral
// Get the module source file and all augmenting files from the import name node from file
fn add_referenced_files_from_import_literal(
    file: Node,
    referenced_files: &mut ReferencedFileSet,
    checker: &mut Checker,
    import_name: Node,
) {
    let symbol = checker.get_symbol_at_location_exported(import_name);
    add_referenced_files_from_symbol(checker, file, referenced_files, symbol);
}

/// The Go `referencedFiles` set of `getReferencedFiles`, in Go order.
// PORT: perf. Go inserts a path for every declaration of every ambient
// module, and the set keeps the first. `add_file` skips a file that it
// added before, without the path hash (in Hono, about 110 ambient module
// declarations for each file). The paths are kept as a list of
// candidates in Go order, so `finish` can give back the old snapshot's set
// when it is the same, without a copy of each path. The set and its order
// are the same.
#[derive(Default)]
struct ReferencedFileSet {
    /// What Go adds to the set, in order. A path can repeat.
    candidates: Vec<ReferencedCandidate>,
    /// The files that `add_file` added.
    files: FxHashSet<Node>,
}

/// One path that `getReferencedFiles` adds: the path of a source file, or
/// a path from a file name.
enum ReferencedCandidate {
    File(Node),
    Path(Path),
}

impl ReferencedFileSet {
    /// Adds the path of `file`.
    fn add_file(&mut self, file: Node) {
        if self.files.insert(file) {
            self.candidates.push(ReferencedCandidate::File(file));
        }
    }

    /// Adds `paths`, in order.
    fn add_paths(&mut self, paths: Vec<Path>) {
        self.candidates
            .extend(paths.into_iter().map(ReferencedCandidate::Path));
    }

    /// The set: None when it is empty (Go stores no set then). `old` is the
    /// file's set in the old snapshot; when the new set has the same paths
    /// in the same order, `old` itself is the result.
    fn finish(self, old: Option<Arc<FxIndexSet<Path>>>) -> Option<Arc<FxIndexSet<Path>>> {
        if let Some(old) = old
            && self.same_as(&old)
        {
            return Some(old);
        }
        let mut paths = FxIndexSet::default();
        for candidate in self.candidates {
            match candidate {
                ReferencedCandidate::File(file) => {
                    paths.insert(Path(source_file_info(file).path.clone()));
                }
                ReferencedCandidate::Path(path) => {
                    paths.insert(path);
                }
            }
        }
        (!paths.is_empty()).then(|| Arc::new(paths))
    }

    /// True when inserting the candidates in order into an empty set gives
    /// `old`, in the same order: the first time each path comes, it is the
    /// next path of `old`, a repeat is a path that came before, and every
    /// path of `old` comes.
    fn same_as(&self, old: &FxIndexSet<Path>) -> bool {
        let mut next = 0;
        for candidate in &self.candidates {
            let index = match candidate {
                ReferencedCandidate::File(file) => {
                    old.get_index_of(source_file_info(*file).path.as_str())
                }
                ReferencedCandidate::Path(path) => old.get_index_of(path.as_str()),
            };
            match index {
                Some(index) if index == next => next += 1,
                Some(index) if index < next => {}
                _ => return false,
            }
        }
        next == old.len()
    }
}

// Go: incremental/programtosnapshot.go:282 addReferencedFileFromFileName
// Gets the path to reference file from file name, it could be resolvedPath if present otherwise path
// PORT: the paths are pushed in Go order; the checker job adds them to the
// set (see `start_referenced_files_job`).
// ts#64544: `file_name` is absolute, so the redirect lookup finds a project
// reference source or output.
fn add_referenced_file_from_file_name(file_name: &str, referenced_files: &mut Vec<Path>) {
    let redirect = get_parse_file_redirect(file_name);
    let file_name = if redirect.is_empty() {
        file_name
    } else {
        &redirect
    };
    referenced_files.push(to_path(
        file_name,
        get_current_directory(),
        use_case_sensitive_file_names(),
    ));
}

// Go: incremental/programtosnapshot.go:291 getReferencedFiles
// Gets the referenced files for a file from the program with values for the keys as referenced file's path to be true
#[must_use]
pub fn get_referenced_files(file: Node) -> Option<FxIndexSet<Path>> {
    start_referenced_files_job(file, None)
        .wait()
        .map(Arc::unwrap_or_clone)
}

/// The result of `get_referenced_files`, computed on the file's checker
/// thread.
pub type ReferencedFilesJob = CheckerJob<Option<Arc<FxIndexSet<Path>>>>;

/// Sends `get_referenced_files` for `file` to its checker thread without
/// waiting (see `compute_program_file_changes`).
// PORT: perf. The whole set is built in the job, on the checker thread, in
// Go order, so the checkers build the sets in parallel. Only the triple
// slash and type reference paths are found here first, because they read
// the Go frontend program, which works on the loading thread only. The set
// work is large: in Hono, the ambient module part tries about 110 paths for
// each file (the `@types/node` modules).
/// `old` is the file's set in the old snapshot (see
/// `ReferencedFileSet::finish`).
pub fn start_referenced_files_job(
    file: Node,
    old: Option<Arc<FxIndexSet<Path>>>,
) -> ReferencedFilesJob {
    // We need to use a set here since the code can contain the same import twice,
    // but that will only be one dependency.
    // To avoid invernal conversion, the key of the referencedFiles map must be of type Path
    let imports = source_file_info(file).imports.clone();
    let module_augmentations = source_file_info(file).module_augmentations.clone();
    let file_name_paths = referenced_file_name_paths(file);
    send_type_checker_job_for_file(file, move |checker| {
        let mut referenced_files = ReferencedFileSet::default();
        for import_name in imports {
            add_referenced_files_from_import_literal(
                file,
                &mut referenced_files,
                checker,
                import_name,
            );
        }
        referenced_files.add_paths(file_name_paths);
        // Add module augmentation as references
        for module_name in module_augmentations {
            if !is_string_literal(module_name) {
                continue;
            }
            add_referenced_files_from_import_literal(
                file,
                &mut referenced_files,
                checker,
                module_name,
            );
        }
        // From ambient modules
        // PORT: perf. Go runs `addReferencedFilesFromSymbol` for every
        // ambient module of every file. The files of those declarations
        // are the same for each file of this checker, so they are found
        // once per checker (`ambient_module_files`).
        for &file_of_decl in ambient_module_files(checker).iter() {
            if file != file_of_decl {
                referenced_files.add_file(file_of_decl);
            }
        }
        referenced_files.finish(old)
    })
}

/// The source files of the declarations of the checker's ambient modules,
/// for one checker (see `ambient_module_files`).
struct AmbientModuleFiles {
    /// The program and the checker that the list is for.
    program: u32,
    checker: u32,
    /// The declaration count of each ambient module when the list was made.
    /// A merge can add declarations later, and then the list is made again.
    counts: Vec<usize>,
    files: Rc<[Node]>,
}

thread_local! {
    /// The ambient module files of the last checker that used this thread.
    static AMBIENT_MODULE_FILES: std::cell::RefCell<Option<AmbientModuleFiles>> =
        const { std::cell::RefCell::new(None) };
}

/// The files that `add_referenced_files_from_symbol` adds for the ambient
/// modules of `checker` (Go `checker.GetAmbientModules()` in
/// `getReferencedFiles`): the source file of each declaration of each
/// ambient module, in that order, each file once, with no nil file. The
/// caller skips the file whose references it collects, as Go does. The
/// list is the same for every file of the checker, so it is made once per
/// checker, not once per file (in Hono, about 110 declarations and a walk
/// up to the source file for each).
fn ambient_module_files(checker: &mut Checker) -> Rc<[Node]> {
    let modules = checker.get_ambient_modules();
    let program = crate::core::prog().id;
    let checker_id = checker.id;
    let current = |cache: &AmbientModuleFiles, checker: &Checker| {
        cache.program == program
            && cache.checker == checker_id
            && cache.counts.len() == modules.len()
            && modules
                .iter()
                .zip(&cache.counts)
                .all(|(&module, &count)| checker.sym(module).declarations.len() == count)
    };
    let cached = AMBIENT_MODULE_FILES.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|cache| current(cache, checker))
            .map(|cache| Rc::clone(&cache.files))
    });
    if let Some(files) = cached {
        return files;
    }
    let mut seen = FxHashSet::default();
    let mut files = Vec::new();
    let mut counts = Vec::with_capacity(modules.len());
    for &module in &modules {
        let declarations = &checker.sym(module).declarations;
        counts.push(declarations.len());
        for &declaration in declarations.iter() {
            let file_of_decl = get_source_file_of_node(declaration);
            if !file_of_decl.is_nil() && seen.insert(file_of_decl) {
                files.push(file_of_decl);
            }
        }
    }
    let files: Rc<[Node]> = files.into();
    AMBIENT_MODULE_FILES.with(|slot| {
        *slot.borrow_mut() = Some(AmbientModuleFiles {
            program,
            checker: checker_id,
            counts,
            files: Rc::clone(&files),
        });
    });
    files
}

/// The triple slash and type reference parts of `get_referenced_files`, in
/// Go order.
fn referenced_file_name_paths(file: Node) -> Vec<Path> {
    let mut referenced_files = Vec::new();
    let source_file_directory = get_directory_path(source_file_file_name(file));
    // Handle triple slash references
    for referenced_file in &source_file_info(file).referenced_files {
        // ts#64544: the name resolves against the file's directory first.
        add_referenced_file_from_file_name(
            &get_normalized_absolute_path(&referenced_file.file_name, &source_file_directory),
            &mut referenced_files,
        );
    }

    // Handle type reference directives
    let info = source_file_info(file);
    let path = info.path.as_str();
    for type_ref in get_resolved_type_reference_directives_in_file(path) {
        if !type_ref.resolved_file_name.is_empty() {
            add_referenced_file_from_file_name(&type_ref.resolved_file_name, &mut referenced_files);
        }
    }
    referenced_files
}

// Go: incremental/programtosnapshot.go:337 repopulateDiagnosticsOfFile
// repopulateDiagnosticsOfFile repopulates diagnostic chains that depend on program state.
// When diagnostics are copied from a previous build, their message chains may reference
// stale program state (e.g., resolved module alternate results, package.json scope).
// This function recomputes those chains using the current program's state.
// PORT: testing. `diags.clone()` keeps the entry id (Go returns the same
// pointer), and the new entry gets a new id (see
// `DiagnosticsOrBuildInfoDiagnosticsWithFileName`).
#[must_use]
pub fn repopulate_diagnostics_of_file(
    diags: &DiagnosticsOrBuildInfoDiagnosticsWithFileName,
    file: Node,
) -> DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    if let Some(diagnostics) = &diags.diagnostics {
        let Some(repopulated) = repopulate_diagnostics_list(diagnostics, file) else {
            return diags.clone();
        };
        return DiagnosticsOrBuildInfoDiagnosticsWithFileName {
            diagnostics: Some(repopulated),
            ..Default::default()
        };
    }
    // buildInfoDiagnostics will be repopulated via toDiagnostic's repopulateInfo handling
    diags.clone()
}

// Go: incremental/programtosnapshot.go:351 repopulateDiagnosticsList
// repopulateDiagnosticsList repopulates diagnostic chains in a list of diagnostics.
// Returns nil if no diagnostics needed repopulation (i.e., no changes were made).
#[must_use]
pub fn repopulate_diagnostics_list(diags: &[Diagnostic], file: Node) -> Option<Vec<Diagnostic>> {
    let mut changed = false;
    let mut result = Vec::with_capacity(diags.len());
    for d in diags {
        if let Some(repopulated) = repopulate_diagnostic_message_chain(d.message_chain(), file) {
            let mut clone = d.clone();
            clone.set_message_chain(repopulated);
            result.push(clone);
            changed = true;
        } else {
            result.push(d.clone());
        }
    }
    if !changed {
        return None;
    }
    Some(result)
}

// Go: incremental/programtosnapshot.go:373 repopulateDiagnosticMessageChain
// repopulateDiagnosticMessageChain repopulates chains that have repopulate info.
// Returns nil if no changes were made.
#[must_use]
pub fn repopulate_diagnostic_message_chain(
    chain: &[Diagnostic],
    file: Node,
) -> Option<Vec<Diagnostic>> {
    if chain.is_empty() {
        return None;
    }
    let mut changed = false;
    let mut result = Vec::with_capacity(chain.len());
    for c in chain {
        if let Some(repopulate_info) = c.repopulate_info() {
            // Convert to buildInfoDiagnosticWithFileName and repopulate
            // PORT: `repopulate_diagnostic_chain` reads the Go offsets in `file`.
            let (pos, end) = go_text_range(file, c.loc());
            let mut b = BuildInfoDiagnosticWithFileName {
                pos,
                end,
                code: c.code(),
                category: c.category().0,
                source: c.source().to_string(),
                message_text: c.message_text().to_string(),
                message_key: c.message_key().to_string(),
                message_args: Some(c.message_args().to_vec()),
                repopulate_info: Some(repopulate_info),
                ..Default::default()
            };
            // Recursively handle nested chains
            for nested in c.message_chain() {
                b.message_chain
                    .get_or_insert_with(Vec::new)
                    .push(ast_diag_to_build_info_diag(nested));
            }
            result.push(repopulate_diagnostic_chain(&b, file));
            changed = true;
        } else {
            // Check nested chains
            if let Some(nested) = repopulate_diagnostic_message_chain(c.message_chain(), file) {
                let mut clone = c.clone();
                clone.set_message_chain(nested);
                result.push(clone);
                changed = true;
            } else {
                result.push(c.clone());
            }
        }
    }
    if !changed {
        return None;
    }
    Some(result)
}

// Go: incremental/programtosnapshot.go:418 astDiagToBuildInfoDiag
#[must_use]
pub fn ast_diag_to_build_info_diag(d: &Diagnostic) -> BuildInfoDiagnosticWithFileName {
    // PORT: Go byte offsets (see `go_text_range`).
    let (pos, end) = go_text_range(d.file(), d.loc());
    let mut b = BuildInfoDiagnosticWithFileName {
        pos,
        end,
        code: d.code(),
        category: d.category().0,
        source: d.source().to_string(),
        message_text: d.message_text().to_string(),
        message_key: d.message_key().to_string(),
        message_args: Some(d.message_args().to_vec()),
        repopulate_info: d.repopulate_info(),
        ..Default::default()
    };
    for nested in d.message_chain() {
        b.message_chain
            .get_or_insert_with(Vec::new)
            .push(ast_diag_to_build_info_diag(nested));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: incremental/external_diagnostic_test.go:14 TestExternalDiagnosticBuildInfoRoundTrip (tsgo#4712)
    // PORT: Go `toDiagnostic(nil, file)` has no program argument here.
    #[test]
    fn test_external_diagnostic_build_info_round_trip() {
        let file = parse_source_file(
            &SourceFileParseOptions {
                file_name: "/app.vue".to_string(),
                path: Path("/app.vue".to_string()),
                ..Default::default()
            },
            "",
            ScriptKind::TS,
        )
        .root;
        let diagnostic = crate::ast::new_external_diagnostic(
            file,
            TextRange::new(1, 2),
            "vue",
            crate::diagnostics::Category::Warning,
            1001,
            "mapper warning",
        );

        let serialized = ast_diag_to_build_info_diag(&diagnostic);
        assert_eq!(serialized.source, "vue");
        assert_eq!(serialized.message_text, "mapper warning");

        let restored = serialized.to_diagnostic(file);
        assert_eq!(restored.source(), "vue");
        assert_eq!(restored.localize(&crate::locale::DEFAULT), "mapper warning");
    }
}
