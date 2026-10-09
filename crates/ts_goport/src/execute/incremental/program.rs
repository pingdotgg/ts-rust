//! Port of execute/incremental/program.go, plus `ReadBuildInfoProgram`
//! (incremental.go:43), which builds a `Program` from a snapshot.
//!
//! PORT: Go `Program.program` is the current `&'static GoProgram`
//! (`prog()`, nil for a program read from build info), and its methods are
//! the `program.rs` free functions, which read `prog()`. A caller with
//! several programs (the build task) makes the program current while it
//! uses this one. Go passes a context; the port has none. The
//! `ProgramLike` methods take `&self`, so the snapshot is behind a
//! `RefCell`; it is an `Rc` because Go `programToSnapshot` can reuse the
//! old program's snapshot.

use super::build_info::*;
use super::build_info_to_snapshot::build_info_to_snapshot;
use super::checker_access::*;
use super::emit_files::{
    StartedEmit, emit_files, finish_emit_files, fs_error_text, start_emit_files,
};
use super::hash::FileInfo;
use super::incremental::{BuildInfoReader, Host, marshal_build_info};
use super::program_to_snapshot::program_to_snapshot;
use super::snapshot::*;
use super::snapshot_to_build_info::snapshot_to_build_info;
use crate::emitter::emitter::EmitOnly;
use crate::emitter::program_emit::{
    EmitOptions, EmitResult, WriteFileData, check_cannot_see_outputs, early_emit_options_allow,
};
use crate::execute::tsc::emit::ProgramLike;
use crate::frontend::prelude::*;
use crate::gostd::slices::stable_sort_by;
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Go `func() time.Time`, the `nestedEmitNow` of `NewProgram` (`sys.Now`).
pub type NestedEmitNow = Rc<dyn Fn() -> SystemTime>;

// Go: incremental/program.go:22 SignatureUpdateKind
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SignatureUpdateKind {
    #[default]
    ComputedDts = 0,
    StoredAtEmit = 1,
    UsedVersion = 2,
}

// Go: incremental/program.go:30 Program
// PORT: Go `host` is nil for a program read from build info. Go
// `nestedEmitMu` guards the nested emit fields; the program is used on one
// thread, so they are `Cell`s. Go `time.Time` is `Option<SystemTime>`
// (`None` = zero).
pub struct Program {
    pub(crate) snapshot: Rc<RefCell<Snapshot>>,
    pub(crate) program: Option<&'static GoProgram>,
    pub(crate) host: Option<Rc<dyn Host>>,

    // Testing data
    pub(crate) testing_data: Option<RefCell<TestingData>>,

    nested_emit_now: Option<NestedEmitNow>,
    nested_emit_depth: Cell<i32>,
    nested_emit_start: Cell<Option<SystemTime>>,
    nested_emit_time: Cell<Duration>,

    // PORT: not in Go. What `start_check` read and started (see there).
    started: RefCell<StartedCheck>,
    // PORT: not in Go. The time that `start_check` spent in the part of
    // `GetSemanticDiagnostics` that it does before it sends the check (see
    // `take_started_check_time`).
    started_check_time: Cell<Duration>,
}

/// The work of `tsc.EmitFilesAndReportErrors` that `Program::start_check`
/// and `Program::start_emit` did before the caller asks for it.
#[derive(Default)]
struct StartedCheck {
    /// The first `GetGlobalDiagnostics` result. The next
    /// `get_global_diagnostics` call takes it.
    global_diagnostics: Option<Vec<Diagnostic>>,
    /// The check of the affected files. `get_semantic_diagnostics` takes it.
    check: Option<PendingSemanticDiagnostics>,
    /// The emit of the affected files, sent behind the check. `emit` takes
    /// it.
    emit: Option<StartedEmit>,
}

// Go: incremental/program.go:47 NewProgram
// PORT: Go `program` is the current program (`prog()`), so it is not a
// parameter.
#[must_use]
pub fn new_program(
    old_program: Option<&Program>,
    host: Rc<dyn Host>,
    nested_emit_now: Option<NestedEmitNow>,
    testing: bool,
) -> Program {
    let mut incremental_program = Program {
        snapshot: program_to_snapshot(old_program, testing),
        program: Some(prog()),
        host: Some(host),
        testing_data: None,
        nested_emit_now,
        nested_emit_depth: Cell::new(0),
        nested_emit_start: Cell::new(None),
        nested_emit_time: Cell::new(Duration::ZERO),
        started: RefCell::default(),
        started_check_time: Cell::new(Duration::ZERO),
    };

    if testing {
        // PORT: testing. Go keeps pointers to the new snapshot's and the old
        // program's `semanticDiagnosticsPerFile` (see `TestingData`).
        let old_semantic_diagnostics_ids = match old_program {
            Some(old_program) => old_program
                .snapshot
                .borrow()
                .semantic_diagnostics_per_file
                .iter()
                .map(|(path, diagnostics)| (path.clone(), diagnostics.id))
                .collect(),
            None => FxHashMap::default(),
        };
        incremental_program.testing_data = Some(RefCell::new(TestingData {
            old_semantic_diagnostics_ids,
            ..TestingData::default()
        }));
    }
    incremental_program
}

