//! Port of execute/incremental/affectedfileshandler.go.
//!
//! PORT: Go runs `getFilesAffectedBy` and `handleDtsMayChangeOfAffectedFile`
//! on work groups, with `SyncMap`s and mutexes for the shared state. Here
//! they run one after another on the loading thread, in the order Go queues
//! them, so the handler state is plain fields. Go `dtsMayChange` maps are
//! shared by reference between the list and the queued call; here the call
//! gets the index of its map in the list.
//!
//! PORT: not in Go (t3inc1, t3inc3). In a `noEmit` program with more than
//! `DTS_WALK_FILE_LIMIT` files, a changed file with a new d.ts signature
//! skips Go's d.ts walk over its importers (`skips_dts_walk`), and a changed
//! file with few importers, none of which affects the global scope, skips
//! its own d.ts emit (`uses_version_for_changed_file`), when the pending
//! emit stays Go's (`pending_emit_covers`). In the run that takes a rule,
//! the build info differs from Go's only where Go reads the skipped d.ts
//! emits: the `fileInfos` signatures (versions, as a fresh build stores) and
//! the package.json files that only those emits look up (`packageJsons`,
//! `missingPackageJsons`). In a later run, a change of a file that stores
//! its version and keeps its d.ts counts as a new signature, so its closure
//! is checked again, where Go checks only that file; when that file or a
//! file of its closure affects the global scope, every file is checked again
//! (when the changed file affects it, every file also computes its d.ts
//! signature, Go's global path, about 2x a cold run on T3 Code apps/web).
//! When an emitting build without declarations came in between, that run
//! also makes the files that import it pending (`affectedFilesPendingEmit`;
//! Go: only that file), and the next emitting build writes them again, with
//! the same bytes. A change that affects the global scope makes every file
//! compute its d.ts signature, as in Go, except the files that a rule gave
//! their version in the same run.

use super::checker_access::*;
use super::hash::FileInfo;
use super::hash::*;
use super::program::{Program, SignatureUpdateKind};
use super::snapshot::*;
use crate::emitter::emitter::EmitOnly;
use crate::emitter::program_emit::{EmitOptions, WriteFile, WriteFileData, emit};
use crate::frontend::prelude::*;
use crate::program::source_file_may_be_emitted;
use std::cell::OnceCell;
use std::sync::{Arc, Mutex, OnceLock};

/// PORT: not in Go (t3inc1). The most source files that are not declaration
/// or JSON files that a `noEmit` program can have and still run Go's d.ts
/// walk (`skips_dts_walk`). In Go's tsc, tsbuild and tscWatch baselines at
/// the pin, a program has at most 23 such files, and at most 6 in a baseline
/// that uses `noEmit`, so the baselines never reach it.
const DTS_WALK_FILE_LIMIT: usize = 32;

/// False when `GOPORT_DTS_WALK_FALLBACK` is `0`: every change then runs Go's
/// d.ts walk (`skips_dts_walk`). For A/B tests and timing on one binary.
/// Read once.
fn dts_walk_fallback_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("GOPORT_DTS_WALK_FALLBACK").as_deref() != Ok("0"))
}

// Go: incremental/affectedfileshandler.go:18 dtsMayChange
pub type DtsMayChange = FxIndexMap<Path, FileEmitKind>;

// Go: incremental/affectedfileshandler.go:24 updatedSignature
// PORT: Go `mu` guards the parallel computation; there is none here.
#[derive(Clone, Debug, Default)]
pub struct UpdatedSignature {
    pub signature: String,
    pub kind: SignatureUpdateKind,
}

// Go: incremental/affectedfileshandler.go:30 affectedFilesHandler
// PORT: Go `ctx` is dropped (the port has no cancellation).
pub struct AffectedFilesHandler<'a> {
    program: &'a Program,
    has_all_files_excluding_default_library_file: bool,
    updated_signatures: FxIndexMap<Path, UpdatedSignature>,
    dts_may_change: Vec<DtsMayChange>,
    files_to_remove_diagnostics: FxIndexSet<Path>,
    cleaned_diagnostics_of_lib_files: bool,
    seen_file_and_references: FxHashMap<Path, bool>,
    /// PORT: not in Go (t3inc1). The value of `skips_dts_walk`.
    skips_dts_walk: OnceCell<bool>,
}

/// Go `file.Path()` as a `tspath.Path`.
fn path_of(file: Node) -> Path {
    Path(source_file_info(file).path.clone())
}