// Go: incremental/incremental.go:44 ReadBuildInfoProgram
#[must_use]
pub fn read_build_info_program(
    config: &ParsedCommandLine,
    reader: &dyn BuildInfoReader,
    host: &dyn CompilerHost,
) -> Option<Program> {
    // Read buildInfo file
    let build_info = reader.read_build_info(config)?;
    build_info_program(config, &build_info, host)
}

/// `read_build_info_program` after the read: the program of `build_info`.
// PORT: not in Go. The `tsc -b` task holds its build info
// (`loadOrStoreBuildInfo`), and gives it here without the copy that a
// `BuildInfoReader` returns.
#[must_use]
pub fn build_info_program(
    config: &ParsedCommandLine,
    build_info: &BuildInfo,
    host: &dyn CompilerHost,
) -> Option<Program> {
    if !build_info.is_valid_version(
        crate::effect::rulerunner::enabled_options(config.compiler_options()).is_some(),
    ) || !build_info.is_incremental()
    {
        return None;
    }
    // If any configured content mapper's identity has changed, files it produced may be stale, so the
    // old program cannot be reused.
    let content_mapper_project = host.content_mapper_project();
    match content_mapper_identities(content_mapper_project.as_deref()) {
        Ok(identities) if build_info.content_mapper_identities_match(identities.as_deref()) => {}
        _ => return None,
    }

    // Convert to information that can be used to create incremental program
    Some(Program {
        snapshot: Rc::new(RefCell::new(build_info_to_snapshot(
            build_info, config, host,
        ))),
        program: None,
        host: None,
        testing_data: None,
        nested_emit_now: None,
        nested_emit_depth: Cell::new(0),
        nested_emit_start: Cell::new(None),
        nested_emit_time: Cell::new(Duration::ZERO),
        started: RefCell::default(),
        started_check_time: Cell::new(Duration::ZERO),
    })
}

// Go: incremental/program.go:68 TestingData
// PORT: testing. Go keeps pointers to the new snapshot's and the old
// program's `semanticDiagnosticsPerFile`, and the Go test harness compares
// the entry pointers. The entry identity is its `id` (see
// `DiagnosticsOrBuildInfoDiagnosticsWithFileName`). Go
// `SemanticDiagnosticsPerFile` is `Program::semantic_diagnostics_id`,
// which reads the current snapshot as the Go pointer does. Go
// `OldProgramSemanticDiagnosticsPerFile` is `old_semantic_diagnostics_ids`,
// the old map's ids when `new_program` ran (empty without an old program).
// Nothing changes the old map after that, except when Go
// `programToSnapshot` reuses the old snapshot (same program). Then Go
// compares one map with itself; only watch mode (not on this branch) makes
// that case, and it must read the ids live.
#[derive(Clone, Debug, Default)]
pub struct TestingData {
    pub updated_signature_kinds: FxHashMap<Path, SignatureUpdateKind>,
    pub old_semantic_diagnostics_ids: FxHashMap<Path, u64>,
}