/// The callback of `forEachFileReferencedBy`: `(queueForFile, fastReturn)`.
type ReferencedByCallback<'c, 'a> =
    dyn FnMut(&mut AffectedFilesHandler<'a>, Node, &Path) -> (bool, bool) + 'c;

impl<'a> AffectedFilesHandler<'a> {
    #[must_use]
    pub fn new(program: &'a Program) -> Self {
        AffectedFilesHandler {
            program,
            has_all_files_excluding_default_library_file: false,
            updated_signatures: FxIndexMap::default(),
            dts_may_change: Vec::new(),
            files_to_remove_diagnostics: FxIndexSet::default(),
            cleaned_diagnostics_of_lib_files: false,
            seen_file_and_references: FxHashMap::default(),
            skips_dts_walk: OnceCell::new(),
        }
    }

    // Go: incremental/affectedfileshandler.go:41 getDtsMayChange
    // PORT: returns the index of the new map in `dts_may_change`.
    fn get_dts_may_change(
        &mut self,
        affected_file_path: Path,
        affected_file_emit_kind: FileEmitKind,
    ) -> usize {
        let mut result = DtsMayChange::default();
        result.insert(affected_file_path, affected_file_emit_kind);
        self.dts_may_change.push(result);
        self.dts_may_change.len() - 1
    }

    // Go: incremental/affectedfileshandler.go:47 isChangedSignature
    fn is_changed_signature(&self, path: &Path) -> bool {
        let new_signature = self
            .updated_signatures
            .get(path)
            .expect("isChangedSignature: signature is updated");
        // This method is called after updating signatures of that path, so signature is present in updatedSignatures
        // And is already calculated, so no need to lock and unlock mutex on the entry
        let snapshot = self.program.snapshot.borrow();
        let old_info = snapshot
            .file_infos
            .get(path)
            .expect("isChangedSignature: file info");
        new_signature.signature != old_info.signature
    }

    // Go: incremental/affectedfileshandler.go:55 removeSemanticDiagnosticsOf
    fn remove_semantic_diagnostics_of(&mut self, path: Path) {
        self.files_to_remove_diagnostics.insert(path);
    }

    // Go: incremental/affectedfileshandler.go:59 removeDiagnosticsOfLibraryFiles
    fn remove_diagnostics_of_library_files(&mut self) {
        if self.cleaned_diagnostics_of_lib_files {
            return;
        }
        self.cleaned_diagnostics_of_lib_files = true;
        for file in source_files() {
            let path = path_of(file);
            if is_source_file_default_library(&path) && !skip_type_checking(file, true) {
                self.remove_semantic_diagnostics_of(path);
            }
        }
    }

    // Go: incremental/affectedfileshandler.go:69 computeDtsSignature
    // PORT: emit runs `WriteFile` on the file's checker thread, so the
    // signature comes back through a shared cell.
    fn compute_dts_signature(&self, file: Node) -> String {
        let done = self.program.begin_nested_emit();
        let signature = Arc::new(Mutex::new(String::new()));
        let hash_with_text = self.program.snapshot.borrow().hash_with_text;
        let result_cell = Arc::clone(&signature);
        let write_file: WriteFile = Arc::new(
            move |file_name: &str, text: &str, data: &mut WriteFileData| {
                if !is_declaration_file_name(file_name) {
                    panic!("File extension for signature expected to be dts, got : {file_name}");
                }
                *result_cell.lock().expect("signature lock") =
                    compute_signature_with_diagnostics(file, text, data, hash_with_text);
                Ok(())
            },
        );
        emit(EmitOptions {
            // #4699: Go `core.SingleElementSlice(file)`; `file` is not nil.
            target_source_files: Some(vec![file]),
            // #4849: renamed from Go `EmitOnlyForcedDts`.
            emit_only: EmitOnly::BuilderSignature,
            write_file: Some(write_file),
            ..EmitOptions::default()
        });
        let result = signature.lock().expect("signature lock").clone();
        // Go: defer done()
        done();
        result
    }

    // Go: incremental/affectedfileshandler.go:87 updateShapeSignature (ts#64026)
    fn update_shape_signature(&mut self, file: Node, use_file_version_as_signature: bool) -> bool {
        let path = path_of(file);
        // If we have cached the result for this file, that means hence forth we should assume file shape is uptodate
        if self.updated_signatures.contains_key(&path) {
            return false;
        }
        self.updated_signatures
            .insert(path.clone(), UpdatedSignature::default());

        let (prev_signature, version) = {
            let snapshot = self.program.snapshot.borrow();
            let info = snapshot
                .file_infos
                .get(&path)
                .expect("updateShapeSignature: file info");
            (info.signature.clone(), info.version.clone())
        };
        let mut update = UpdatedSignature::default();
        // JSON files have no declaration output from which to compute a shape
        // signature, so use the file version to conservatively invalidate dependents.
        if !source_file_info(file).is_declaration_file
            && !is_json_source_file(file)
            && !use_file_version_as_signature
        {
            update.signature = self.compute_dts_signature(file);
        }
        // Default is to use file version as signature
        if update.signature.is_empty() {
            update.signature = version;
            update.kind = SignatureUpdateKind::UsedVersion;
        }
        let changed = update.signature != prev_signature;
        self.updated_signatures.insert(path, update);
        changed
    }

    // Go: incremental/affectedfileshandler.go:114 getFilesAffectedBy
    fn get_files_affected_by(&mut self, path: &Path) -> Vec<Node> {
        let file = get_source_file_by_path(path);
        if file.is_nil() {
            return Vec::new();
        }

        // PORT: not in Go (t3inc1). Go computes the d.ts signature of `file`.
        let use_version = self.uses_version_for_changed_file(file);
        if !self.update_shape_signature(file, use_version) {
            return vec![file];
        }

        let affects_global_scope = self
            .program
            .snapshot
            .borrow()
            .file_infos
            .get(&path_of(file))
            .expect("getFilesAffectedBy: file info")
            .affects_global_scope;
        if affects_global_scope {
            self.has_all_files_excluding_default_library_file = true;
            return self
                .program
                .snapshot
                .borrow()
                .get_all_files_excluding_default_library_file(file)
                .to_vec();
        }

        if self
            .program
            .snapshot
            .borrow()
            .options
            .isolated_modules
            .is_true()
        {
            return vec![file];
        }

        // PORT: not in Go (t3inc1, t3inc3). Every file that imports `file`
        // is affected, and its version is its signature (no d.ts emit), when
        // that keeps Go's pending emit (`walk_skip_keeps_pending_emit`).
        if self.skips_dts_walk() {
            let closure = self.for_each_file_referenced_by(file, &mut |_, current_file, _| {
                (current_file.is_some(), false)
            });
            if use_version || self.walk_skip_keeps_pending_emit(file, &closure) {
                let files: Vec<Node> = closure
                    .into_values()
                    .filter(|file| file.is_some())
                    .collect();
                for &current_file in &files {
                    self.update_shape_signature(current_file, true);
                }
                return files;
            }
        }

        // Now we need to if each file in the referencedBy list has a shape change as well.
        // Because if so, its own referencedBy files need to be saved as well to make the
        // emitting result consistent with files on disk.
        let seen_file_names_map =
            self.for_each_file_referenced_by(file, &mut |handler, current_file, _current_path| {
                // If the current file is not nil and has a shape change, we need to queue it for processing
                if current_file.is_some() && handler.update_shape_signature(current_file, false) {
                    return (true, false);
                }
                (false, false)
            });
        // Return array of values that needs emit
        seen_file_names_map
            .into_values()
            .filter(|file| file.is_some())
            .collect()
    }

    /// PORT: not in Go (t3inc1, perf). True when `getFilesAffectedBy` skips
    /// the d.ts walk over the files that import a changed file with a new d.ts
    /// signature: the program has `noEmit`,
    /// `assumeChangesOnlyAffectDirectDependencies` is off, and the program has
    /// more than `DTS_WALK_FILE_LIMIT` source files that are not declaration
    /// or JSON files. The same for every changed file, so read once.
    ///
    /// Go's walk computes a d.ts signature for each importer, one at a time,
    /// and goes on to the importers of each file whose signature changed.
    /// After a fresh build every stored signature is the file version, so the
    /// first walk goes through the whole closure. On T3 Code apps/server one
    /// edit gave 675 serial d.ts emits, and the incremental run took 1.6x the
    /// cold run (Go at the pin: 1.4x). A small closure can cost as much: in
    /// T3 Code packages/contracts an edit of a file with 11 importers gave 11
    /// d.ts emits that took 1.0 s, against 0.35 s for the cold run, because
    /// printing the inferred Effect Schema types costs more than checking
    /// them. So the limit is on the program, not on the closure.
    ///
    /// The walk does not change which files are checked again:
    /// `handleDtsMayChangeOfAffectedFile` removes the semantic diagnostics of
    /// the whole closure of a changed file with a new signature. Under
    /// `noEmit` it only sets the stored signatures, and the full pending
    /// emit kind of each file that it visits. So with this rule:
    /// - the diagnostics are the same;
    /// - the files of the closure store their version as signature, as after
    ///   Go's `handleDtsMayChangeOf` (a later change of one of them always
    ///   counts as a new signature);
    /// - each file of the closure gets the full pending emit kind, where Go
    ///   gives it to the files that the walk visits. So the rule applies only
    ///   when the closure is already pending that kind
    ///   (`walk_skip_keeps_pending_emit`), as after a fresh build: it makes
    ///   every file pending, and a `noEmit` build emits no JS. After an
    ///   emitting build Go's walk runs, so the pending emit stays Go's.
    fn skips_dts_walk(&self) -> bool {
        *self.skips_dts_walk.get_or_init(|| {
            if !dts_walk_fallback_enabled() {
                return false;
            }
            let options = self.program.snapshot.borrow().options;
            if !options.no_emit.is_true()
                || options
                    .assume_changes_only_affect_direct_dependencies
                    .is_true()
            {
                return false;
            }
            source_files()
                .iter()
                .filter(|&&file| {
                    !source_file_info(file).is_declaration_file && !is_json_source_file(file)
                })
                .nth(DTS_WALK_FILE_LIMIT)
                .is_some()
        })
    }

    /// PORT: not in Go (t3inc1, perf). True when `getFilesAffectedBy` gives
    /// the changed `file` its version as signature, with no d.ts emit:
    /// `skips_dts_walk` holds, `file` does not affect the global scope, its
    /// stored signature is not its version, at most `DTS_WALK_FILE_LIMIT`
    /// files import it, directly or not, none of them affects the global
    /// scope, and each of them is already pending the full emit kind. The
    /// version then counts as a new signature, so the closure of `file` is
    /// checked again and gets the full pending emit kind. When the d.ts of
    /// `file` did not change, Go's `getFilesAffectedBy` returns only `file`,
    /// and no other file gets a pending emit. So the closure must be pending
    /// that kind already (t3inc3), as after a fresh build, for the pending
    /// emit to stay Go's. A changed file of the closure is affected in Go
    /// too, so it does not count. (A file pending the d.ts errors has no
    /// cached emit diagnostics, so the new pending emit removes none.)
    ///
    /// Go computes the d.ts signature of each changed file, so that a change
    /// that keeps the d.ts checks only that file again. The emit can cost
    /// more than a check of the whole program: in T3 Code
    /// packages/contracts, two inferred types of orchestrationV2.ts exceed
    /// the length that the compiler serializes (TS7056), and its d.ts emit
    /// took 0.63 s, against 0.36 s for a cold run. With few importers, a
    /// check of the closure costs little and has a bound. A file with more
    /// importers still computes its signature. When the stored signature is
    /// the version, the text did not change (the references or the global
    /// scope did), and Go's d.ts emit decides, as the version would count as
    /// no change.
    fn uses_version_for_changed_file(&self, file: Node) -> bool {
        if !self.skips_dts_walk() {
            return false;
        }
        let snapshot = self.program.snapshot.borrow();
        let start = path_of(file);
        let info = snapshot
            .file_infos
            .get(&start)
            .expect("usesVersionForChangedFile: file info");
        if info.affects_global_scope || info.signature == info.version {
            return false;
        }
        let mut seen: FxHashSet<Path> = FxHashSet::default();
        let mut queue = snapshot.referenced_map.get_referenced_by(&start).to_vec();
        seen.insert(start);
        while let Some(path) = queue.pop() {
            if seen.insert(path.clone()) {
                // `seen` holds `file` too.
                if seen.len() > DTS_WALK_FILE_LIMIT + 1 {
                    return false;
                }
                let refs = snapshot.referenced_map.get_referenced_by(&path);
                queue.extend(refs.iter().filter(|r| !seen.contains(*r)).cloned());
            }
        }
        // A file of the closure that affects the global scope would make
        // every file of the program affected (Go
        // `handleDtsMayChangeOfGlobalScope`), where Go's d.ts emit of `file`
        // may find no change and affect only `file`.
        if seen.iter().any(|path| {
            snapshot
                .file_infos
                .get(path)
                .is_some_and(|info| info.affects_global_scope)
        }) {
            return false;
        }
        // `file` is a changed file, so `pending_emit_covers` skips it.
        let files = seen
            .iter()
            .map(|path| (path, get_source_file_by_path(path)));
        pending_emit_covers(&snapshot, files, get_file_emit_kind(snapshot.options))
    }

    /// PORT: not in Go (t3inc3). True when skipping Go's walk for the changed
    /// `file`, whose d.ts signature changed, keeps Go's pending emit.
    /// `closure` is `file` and the files that import it, directly or not.
    ///
    /// The skip makes each file of `closure` affected, so each gets the full
    /// emit kind (`getDtsMayChange`). Go gives that kind to the files that its
    /// walk visits: always the direct importers of `file` (its walk visits
    /// each one), and the changed files (each is affected by its own
    /// `getFilesAffectedBy`). Each other file of the closure gets the d.ts
    /// kind of `handleDtsMayChangeOf` (none without declarations) from
    /// `handleDtsMayChangeOfFileAndReferences`, the same in Go and here. So
    /// only those other files can differ, in the bits of the full kind that
    /// the d.ts kind lacks (the JS emit and its maps), and they must be
    /// pending those bits already.
    fn walk_skip_keeps_pending_emit(&self, file: Node, closure: &FxIndexMap<Path, Node>) -> bool {
        let snapshot = self.program.snapshot.borrow();
        let options = snapshot.options;
        // Go: handleDtsMayChangeOf, with `invalidateJsFiles` false.
        let dts_kind = if !options.get_emit_declarations() {
            FileEmitKind::NONE
        } else if options.declaration_map.is_true() {
            FileEmitKind::ALL_DTS
        } else {
            FileEmitKind::DTS
        };
        let kind = get_file_emit_kind(options).without(dts_kind);
        let direct_list = snapshot.referenced_map.get_referenced_by(&path_of(file));
        let direct: FxHashSet<&Path> = direct_list.iter().collect();
        let deeper = closure
            .iter()
            .filter(|(path, _)| !direct.contains(path))
            .map(|(path, &current_file)| (path, current_file));
        pending_emit_covers(&snapshot, deeper, kind)
    }

    // Go: incremental/affectedfileshandler.go:152 forEachFileReferencedBy
    fn for_each_file_referenced_by(
        &mut self,
        file: Node,
        fn_: &mut ReferencedByCallback<'_, 'a>,
    ) -> FxIndexMap<Path, Node> {
        // Now we need to if each file in the referencedBy list has a shape change as well.
        // Because if so, its own referencedBy files need to be saved as well to make the
        // emitting result consistent with files on disk.
        let mut seen_file_names_map: FxIndexMap<Path, Node> = FxIndexMap::default();
        // Start with the paths this file was referenced by
        seen_file_names_map.insert(path_of(file), file);
        let mut queue = self
            .program
            .snapshot
            .borrow()
            .referenced_map
            .get_referenced_by(&path_of(file))
            .to_vec();
        while let Some(current_path) = queue.pop() {
            if !seen_file_names_map.contains_key(&current_path) {
                let current_file = get_source_file_by_path(&current_path);
                seen_file_names_map.insert(current_path.clone(), current_file);
                let (queue_for_file, fast_return) = fn_(self, current_file, &current_path);
                if fast_return {
                    return seen_file_names_map;
                }
                if queue_for_file {
                    let refs = self
                        .program
                        .snapshot
                        .borrow()
                        .referenced_map
                        .get_referenced_by(&path_of(current_file));
                    // PORT: perf (t3inc1). Go queues every referencer; a
                    // path already seen is skipped when it comes off the
                    // queue, so leaving it out keeps the order of visits.
                    queue.extend(
                        refs.iter()
                            .filter(|path| !seen_file_names_map.contains_key(*path))
                            .cloned(),
                    );
                }
            }
        }
        seen_file_names_map
    }

    // Go: incremental/affectedfileshandler.go:182 handleDtsMayChangeOfAffectedFile (ts#64691)
    // Handles semantic diagnostics and dts emit for affectedFile and files, that are referencing modules that export entities from affected file
    // This is because even though js emit doesnt change, dts emit / type used can change resulting in need for dts emit and js change
    fn handle_dts_may_change_of_affected_file(
        &mut self,
        dts_may_change: usize,
        affected_file: Node,
    ) {
        let affected_path = path_of(affected_file);
        self.remove_semantic_diagnostics_of(affected_path.clone());

        // If affected files is everything except default library, then nothing more to do
        if self.has_all_files_excluding_default_library_file {
            self.remove_diagnostics_of_library_files();
            // When a change affects the global scope, all files are considered to be affected without updating their signature
            // That means when affected file is handled, its signature can be out of date
            // To avoid this, ensure that we update the signature for any affected file in this scenario.
            self.update_shape_signature(affected_file, false);
            return;
        }

        let options = self.program.snapshot.borrow().options;
        if options
            .assume_changes_only_affect_direct_dependencies
            .is_true()
        {
            return;
        }

        // Iterate on referencing modules that export entities from affected file and delete diagnostics and add pending emit
        // If there was change in signature (dts output) for the changed file,
        // then only we need to handle pending file emit
        if !self
            .program
            .snapshot
            .borrow()
            .changed_files_set
            .contains(&affected_path)
            || !self.is_changed_signature(&affected_path)
        {
            return;
        }

        // At this point affectedFile is actually one of the changed files
        // that has some change in its .d.ts signature.

        // Since isolated modules dont change js files, files affected by change in signature is itself
        // But we need to cleanup semantic diagnostics and queue dts emit for affected files
        if options.isolated_modules.is_true() {
            self.for_each_file_referenced_by(
                affected_file,
                &mut |handler, _current_file, current_path| {
                    if handler.handle_dts_may_change_of_global_scope(
                        dts_may_change,
                        current_path,
                        false,
                    ) {
                        return (false, true);
                    }
                    handler.handle_dts_may_change_of(dts_may_change, current_path, false);
                    if handler.is_changed_signature(current_path) {
                        return (true, false);
                    }
                    (false, false)
                },
            );
        }

        // If exported const enum, we need to ensure that js files are emitted as well since the const enum value changed
        // PORT: Go reads the binder symbol table and takes the file's checker
        // (exclusive) at the first export that is not a const enum. The whole
        // loop runs in one job on that checker's thread, over the checker's
        // copy of the symbols.
        let mut invalidate_js_files = false;
        let file_symbol = affected_file.symbol();
        if file_symbol.is_some() {
            invalidate_js_files =
                get_type_checker_for_file_exclusive(affected_file, move |type_checker| {
                    let exports: Vec<SymbolId> = type_checker
                        .symbols
                        .iter(type_checker.sym(file_symbol).exports)
                        .map(|(_, exported)| exported)
                        .collect();
                    for exported in exports {
                        if type_checker
                            .sym_flags(exported)
                            .intersects(SymbolFlags::CONST_ENUM)
                        {
                            return true;
                        }
                        let aliased = type_checker.skip_alias(exported);
                        if aliased == exported {
                            continue;
                        }
                        if type_checker
                            .sym_flags(aliased)
                            .intersects(SymbolFlags::CONST_ENUM)
                            && type_checker
                                .sym(aliased)
                                .declarations
                                .iter()
                                .any(|&d| get_source_file_of_node(d) == affected_file)
                        {
                            return true;
                        }
                    }
                    false
                });
        }

        // Go through files that reference affected file and handle dts emit and semantic diagnostics for them and their references
        let referencing = self
            .program
            .snapshot
            .borrow()
            .referenced_map
            .get_referenced_by(&affected_path);
        for file_referencing_changed_file in referencing.iter() {
            if self.handle_dts_may_change_of_global_scope(
                dts_may_change,
                file_referencing_changed_file,
                invalidate_js_files,
            ) {
                return;
            }
            // Since references of changed file = affected files - we would have already handled d.ts emit and semantic diagnostics
            // for those files. Now we need to handle files referencing those affected files to ensure correctness.
            let referencing_affected = self
                .program
                .snapshot
                .borrow()
                .referenced_map
                .get_referenced_by(file_referencing_changed_file);
            for file_referencing_affected_file in referencing_affected.iter() {
                if self.handle_dts_may_change_of_file_and_references(
                    dts_may_change,
                    file_referencing_affected_file,
                    invalidate_js_files,
                ) {
                    return;
                }
            }
        }
    }

    // Go: incremental/affectedfileshandler.go:274 handleDtsMayChangeOfFileAndReferences
    fn handle_dts_may_change_of_file_and_references(
        &mut self,
        dts_may_change: usize,
        file_path: &Path,
        invalidate_js_files: bool,
    ) -> bool {
        match self.seen_file_and_references.get(file_path).copied() {
            Some(existing) => {
                if existing || !invalidate_js_files {
                    return false;
                }
                self.seen_file_and_references
                    .insert(file_path.clone(), true);
            }
            None => {
                self.seen_file_and_references
                    .insert(file_path.clone(), invalidate_js_files);
            }
        }

        if self.handle_dts_may_change_of_global_scope(
            dts_may_change,
            file_path,
            invalidate_js_files,
        ) {
            return true;
        }
        self.handle_dts_may_change_of(dts_may_change, file_path, invalidate_js_files);

        // Remove the diagnostics of files that import this file and
        // any files that are referenced by it (directly or indirectly)
        let referencing = self
            .program
            .snapshot
            .borrow()
            .referenced_map
            .get_referenced_by(file_path);
        for referencing_file_path in referencing.iter() {
            if self.handle_dts_may_change_of_file_and_references(
                dts_may_change,
                referencing_file_path,
                invalidate_js_files,
            ) {
                return true;
            }
        }
        false
    }

    // Go: incremental/affectedfileshandler.go:296 handleDtsMayChangeOfGlobalScope
    fn handle_dts_may_change_of_global_scope(
        &mut self,
        dts_may_change: usize,
        file_path: &Path,
        invalidate_js_files: bool,
    ) -> bool {
        let affects_global_scope = self
            .program
            .snapshot
            .borrow()
            .file_infos
            .get(file_path)
            .is_some_and(|info| info.affects_global_scope);
        if !affects_global_scope {
            return false;
        }
        // Every file needs to be handled
        let files = self
            .program
            .snapshot
            .borrow()
            .get_all_files_excluding_default_library_file(Node::NIL)
            .to_vec();
        for file in files {
            self.handle_dts_may_change_of(dts_may_change, &path_of(file), invalidate_js_files);
        }
        self.remove_diagnostics_of_library_files();
        true
    }

    // Go: incremental/affectedfileshandler.go:310 handleDtsMayChangeOf
    // Handle the dts may change, so they need to be added to pending emit if dts emit is enabled,
    // Also we need to make sure signature is updated for these files
    fn handle_dts_may_change_of(
        &mut self,
        dts_may_change: usize,
        path: &Path,
        invalidate_js_files: bool,
    ) {
        if self
            .program
            .snapshot
            .borrow()
            .changed_files_set
            .contains(path)
        {
            return;
        }
        let file = get_source_file_by_path(path);
        if file.is_nil() {
            return;
        }
        self.remove_semantic_diagnostics_of(path.clone());
        // Even though the js emit doesnt change and we are already handling dts emit and semantic diagnostics
        // we need to update the signature to reflect correctness of the signature(which is output d.ts emit) of this file
        // This ensures that we dont later during incremental builds considering wrong signature.
        // Eg where this also is needed to ensure that .tsbuildinfo generated by incremental build should be same as if it was first fresh build
        // But we avoid expensive full shape computation, as using file version as shape is enough for correctness.
        self.update_shape_signature(file, true);
        let options = self.program.snapshot.borrow().options;
        // If not dts emit, nothing more to do
        if invalidate_js_files {
            self.dts_may_change[dts_may_change].insert(path.clone(), get_file_emit_kind(options));
        } else if options.get_emit_declarations() {
            self.dts_may_change[dts_may_change].insert(
                path.clone(),
                if options.declaration_map.is_true() {
                    FileEmitKind::ALL_DTS
                } else {
                    FileEmitKind::DTS
                },
            );
        }
    }

    // Go: incremental/affectedfileshandler.go:333 updateSnapshot
    fn update_snapshot(self) {
        let mut snapshot = self.program.snapshot.borrow_mut();
        for (file_path, update) in &self.updated_signatures {
            if let Some(info) = snapshot.file_infos.get_mut(file_path) {
                info.signature = update.signature.clone();
                if let Some(testing_data) = &self.program.testing_data {
                    testing_data
                        .borrow_mut()
                        .updated_signature_kinds
                        .insert(file_path.clone(), update.kind);
                }
            }
        }
        // PORT: perf (t3inc1). Go deletes each file from its map. One pass
        // keeps the order of the other files, as a `shift_remove` (O(n)) of
        // each file did.
        if !self.files_to_remove_diagnostics.is_empty() {
            snapshot
                .semantic_diagnostics_per_file
                .retain(|path, _| !self.files_to_remove_diagnostics.contains(path));
        }
        for change in &self.dts_may_change {
            for (file_path, emit_kind) in change {
                snapshot.add_file_to_affected_files_pending_emit(file_path.clone(), *emit_kind);
            }
        }
        snapshot.changed_files_set = FxIndexSet::default();
        snapshot.build_info_emit_pending = true;
    }
}

/// PORT: not in Go (t3inc3). True when adding `kind` to the pending emit of
/// each of `files` (path and source file) keeps Go's pending emit: the file
/// is already pending an emit that includes `kind`
/// (`addFileToAffectedFilesPendingEmit` ors the kinds), it is a changed file
/// (affected in Go too), or it has no source file or cannot be emitted. Go
/// never affects a path with no source file, and drops the pending emit of a
/// file that cannot be emitted, such as a declaration file or a file of an
/// external library, from the build info (`setAffectedFilesPendingEmit`)
/// and at each emit (`emitFilesIncremental`). The handler reads the
/// snapshot before `updateSnapshot`.
fn pending_emit_covers<'p>(
    snapshot: &Snapshot,
    files: impl IntoIterator<Item = (&'p Path, Node)>,
    kind: FileEmitKind,
) -> bool {
    kind.is_empty()
        || files.into_iter().all(|(path, file)| {
            snapshot
                .affected_files_pending_emit
                .get(path)
                .is_some_and(|pending| pending.contains(kind))
                || snapshot.changed_files_set.contains(path)
                || file.is_nil()
                || !source_file_may_be_emitted(file, false)
        })
}

// Go: incremental/affectedfileshandler.go:359 collectAllAffectedFiles
pub fn collect_all_affected_files(program: &Program) {
    if program.snapshot.borrow().changed_files_set.is_empty() {
        return;
    }

    let mut handler = AffectedFilesHandler::new(program);
    let mut result: FxIndexSet<Node> = FxIndexSet::default();
    let mut changed_files: Vec<Path> = program
        .snapshot
        .borrow()
        .changed_files_set
        .iter()
        .cloned()
        .collect();
    // PORT: Go runs both loops on work groups. Each `computeDtsSignature` is a
    // d.ts emit of one file (affectedfileshandler.go:69), and where its
    // widenings come first sets the d.ts text, so the order counts. Single
    // threaded, Go runs each queue last queued first (singleThreadedWorkGroup
    // pop, core/workgroup.go:78), so the port runs both loops in reverse
    // (dtsorder1: better-auth/00/incr step 2). Go calls `getDtsMayChange` in
    // the `Range` before it queues, but the port calls it in the reversed
    // loop, so new pending-emit entries come in reverse order. Go's order
    // there comes from random map order, so the reverse is still one of Go's
    // possible orders. The flag is the program's own, as Go's
    // `program.program.SingleThreaded()`, not that of the current program.
    let single_threaded = program.program().single_threaded();
    if single_threaded {
        changed_files.reverse();
    }
    for file in &changed_files {
        for affected_file in handler.get_files_affected_by(file) {
            result.insert(affected_file);
        }
    }

    // For all the affected files, get all the files that would need to change their dts or js files,
    // update their diagnostics
    let emit_kind = get_file_emit_kind(program.snapshot.borrow().options);
    let mut result: Vec<Node> = result.into_iter().collect();
    if single_threaded {
        result.reverse();
    }
    for file in result {
        // remove the cached semantic diagnostics and handle dts emit and js emit if needed
        let dts_may_change = handler.get_dts_may_change(path_of(file), emit_kind);
        handler.handle_dts_may_change_of_affected_file(dts_may_change, file);
    }

    // Update the snapshot with the new state
    handler.update_snapshot();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::enter_program;
    use crate::execute::incremental::incremental::create_host;
    use crate::execute::incremental::program::new_program;
    use crate::program::{go_frontend_program, release_program, try_load_version};

    // followups64 (R215 reviewer item a): `collect_all_affected_files` takes
    // the single-thread order from its own program, as Go's
    // `program.program.SingleThreaded()`, not from the current program.
    // The product makes the program current while it uses it, so here a
    // second load of the same files is current while the handler runs.
    // `b.ts` imports `a.ts`, and the new `a.ts` has a new signature, so the
    // affected files are `a.ts`, then `b.ts`. The single-thread order runs
    // them last queued first, so the pending emit lists `b.ts` first.
    #[test]
    fn affected_file_order_reads_the_program_flag() {
        let dir =
            std::env::temp_dir().join(format!("goport-affected-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write(
            "tsconfig.json",
            r#"{"compilerOptions":{"incremental":true,"types":[]},"files":["a.ts","b.ts"]}"#,
        );
        write("b.ts", "import { x } from './a';\nexport const y = x;\n");
        let config = format!("{}/tsconfig.json", dir.to_string_lossy().replace('\\', "/"));
        let load = |single_threaded: bool| {
            try_load_version(&config, |options| {
                if single_threaded {
                    options.single_threaded = Tristate::True;
                }
            })
            .unwrap_or_else(|e| panic!("cannot load {config}: {e}"))
        };
        // The incremental program of the current program.
        let incremental = |old: Option<&Program>| {
            let host = create_host(go_frontend_program().expect("frontend").host().clone());
            new_program(old, host, None, false)
        };
        for (own, current, expected) in [
            (true, false, ["b.ts", "a.ts"]),
            (false, true, ["a.ts", "b.ts"]),
        ] {
            write("a.ts", "export const x = 1;\n");
            let first = load(false);
            let old = {
                let _scope = enter_program(Some(first));
                let old = incremental(None);
                // As after an emit, so the new snapshot does not take the
                // old pending emit with its order.
                old.snapshot
                    .borrow_mut()
                    .affected_files_pending_emit
                    .clear();
                old
            };
            write("a.ts", "export const x = 'a';\n");
            let second = load(own);
            let new = {
                let _scope = enter_program(Some(second));
                incremental(Some(&old))
            };
            let other = load(current);
            {
                let _scope = enter_program(Some(other));
                collect_all_affected_files(&new);
            }
            let pending: Vec<String> = new
                .snapshot
                .borrow()
                .affected_files_pending_emit
                .keys()
                .map(|path| path.0.rsplit('/').next().unwrap_or_default().to_string())
                .collect();
            drop((old, new));
            for program in [first, second, other] {
                release_program(program);
            }
            assert_eq!(
                pending, expected,
                "own --singleThreaded {own}, current program --singleThreaded {current}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `noEmit` project of `no_emit_change_of_a`: `a.ts`, `importers`
    /// files `b<i>.ts` that import it, and `others` files `c<i>.ts`.
    #[derive(Clone, Copy, Default)]
    struct Project {
        importers: usize,
        others: usize,
        /// `c<i>.ts` imports `b<i>.ts` (`others <= importers`), else nothing.
        chained: bool,
        /// `a.ts` declares a global, so it affects the global scope.
        global: bool,
        /// The first program has no pending emit, as after an emitting
        /// build.
        after_emit: bool,
        /// `b0.ts` declares a global, so it affects the global scope.
        global_importer: bool,
        /// The edit of `a.ts` keeps its d.ts, after a first edit that
        /// also keeps it, so that `a.ts` stores its d.ts signature.
        same_dts: bool,
    }

    /// What `collect_all_affected_files` does in `project` when `a.ts` is
    /// edited (a new d.ts signature, unless `same_dts`; then after the
    /// warm-up edit of `same_dts`). Returns the signature kind of each
    /// updated file and the files that keep their semantic diagnostics, by
    /// base name and sorted.
    fn no_emit_change_of_a(
        test: &str,
        project: Project,
    ) -> (FxHashMap<String, SignatureUpdateKind>, Vec<String>) {
        let dir = std::env::temp_dir().join(format!("goport-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        write(
            "tsconfig.json",
            r#"{"compilerOptions":{"incremental":true,"noEmit":true,"skipLibCheck":true,"types":[]},"include":["*.ts"]}"#,
        );
        for i in 0..project.importers {
            let global = if project.global_importer && i == 0 {
                "declare global { var gb: number; }\n"
            } else {
                ""
            };
            write(
                &format!("b{i}.ts"),
                &format!("import {{ x }} from './a';\nexport const y{i} = x;\n{global}"),
            );
        }
        for i in 0..project.others {
            let text = if project.chained {
                format!("import {{ y{i} }} from './b{i}';\nexport const z{i} = y{i};\n")
            } else {
                format!("export const z{i} = {i};\n")
            };
            write(&format!("c{i}.ts"), &text);
        }
        let global = if project.global {
            "declare global { var ga: number; }\n"
        } else {
            ""
        };
        let (before, after) = if project.same_dts {
            ("export const x: number = 1;", "export const x: number = 2;")
        } else {
            ("export const x = 1;", "export const x = 'a';")
        };
        write("a.ts", &format!("{before}\n{global}"));
        let config = format!("{}/tsconfig.json", dir.to_string_lossy().replace('\\', "/"));
        let load = || {
            try_load_version(&config, |_| {})
                .unwrap_or_else(|e| panic!("cannot load {config}: {e}"))
        };
        let host = || create_host(go_frontend_program().expect("frontend").host().clone());
        let first = load();
        // Both programs are testing programs, so their file versions hash the
        // same way (`hash_with_text`).
        let old = {
            let _scope = enter_program(Some(first));
            let old = new_program(None, host(), None, true);
            // The cold check, so every file has cached diagnostics.
            let _ = old.get_semantic_diagnostics(Node::NIL);
            if project.after_emit {
                old.snapshot
                    .borrow_mut()
                    .affected_files_pending_emit
                    .clear();
            }
            old
        };
        // `same_dts`: first an edit that keeps the d.ts, so that `a.ts`
        // stores its d.ts signature, as after a run that computed it. (The
        // cold program stores none, so its first change always counts as a
        // new signature, in Go too.)
        let (old, warm) = if project.same_dts {
            write("a.ts", &format!("export const x: number = 3;\n{global}"));
            let warm = load();
            let next = {
                let _scope = enter_program(Some(warm));
                let next = new_program(Some(&old), host(), None, true);
                let _ = next.get_semantic_diagnostics(Node::NIL);
                next
            };
            drop(old);
            (next, Some(warm))
        } else {
            (old, None)
        };
        write("a.ts", &format!("{after}\n{global}"));
        let second = load();
        let new = {
            let _scope = enter_program(Some(second));
            let new = new_program(Some(&old), host(), None, true);
            collect_all_affected_files(&new);
            new
        };
        let base_name = |path: &Path| path.0.rsplit('/').next().unwrap_or_default().to_string();
        // The project files, not the libs.
        let ours = |name: &String| {
            name == "a.ts"
                || name
                    .strip_prefix(['b', 'c'])
                    .and_then(|rest| rest.strip_suffix(".ts"))
                    .is_some_and(|i| i.parse::<usize>().is_ok())
        };
        let kinds = new
            .get_testing_data()
            .expect("testing data")
            .updated_signature_kinds
            .iter()
            .map(|(path, kind)| (base_name(path), *kind))
            .collect();
        let mut kept: Vec<String> = new
            .snapshot
            .borrow()
            .semantic_diagnostics_per_file
            .keys()
            .map(base_name)
            .filter(ours)
            .collect();
        kept.sort();
        drop((old, new));
        release_program(first);
        if let Some(warm) = warm {
            release_program(warm);
        }
        release_program(second);
        let _ = std::fs::remove_dir_all(&dir);
        (kinds, kept)
    }

    /// Runs `no_emit_change_of_a` and checks the signature kind of `a.ts`,
    /// of each `b<i>.ts` and of each `c<i>.ts`. `c_kind` is `None` when no
    /// `c<i>.ts` is updated: then they keep their cached diagnostics, as in
    /// Go, and every other project file is checked again.
    fn check_no_emit_change_of_a(
        test: &str,
        project: Project,
        a_kind: SignatureUpdateKind,
        b_kind: SignatureUpdateKind,
        c_kind: Option<SignatureUpdateKind>,
    ) {
        let (kinds, kept) = no_emit_change_of_a(test, project);
        assert_eq!(kinds.get("a.ts"), Some(&a_kind), "{kinds:?}");
        for i in 0..project.importers {
            assert_eq!(kinds.get(&format!("b{i}.ts")), Some(&b_kind), "b{i}.ts");
        }
        for i in 0..project.others {
            assert_eq!(kinds.get(&format!("c{i}.ts")), c_kind.as_ref(), "c{i}.ts");
        }
        let updated_others = if c_kind.is_some() { project.others } else { 0 };
        assert_eq!(
            kinds.len(),
            1 + project.importers + updated_others,
            "{kinds:?}"
        );
        let mut c_files: Vec<String> = (0..project.others - updated_others)
            .map(|i| format!("c{i}.ts"))
            .collect();
        c_files.sort();
        assert_eq!(kept, c_files);
    }

    // t3inc1: in a `noEmit` program of `DTS_WALK_FILE_LIMIT + 1` files, a
    // changed file with 3 importers and its importers get their versions as
    // signatures, with no d.ts emit (Go computes each one).
    #[test]
    fn no_emit_large_program_narrow_change_uses_versions() {
        let importers = 3;
        let project = Project {
            importers,
            others: DTS_WALK_FILE_LIMIT - importers,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-large-narrow",
            project,
            SignatureUpdateKind::UsedVersion,
            SignatureUpdateKind::UsedVersion,
            None,
        );
    }

    // t3inc1: with more than `DTS_WALK_FILE_LIMIT` importers, the changed
    // file computes its d.ts signature, and its importers get their versions.
    #[test]
    fn no_emit_large_program_wide_change_computes_the_changed_file() {
        let project = Project {
            importers: DTS_WALK_FILE_LIMIT + 1,
            others: 1,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-large-wide",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::UsedVersion,
            None,
        );
    }

    // t3inc1: in a program of `DTS_WALK_FILE_LIMIT` files, Go's d.ts walk
    // runs.
    #[test]
    fn no_emit_small_program_computes_signatures() {
        let importers = 3;
        let project = Project {
            importers,
            others: DTS_WALK_FILE_LIMIT - importers - 1,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-small",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::ComputedDts,
            None,
        );
    }

    // t3inc3: after an emitting build no file is pending, so a skip of the
    // walk would give `c<i>.ts` (which imports `b<i>.ts`) the JS pending emit
    // that Go's walk gives only to the files it visits. Go's walk runs: it
    // computes each importer's d.ts, and goes on to `c<i>.ts` because the
    // d.ts of `b<i>.ts` changed.
    #[test]
    fn no_emit_change_after_an_emit_runs_the_walk() {
        let project = Project {
            importers: DTS_WALK_FILE_LIMIT + 1,
            others: 1,
            chained: true,
            after_emit: true,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-after-emit-walk",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::ComputedDts,
            Some(SignatureUpdateKind::ComputedDts),
        );
    }

    // t3inc3: Go's walk always visits the direct importers, so when no other
    // file imports `a.ts` the skip keeps Go's pending emit, after an emit
    // too.
    #[test]
    fn no_emit_change_after_an_emit_of_direct_importers_skips_the_walk() {
        let project = Project {
            importers: DTS_WALK_FILE_LIMIT + 1,
            others: 1,
            after_emit: true,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-after-emit-direct",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::UsedVersion,
            None,
        );
    }

    // t3inc3: after an emitting build the importers of `a.ts` are not
    // pending, so `a.ts` computes its d.ts signature: had its d.ts not
    // changed, Go would make only `a.ts` pending.
    #[test]
    fn no_emit_narrow_change_after_an_emit_computes_its_signature() {
        let importers = 3;
        let project = Project {
            importers,
            others: DTS_WALK_FILE_LIMIT - importers,
            after_emit: true,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-after-emit-narrow",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::UsedVersion,
            None,
        );
    }

    // t3inc3: a change of `a.ts`, which affects the global scope, makes every
    // file affected, and each one computes its d.ts signature, as in Go
    // (`handleDtsMayChangeOfAffectedFile`), also where every file is pending
    // and the other rules apply. A later change of a file that keeps its d.ts
    // then checks only that file.
    #[test]
    fn no_emit_global_change_computes_signatures() {
        let project = Project {
            importers: 3,
            others: DTS_WALK_FILE_LIMIT,
            global: true,
            ..Project::default()
        };
        check_no_emit_change_of_a(
            "no-emit-global",
            project,
            SignatureUpdateKind::ComputedDts,
            SignatureUpdateKind::ComputedDts,
            Some(SignatureUpdateKind::ComputedDts),
        );
    }

    // t3inc3 round 3: `b0.ts`, an importer of `a.ts`, affects the global
    // scope, so a change of `a.ts` with a new d.ts signature makes every file
    // affected (Go `handleDtsMayChangeOfGlobalScope`). Its version would
    // count as such a change, so `a.ts`, which stores its d.ts signature
    // after a first edit, computes it again, as in Go: the second edit keeps
    // the d.ts, so only `a.ts` is checked again.
    #[test]
    fn no_emit_narrow_change_near_a_global_file_computes_its_signature() {
        let importers = 3;
        let project = Project {
            importers,
            others: DTS_WALK_FILE_LIMIT - importers,
            global_importer: true,
            same_dts: true,
            ..Project::default()
        };
        let (kinds, kept) = no_emit_change_of_a("no-emit-near-global", project);
        let kinds: Vec<(String, SignatureUpdateKind)> = kinds.into_iter().collect();
        assert_eq!(
            kinds,
            [("a.ts".to_string(), SignatureUpdateKind::ComputedDts)]
        );
        let mut others: Vec<String> = (0..importers)
            .map(|i| format!("b{i}.ts"))
            .chain((0..project.others).map(|i| format!("c{i}.ts")))
            .collect();
        others.sort();
        assert_eq!(kept, others);
    }
}