impl Program {
    // Go: incremental/program.go:74 GetTestingData
    #[must_use]
    pub fn get_testing_data(&self) -> Option<std::cell::Ref<'_, TestingData>> {
        self.testing_data.as_ref().map(RefCell::borrow)
    }

    // Go: incremental/program.go:78 beginNestedEmit
    // PORT: Go returns the `done` func for `defer`; the caller calls the
    // returned closure when the nested emit ends. A negative Go duration
    // (the clock went back) is zero here.
    pub(crate) fn begin_nested_emit(&self) -> impl FnOnce() + '_ {
        let now = self.nested_emit_now.clone();
        if let Some(now) = &now {
            if self.nested_emit_depth.get() == 0 {
                self.nested_emit_start.set(Some(now()));
            }
            self.nested_emit_depth.set(self.nested_emit_depth.get() + 1);
        }

        move || {
            let Some(now) = now else {
                return;
            };
            self.nested_emit_depth.set(self.nested_emit_depth.get() - 1);
            if self.nested_emit_depth.get() == 0 {
                let start = self
                    .nested_emit_start
                    .get()
                    .expect("beginNestedEmit set the start");
                let elapsed = now().duration_since(start).unwrap_or_default();
                self.nested_emit_time
                    .set(self.nested_emit_time.get() + elapsed);
            }
        }
    }

    // Go: incremental/program.go:101 TakeNestedEmitTime
    #[must_use]
    pub fn take_nested_emit_time(&self) -> Duration {
        self.nested_emit_time.replace(Duration::ZERO)
    }

    /// PORT: not in Go. The time that `start_check` spent on the affected
    /// files (`collectAllAffectedFiles`, with the nested declaration emits
    /// of their signatures) before it sent the check, by the
    /// `nestedEmitNow` clock (Go `sys.Now`); zero when it did not run. Go
    /// does that work inside the `GetSemanticDiagnostics` call that
    /// `tsc.EmitFilesAndReportErrors` times as the check. That call adds
    /// this time to its own, so the nested emit time that it then moves to
    /// the emit time is inside the check time, as in Go.
    #[must_use]
    pub fn take_started_check_time(&self) -> Duration {
        self.started_check_time.replace(Duration::ZERO)
    }

    // PORT: testing. Go `testingData.SemanticDiagnosticsPerFile.Load(path)`,
    // as the entry identity (see `TestingData`).
    #[must_use]
    pub fn semantic_diagnostics_id(&self, path: &Path) -> Option<u64> {
        self.snapshot
            .borrow()
            .semantic_diagnostics_per_file
            .get(path)
            .map(|diagnostics| diagnostics.id)
    }

    // Go: incremental/program.go:109 panicIfNoProgram
    fn panic_if_no_program(&self, method: &str) {
        if self.program.is_none() {
            panic!("{method}: should not be called without program");
        }
    }

    // Go: incremental/program.go:115 GetProgram
    #[must_use]
    pub fn get_program(&self) -> &'static GoProgram {
        self.panic_if_no_program("GetProgram");
        self.program.expect("program")
    }

    /// PORT: not in Go. Drops this program and returns its snapshot when
    /// nothing else holds it, so the caller can free it elsewhere.
    #[must_use]
    pub fn into_snapshot(self) -> Option<Snapshot> {
        let Program { snapshot, .. } = self;
        Rc::try_unwrap(snapshot).ok().map(RefCell::into_inner)
    }

    // Go: incremental/program.go:120 HasChangedDtsFile
    #[must_use]
    pub fn has_changed_dts_file(&self) -> bool {
        self.snapshot.borrow().has_changed_dts_file
    }

    // Go: incremental/program.go:125 Options
    // Options implements compiler.AnyProgram interface.
    #[must_use]
    pub fn options(&self) -> &'static CompilerOptions {
        self.snapshot.borrow().options
    }

    // Go: incremental/program.go:130 CommonSourceDirectory
    // CommonSourceDirectory implements compiler.AnyProgram interface.
    #[must_use]
    pub fn common_source_directory(&self) -> &'static str {
        self.panic_if_no_program("CommonSourceDirectory");
        common_source_directory()
    }

    // Go: incremental/program.go:30 Program
    // Program implements compiler.AnyProgram interface.
    #[must_use]
    pub fn program(&self) -> &'static GoProgram {
        self.panic_if_no_program("Program");
        self.program.expect("program")
    }

    // Go: incremental/program.go:142 IsSourceFileDefaultLibrary
    // IsSourceFileDefaultLibrary implements compiler.AnyProgram interface.
    #[must_use]
    pub fn is_source_file_default_library(&self, path: &Path) -> bool {
        self.panic_if_no_program("IsSourceFileDefaultLibrary");
        is_source_file_default_library(path)
    }

    // Go: incremental/program.go:148 GetSourceFiles
    // GetSourceFiles implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_source_files(&self) -> Vec<Node> {
        self.panic_if_no_program("GetSourceFiles");
        source_files()
    }

    // Go: incremental/program.go:154 GetSourceFile
    // GetSourceFile implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_source_file(&self, path: &str) -> Node {
        self.panic_if_no_program("GetSourceFile");
        get_source_file(path)
    }

    // Go: incremental/program.go:160 GetConfigFileParsingDiagnostics
    // GetConfigFileParsingDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_config_file_parsing_diagnostics(&self) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetConfigFileParsingDiagnostics");
        get_config_file_parsing_diagnostics()
    }

    // Go: incremental/program.go:166 GetSyntacticDiagnostics
    // GetSyntacticDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_syntactic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetSyntacticDiagnostics");
        get_syntactic_diagnostics(file)
    }

    // Go: incremental/program.go:172 GetBindDiagnostics
    // GetBindDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetBindDiagnostics");
        get_bind_diagnostics(file)
    }

    // Go: incremental/program.go:177 GetProgramDiagnostics
    #[must_use]
    pub fn get_program_diagnostics(&self) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetProgramDiagnostics");
        get_program_diagnostics()
    }

    // Go: incremental/program.go:182 GetGlobalDiagnostics
    // PORT: the first call returns what `start_check` read, if it ran.
    // Since ts#64452 Go `GetDiagnosticsOfAnyProgram` does not ask an
    // incremental program for them again after the check: each file's
    // cached diagnostics hold the globals that its check found
    // (`GetSemanticDiagnosticsForIncremental`).
    #[must_use]
    pub fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetGlobalDiagnostics");
        if let Some(diagnostics) = self.started.borrow_mut().global_diagnostics.take() {
            return diagnostics;
        }
        get_global_diagnostics()
    }

    /// PORT: not in Go. Starts the semantic check that
    /// `tsc.EmitFilesAndReportErrors` asks for, and returns without waiting
    /// for it. `tsc -b` calls it when the program is made: Go builds up to 4
    /// projects on goroutines at the same time, and here each project's
    /// checkers check while the loading thread makes the next program or
    /// emits an earlier one. `start_check_and_emit` (`tsc -p`) calls it too.
    ///
    /// It does what `EmitFilesAndReportErrors` (through
    /// `GetDiagnosticsOfAnyProgram`) does before the check, with the same
    /// checker jobs in the same order. When the syntactic and program
    /// diagnostics are empty (and not `--listFilesOnly` or `--noCheck`), it
    /// reads the global diagnostics. When those are empty too, it handles
    /// the affected files (`collectAllAffectedFiles`) and sends the check of
    /// the files that `get_semantic_diagnostics` would check. The next
    /// `get_global_diagnostics` call returns the global diagnostics read
    /// here, and `get_semantic_diagnostics` waits for the check. The caller
    /// must use this program only through those calls, in that order, as
    /// `EmitFilesAndReportErrors` does (`start_check_used` checks it).
    pub fn start_check(&self) {
        self.panic_if_no_program("StartCheck");
        if self.snapshot.borrow().options.no_check.is_true()
            || options().list_files_only.is_true()
            || !get_syntactic_diagnostics(Node::NIL).is_empty()
            || !get_program_diagnostics().is_empty()
        {
            return;
        }
        let global_diagnostics = get_global_diagnostics();
        let has_global_diagnostics = !global_diagnostics.is_empty();
        self.started.borrow_mut().global_diagnostics = Some(global_diagnostics);
        if has_global_diagnostics {
            return;
        }
        let check_start = self.nested_emit_now.as_ref().map(|now| now());
        if let Some(affected_files) = self.semantic_diagnostics_files_to_check(Node::NIL) {
            self.started.borrow_mut().check =
                Some(start_semantic_diagnostics_for_incremental(&affected_files));
        }
        if let (Some(now), Some(check_start)) = (&self.nested_emit_now, check_start) {
            self.started_check_time
                .set(now().duration_since(check_start).unwrap_or_default());
        }
    }

    /// PORT: not in Go (perf). When the options allow an early emit
    /// (`early_emit_options_allow`): `start_check`, then `start_emit`.
    /// `tsc -p` calls it when the program is made
    /// (`perform_incremental_compilation`). When the options do not allow
    /// it, it does nothing, and the calls run in Go's order.
    ///
    /// Go waits for the whole check, then emits. Here each checker thread
    /// gets the same jobs in the same order (check, emit), and runs its emit
    /// when its own check ends. The emit pool runs the JS parts during the
    /// check. `emit` waits for the emit. `options` must be the
    /// options of that `emit` call: no target file, `EmitOnly::All` and the
    /// same `WriteFile`.
    pub fn start_check_and_emit(&self, options: EmitOptions) {
        if !early_emit_options_allow() {
            return;
        }
        self.start_check();
        self.start_emit(options);
    }

    /// PORT: not in Go (perf). After `start_check`: when the options allow
    /// an early emit (`early_emit_options_allow`), Go emits (no
    /// `--listFilesOnly`) and the check cannot see the outputs
    /// (`check_cannot_see_outputs`), sends the rest of the checker work of
    /// `tsc.EmitFilesAndReportErrors` without a wait: the emit of the
    /// affected files with `options` (`start_emit_files`; since ts#64452 Go
    /// reads no global diagnostics after the check of an incremental
    /// program). Else it does nothing. `start_check_and_emit` (`tsc -p`)
    /// calls it. `tsc -b` calls it right after `start_check` in
    /// `BuildTask::compile_and_emit_start`, inside
    /// `buffer_early_emit_writes`, so the writes wait for the task's
    /// `compile_and_emit_finish`. In tests `tsc -b` calls it in
    /// `BuildTask::compile_and_emit_finish`, right before
    /// `EmitAndReportStatistics`.
    ///
    /// The emit starts also when `start_check` sent no check. Then Go's
    /// task only emits (tscbemit1, tscbemit2): every program file has
    /// cached semantic diagnostics, so
    /// `collectSemanticDiagnosticsOfAffectedFiles` returns before a check;
    /// or `GetDiagnosticsOfAnyProgram` skips the semantic diagnostics
    /// (syntactic, program or global diagnostics); or they are empty
    /// (`noCheck`). The emit is then the program's only
    /// checker work, and `tsc -b` finishes the task when that emit ends
    /// (`BuildTask::notify_when_compiled`), as Go's task goroutine writes
    /// when its own emit ends. Without it, the emit ran in
    /// `compile_and_emit_finish` and such tasks finished in build order.
    ///
    /// Go reads the global diagnostics before the emit whenever the
    /// syntactic diagnostics are empty, also with `noCheck` or program
    /// diagnostics, where `start_check` returns before it reads them. The
    /// affected-file walk and the emit can add global diagnostics (Go's
    /// "incidental signature-generation globals"), so this reads them
    /// first, after the program diagnostics, as `start_check` does. The
    /// next `get_global_diagnostics` call takes them.
    pub fn start_emit(&self, options: EmitOptions) {
        debug_assert!(
            self.started.borrow().emit.is_none(),
            "start_emit: the emit already started"
        );
        // The file rules read every program file: the checkers check
        // meanwhile (when a check started).
        if !early_emit_options_allow()
            || self.options().list_files_only.is_true()
            || !self.snapshot.borrow().can_use_incremental_state()
            || !check_cannot_see_outputs()
        {
            return;
        }
        if self.started.borrow().global_diagnostics.is_none()
            && get_syntactic_diagnostics(Node::NIL).is_empty()
        {
            get_program_diagnostics();
            let global_diagnostics = get_global_diagnostics();
            self.started.borrow_mut().global_diagnostics = Some(global_diagnostics);
        }
        let emit = start_emit_files(self, options);
        self.started.borrow_mut().emit = Some(emit);
    }

    /// PORT: not in Go. True when the caller used everything that
    /// `start_check` and `start_emit` read and started. `tsc -b`
    /// and `tsc -p` assert it after the emit: a result left over means the
    /// calls did not follow `EmitFilesAndReportErrors`.
    #[must_use]
    pub fn start_check_used(&self) -> bool {
        let started = self.started.borrow();
        started.global_diagnostics.is_none() && started.check.is_none() && started.emit.is_none()
    }

    // Go: incremental/program.go:188 GetSemanticDiagnostics
    // GetSemanticDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetSemanticDiagnostics");
        if self.snapshot.borrow().options.no_check.is_true() {
            return Vec::new();
        }

        // Ensure all the diagnsotics are cached
        self.collect_semantic_diagnostics_of_affected_files(file);

        // Return result from cache
        if file.is_some() {
            return self.get_semantic_diagnostics_of_file(file);
        }

        let mut diagnostics = Vec::new();
        for file in source_files() {
            diagnostics.extend(self.get_semantic_diagnostics_of_file(file));
        }
        diagnostics
    }

    // Go: incremental/program.go:212 getSemanticDiagnosticsOfFile
    fn get_semantic_diagnostics_of_file(&self, file: Node) -> Vec<Diagnostic> {
        let mut snapshot = self.snapshot.borrow_mut();
        let options = snapshot.options;
        let path = Path(source_file_info(file).path.clone());
        let Some(cached_diagnostics) = snapshot.semantic_diagnostics_per_file.get_mut(&path) else {
            panic!("After handling all the affected files, there shouldnt be more changes");
        };
        let diagnostics = cached_diagnostics.get_diagnostics(file);
        drop(snapshot);
        let mut result = filter_no_emit_semantic_diagnostics(diagnostics, options);
        result.extend(get_include_processor_diagnostics(file));
        result
    }

    // Go: incremental/program.go:224 GetDeclarationDiagnostics
    // GetDeclarationDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetDeclarationDiagnostics");
        let result = emit_files(
            self,
            EmitOptions {
                // #4699: Go `core.SingleElementSlice(file)`: nil for a nil file.
                target_source_files: file.is_some().then(|| vec![file]),
                ..EmitOptions::default()
            },
            true,
        );
        result.diagnostics
    }

    // Go: incremental/program.go:236 GetSuggestionDiagnostics
    // GetSuggestionDiagnostics implements compiler.AnyProgram interface.
    #[must_use]
    pub fn get_suggestion_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetSuggestionDiagnostics");
        get_suggestion_diagnostics(file) // TODO: incremental suggestion diagnostics (only relevant in editor incremental builder?)
    }

    // Go: incremental/program.go:242 Emit
    // GetModeForUsageLocation implements compiler.AnyProgram interface.
    // PORT: with an emit that `start_emit` sent, this waits for it
    // and finishes it. That emit started only without `noEmit` and
    // `noEmitOnError`, so Go goes to `emitFiles` here too.
    pub fn emit(&self, options: EmitOptions) -> EmitResult {
        self.panic_if_no_program("Emit");

        let started = self.started.borrow_mut().emit.take();
        if let Some(started) = started {
            return finish_emit_files(self, started, &options);
        }

        let mut result = None;
        // #4407: Go `HandleNoEmitOptions` with `emitBuildInfo` under noEmit.
        if !options.force_emit && options.emit_only != EmitOnly::BuilderSignature {
            let emit_build_info: &dyn Fn() -> Option<EmitResult> =
                &|| self.emit_build_info(&options);
            result = handle_no_emit_options(
                self,
                options.target_source_files.as_deref(),
                self.options().no_emit.is_true().then_some(emit_build_info),
            );
        }
        if let Some(mut result) = result {
            if options.target_source_files.is_some() || self.options().no_emit.is_true() {
                return result;
            }

            // Emit buildInfo and combine result
            if let Some(build_info_result) = self.emit_build_info(&options) {
                result.diagnostics.extend(build_info_result.diagnostics);
                result.emitted_files.extend(build_info_result.emitted_files);
            }
            return result;
        }
        emit_files(self, options, false)
    }

    // Go: incremental/program.go:275 collectSemanticDiagnosticsOfAffectedFiles (ts#64452)
    // Handle affected files and cache the semantic diagnostics for all of them or the file asked for
    // PORT: split in two (`semantic_diagnostics_files_to_check` and
    // `commit_semantic_diagnostics`), so `start_check` can send the check
    // early. A check that `start_check` sent is committed here first.
    fn collect_semantic_diagnostics_of_affected_files(&self, file: Node) {
        let started = self.started.borrow_mut().check.take();
        if let Some(started) = started {
            let affected_files = started.files().to_vec();
            self.commit_semantic_diagnostics(&affected_files, started.wait());
            if file.is_nil() {
                return;
            }
        }
        let Some(affected_files) = self.semantic_diagnostics_files_to_check(file) else {
            return;
        };

        // Get their diagnostics and cache them
        let diagnostics_per_file = get_semantic_diagnostics_for_incremental(&affected_files);
        self.commit_semantic_diagnostics(&affected_files, diagnostics_per_file);
    }

    /// The first half of `collectSemanticDiagnosticsOfAffectedFiles`: it
    /// handles the affected files and returns the files to check, or `None`
    /// where Go returns before the check.
    fn semantic_diagnostics_files_to_check(&self, file: Node) -> Option<Vec<Node>> {
        if self.snapshot.borrow().can_use_incremental_state() {
            // Get all affected files
            super::affected_files::collect_all_affected_files(self);

            if self.snapshot.borrow().semantic_diagnostics_per_file.len() == source_files().len() {
                // If we have all the files,
                return None;
            }
        }

        if file.is_some() {
            let path = Path(source_file_info(file).path.clone());
            if self
                .snapshot
                .borrow()
                .semantic_diagnostics_per_file
                .contains_key(&path)
            {
                return None;
            }
            return Some(vec![file]);
        }
        let snapshot = self.snapshot.borrow();
        Some(
            source_files()
                .into_iter()
                .filter(|&file| {
                    !snapshot
                        .semantic_diagnostics_per_file
                        .contains_key(source_file_info(file).path.as_str())
                })
                .collect(),
        )
    }

    /// The second half of `collectSemanticDiagnosticsOfAffectedFiles`: it
    /// caches the check result of `affected_files` in the snapshot.
    fn commit_semantic_diagnostics(
        &self,
        affected_files: &[Node],
        mut diagnostics_per_file: FxHashMap<Node, Vec<Diagnostic>>,
    ) {
        // Commit changes to snapshot
        let mut snapshot = self.snapshot.borrow_mut();
        for file in affected_files {
            if let Some(diagnostics) = diagnostics_per_file.remove(file) {
                snapshot.semantic_diagnostics_per_file.insert(
                    Path(source_file_info(*file).path.clone()),
                    DiagnosticsOrBuildInfoDiagnosticsWithFileName {
                        diagnostics: Some(diagnostics),
                        ..Default::default()
                    },
                );
            }
        }
        if snapshot.semantic_diagnostics_per_file.len() == source_files().len()
            && snapshot.check_pending
            && !snapshot.options.no_check.is_true()
        {
            snapshot.check_pending = false;
        }
        snapshot.build_info_emit_pending = true;
    }

    // Go: incremental/program.go:321 emitBuildInfo
    pub(crate) fn emit_build_info(&self, options: &EmitOptions) -> Option<EmitResult> {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "emitBuildInfo",
                Vec::new(),
                true,
            )
        });
        let build_info_file_name = get_build_info_file_name(
            self.snapshot.borrow().options,
            &ComparePathsOptions {
                current_directory: get_current_directory().to_string(),
                use_case_sensitive_file_names: use_case_sensitive_file_names(),
            },
        );
        if build_info_file_name.is_empty() || is_emit_blocked(&build_info_file_name) {
            return None;
        }
        if self.snapshot.borrow().has_errors == Tristate::Unknown {
            self.ensure_has_errors_for_state();
            let mut snapshot = self.snapshot.borrow_mut();
            if snapshot.has_errors != snapshot.has_errors_from_old_state
                || snapshot.has_semantic_errors != snapshot.has_semantic_errors_from_old_state
            {
                snapshot.build_info_emit_pending = true;
            }
        }
        if self.snapshot.borrow().package_jsons.is_none() {
            self.ensure_package_jsons_for_state();
            let mut snapshot = self.snapshot.borrow_mut();
            if snapshot.package_jsons.as_deref().unwrap_or_default()
                != snapshot.package_jsons_from_old_state.as_slice()
                || snapshot
                    .missing_package_jsons
                    .as_deref()
                    .unwrap_or_default()
                    != snapshot.missing_package_jsons_from_old_state.as_slice()
            {
                snapshot.build_info_emit_pending = true;
            }
        }
        if !self.snapshot.borrow().build_info_emit_pending {
            return None;
        }
        let build_info =
            match snapshot_to_build_info(&self.snapshot.borrow(), &build_info_file_name) {
                Ok(build_info) => build_info,
                Err(err) => {
                    return Some(EmitResult {
                        emit_skipped: true,
                        diagnostics: vec![content_mapper_project_diagnostic(&err)],
                        ..EmitResult::default()
                    });
                }
            };
        let text = match marshal_build_info(&build_info) {
            Ok(text) => text,
            Err(err) => panic!("Failed to marshal build info: {err}"),
        };
        let err = if let Some(write_file) = &options.write_file {
            write_file(
                &build_info_file_name,
                &text,
                &mut WriteFileData {
                    build_info: Some(Arc::new(build_info)),
                    ..WriteFileData::default()
                },
            )
        } else {
            host()
                .fs()
                .write_file(&build_info_file_name, &text)
                .map_err(|err| fs_error_text(&err))
        };
        if let Err(err) = err {
            return Some(EmitResult {
                emit_skipped: true,
                diagnostics: vec![new_compiler_diagnostic(
                    diag::Could_not_write_file_0_Colon_1,
                    args![build_info_file_name, err],
                )],
                ..EmitResult::default()
            });
        }
        self.snapshot.borrow_mut().build_info_emit_pending = false;
        Some(EmitResult {
            emit_skipped: false,
            emitted_files: vec![build_info_file_name],
            ..EmitResult::default()
        })
    }

    // Go: incremental/program.go:386 ensureHasErrorsForState
    // PORT: Go `program` is the current program.
    fn ensure_has_errors_for_state(&self) {
        let files = source_files();
        let has_include_processing_diagnostics: Box<dyn Fn() -> bool>;
        let mut has_emit_diagnostics = false;
        let (can_use_incremental_state, is_incremental) = {
            let snapshot = self.snapshot.borrow();
            (
                snapshot.can_use_incremental_state(),
                snapshot.options.is_incremental(),
            )
        };
        if can_use_incremental_state {
            let mut found_include_processing_diagnostics: Option<bool> = None;
            {
                let snapshot = self.snapshot.borrow();
                if files.iter().any(|&file| {
                    if snapshot
                        .emit_diagnostics_per_file
                        .contains_key(source_file_info(file).path.as_str())
                    {
                        // emit diagnostics will be encoded in buildInfo;
                        return true;
                    }
                    if found_include_processing_diagnostics.is_none()
                        && !get_include_processor_diagnostics(file).is_empty()
                    {
                        found_include_processing_diagnostics = Some(true);
                    }
                    false
                }) {
                    has_emit_diagnostics = true;
                }
            }
            let value = found_include_processing_diagnostics.unwrap_or(false);
            has_include_processing_diagnostics = Box::new(move || value);
        } else {
            has_emit_diagnostics = self.snapshot.borrow().has_emit_diagnostics;
            let files = files.clone();
            has_include_processing_diagnostics = Box::new(move || {
                files
                    .iter()
                    .any(|&file| !get_include_processor_diagnostics(file).is_empty())
            });
        }

        if has_emit_diagnostics {
            let mut snapshot = self.snapshot.borrow_mut();
            // Record this for only non incremental build info
            snapshot.has_errors = if is_incremental {
                Tristate::False
            } else {
                Tristate::True
            };
            // Dont need to encode semantic errors state since the emit diagnostics are encoded
            snapshot.has_semantic_errors = false;
            return;
        }

        if has_include_processing_diagnostics()
            || !get_config_file_parsing_diagnostics().is_empty()
            || !get_syntactic_diagnostics(Node::NIL).is_empty()
            || !get_program_diagnostics().is_empty()
            || !get_global_diagnostics().is_empty()
        {
            let mut snapshot = self.snapshot.borrow_mut();
            snapshot.has_errors = Tristate::True;
            // Dont need to encode semantic errors state since the syntax and program diagnostics are encoded as present
            snapshot.has_semantic_errors = false;
            return;
        }

        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.has_errors = Tristate::False;
        // Check semantic and emit diagnostics first as we dont need to ask program about it
        let has_semantic_diagnostics = files.iter().any(|&file| {
            match snapshot
                .semantic_diagnostics_per_file
                .get(source_file_info(file).path.as_str())
            {
                // Missing semantic diagnostics in cache will be encoded in incremental buildInfo
                None => is_incremental,
                Some(semantic_diagnostics) => {
                    // cached semantic diagnostics will be encoded in buildInfo
                    semantic_diagnostics
                        .diagnostics
                        .as_ref()
                        .is_some_and(|diagnostics| !diagnostics.is_empty())
                        || !semantic_diagnostics.build_info_diagnostics.is_empty()
                }
            }
        });
        if has_semantic_diagnostics {
            // Because semantic diagnostics are recorded in buildInfo, we dont need to encode hasErrors in incremental buildInfo
            // But encode as errors in non incremental buildInfo
            snapshot.has_semantic_errors = !is_incremental;
        }
    }

    // Go: incremental/program.go:453 ensurePackageJsonsForState
    // PORT: Go appends to the snapshot slices inside the callback. The
    // callback here fills local lists, so the snapshot is not borrowed while
    // the file system runs.
    fn ensure_package_jsons_for_state(&self) {
        let (mut package_jsons, mut missing_package_jsons) = {
            let mut snapshot = self.snapshot.borrow_mut();
            (
                snapshot.package_jsons.take().unwrap_or_default(),
                snapshot.missing_package_jsons.take().unwrap_or_default(),
            )
        };
        let config = get_directory_path(command_line().config_name());
        if !config.is_empty() {
            package_json_cache_entries(|_key, value| {
                let mut package_json = combine_paths(value.package_directory, &["package.json"]);
                if value.exists || value.directory_exists {
                    package_json = host().fs().realpath(&package_json);
                }
                if value.exists {
                    package_jsons.push(package_json);
                } else if package_json.contains("/node_modules/") {
                    missing_package_jsons.push(package_json);
                }
                true
            });
        }
        let mut snapshot = self.snapshot.borrow_mut();
        snapshot.package_jsons = Some(normalize_package_jsons(package_jsons));
        snapshot.missing_package_jsons = Some(normalize_package_jsons(missing_package_jsons));
    }

    // Go: incremental/program.go:484 PackageJsonLookupPaths
    #[must_use]
    pub fn package_json_lookup_paths(&self) -> Vec<String> {
        let config = get_directory_path(command_line().config_name());
        if config.is_empty() {
            return Vec::new();
        }

        let mut package_jsons = Vec::new();
        package_json_cache_entries(|_key, value| {
            let mut package_json = combine_paths(value.package_directory, &["package.json"]);
            if value.exists || value.directory_exists {
                package_json = host().fs().realpath(&package_json);
            }
            package_jsons.push(package_json);
            true
        });
        stable_sort_by(&mut package_jsons, Ord::cmp);
        package_jsons.dedup();
        package_jsons
    }
}

// Go: incremental/program.go:476 normalizePackageJsons
// PORT: Go returns a new empty slice for nil. The list is sorted, so
// `dedup` gives the same result as Go `core.Deduplicate`.
fn normalize_package_jsons(mut package_jsons: Vec<String>) -> Vec<String> {
    stable_sort_by(&mut package_jsons, Ord::cmp);
    package_jsons.dedup();
    package_jsons
}

// Go: compiler/program.go:1999 HandleNoEmitOptions
// HandleNoEmitOptions mirrors tsc's handleNoEmitOptions.
// PORT: #4407 replaced Go `HandleNoEmitOnError`.
// `emitter::program_emit::handle_no_emit_options` is the plain program form
// (nil `emitBuildInfo`). Go passes the `ProgramLike`, whose bind and
// semantic diagnostics are the incremental ones here, so this is the same
// body over `ProgramLike`. Go `files` nil is `None`.
#[must_use]
pub fn handle_no_emit_options(
    program: &dyn ProgramLike,
    files: Option<&[Node]>,
    emit_build_info: Option<&dyn Fn() -> Option<EmitResult>>,
) -> Option<EmitResult> {
    if !program.options().no_emit.is_true() {
        if !program.options().no_emit_on_error.is_true() {
            return None; // NoEmit is false and NoEmitOnError is also false, so we can proceed with normal emit
        }

        let diagnostics = get_diagnostics_of_any_program(
            files,
            true,
            &mut |file| program.get_bind_diagnostics(file),
            &mut |file| program.get_semantic_diagnostics(file),
            &mut || program.get_global_diagnostics(),
            &mut |file| program.get_declaration_diagnostics(file),
            // ts#64452: the only caller passes the incremental program (Go
            // `program.(*Program)` fails).
            false,
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
    if let Some(emit_build_info) = emit_build_info
        && let Some(result) = emit_build_info()
    {
        return Some(result);
    }
    Some(EmitResult::default())
}

// Go: compiler/program.go:1980 ProgramLike (var _ compiler.ProgramLike = (*Program)(nil))
impl ProgramLike for Program {
    fn options(&self) -> &'static CompilerOptions {
        Program::options(self)
    }
    fn as_incremental_program(&self) -> Option<&Program> {
        Some(self)
    }
    fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        Program::get_bind_diagnostics(self, file)
    }
    fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        Program::get_global_diagnostics(self)
    }
    fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        Program::get_semantic_diagnostics(self, file)
    }
    fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        Program::get_declaration_diagnostics(self, file)
    }
    fn emit(&self, options: EmitOptions) -> EmitResult {
        Program::emit(self, options)
    }
}
