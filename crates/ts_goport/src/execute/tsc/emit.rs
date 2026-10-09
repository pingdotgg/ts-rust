//! Go: execute/tsc/emit.go (emit a program and report its diagnostics).
//!
//! PORT: Go `compiler.ProgramLike` (compiler/program.go:1710) is the
//! `ProgramLike` trait below, because this is its first user in the port.
//! `CompilerProgram` is the plain `*compiler.Program` over the current
//! program (`prog()`); the incremental program implements the same trait.
//! Go `EmitInput.Program` (the underlying `*compiler.Program`) is the
//! current program and is not a field.

use crate::prelude::*;

use crate::diagnostics_loc::message_localize;
use crate::emitter::program_emit::{EmitOptions, EmitResult, WriteFile, emit};
use crate::frontend::tsoptions::ParsedCommandLine;
use crate::locale::Locale;

use super::compile::{CompileAndEmitResult, CompileTimes, ExitStatus, System, Writer, write_str};
use super::diagnostics::{DiagnosticReporter, DiagnosticsReporter};
use super::statistics::{Statistics, read_mem_stats, statistics_from_program};

// PORT: testing (`CommandLineTesting`)
use super::compile::CommandLineTesting;
use crate::frontend::tspath::Path;
use std::sync::Mutex;
use std::time::SystemTime;

// Go: compiler/program.go:1980 ProgramLike
// PORT: only the methods that `EmitFilesAndReportErrors` and
// `GetDiagnosticsOfAnyProgram` call. Config, syntactic and program
// diagnostics are read from the current program by
// `get_diagnostics_of_any_program`; Go `incremental.Program` forwards them
// to its program too. Go passes a context; the port has none. The methods
// take `&self` like Go interface methods; an implementation with state
// (the incremental snapshot) uses interior mutability.
pub trait ProgramLike {
    fn options(&self) -> &'static CompilerOptions;
    fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic>;
    fn get_global_diagnostics(&self) -> Vec<Diagnostic>;
    fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic>;
    fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic>;
    fn emit(&self, options: EmitOptions) -> EmitResult;

    /// PORT: Go `programLike.(*incremental.Program)` (execute/tsc/emit.go).
    /// Only the incremental program returns itself.
    fn as_incremental_program(&self) -> Option<&crate::execute::incremental::program::Program> {
        None
    }
}

/// Go `*compiler.Program` as a `ProgramLike`: the current program.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompilerProgram;

impl ProgramLike for CompilerProgram {
    // Go: compiler/program.go Options
    fn options(&self) -> &'static CompilerOptions {
        options()
    }
    // Go: compiler/program.go:811 GetBindDiagnostics
    fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        get_bind_diagnostics(file)
    }
    // Go: compiler/program.go GetGlobalDiagnostics
    fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        get_global_diagnostics()
    }
    // Go: compiler/program.go:822 GetSemanticDiagnostics
    fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        get_semantic_diagnostics(file)
    }
    // Go: compiler/program.go GetDeclarationDiagnostics
    fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        get_declaration_diagnostics(file)
    }
    // Go: compiler/program.go Emit
    fn emit(&self, options: EmitOptions) -> EmitResult {
        emit(options)
    }
}

// Go: execute/tsc/emit.go:21 GetTraceWithWriterFromSys
pub fn get_trace_with_writer_from_sys(
    w: Writer,
    locale: Locale,
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> Rc<dyn Fn(&'static crate::diagnostics::Message, Vec<String>)> {
    // PORT: testing
    if let Some(testing) = testing {
        return testing.get_trace(w, locale);
    }
    Rc::new(
        move |msg: &'static crate::diagnostics::Message, args: Vec<String>| {
            write_str(&w, &format!("{}\n", message_localize(msg, &locale, &args)));
        },
    )
}

// Go: execute/tsc/emit.go:31 EmitInput
// PORT: `Program` is the current program (see the module comment).
// `Testing` and `TestingMTimesCache` are `None` outside Go tests; the cache
// is the build host `m_times` (a `Mutex`, Go `SyncMap`). `Tracing` is the
// process session (`crate::tracing::get`). Go `Config` is optional here:
// without it, the program options are used, which are the config options
// with the command line applied.
pub struct EmitInput<'a> {
    pub sys: &'a dyn System,
    pub program_like: &'a dyn ProgramLike,
    pub config: Option<&'a ParsedCommandLine>,
    pub report_diagnostic: DiagnosticReporter,
    pub report_error_summary: DiagnosticsReporter,
    pub writer: Writer,
    pub write_file: Option<WriteFile>,
    pub compile_times: Rc<RefCell<CompileTimes>>,
    // PORT: testing
    pub testing: Option<Rc<dyn CommandLineTesting>>,
    pub testing_m_times_cache: Option<&'a Mutex<FxHashMap<Path, Option<SystemTime>>>>,
}

impl EmitInput<'_> {
    /// Go `input.Config.CompilerOptions()`.
    fn config_options(&self) -> &CompilerOptions {
        match self.config {
            Some(config) => config.compiler_options(),
            None => self.program_like.options(),
        }
    }

    /// Go `input.Config.Locale()`. Without a config, it is parsed from the
    /// program options as Go `(*ParsedCommandLine).Locale` does.
    fn config_locale(&self) -> Locale {
        match self.config {
            Some(config) => config.locale(),
            None => crate::locale::parse(&self.program_like.options().locale).0,
        }
    }
}

// Go: execute/tsc/emit.go:46 EmitAndReportStatistics
pub fn emit_and_report_statistics(input: &EmitInput) -> (CompileAndEmitResult, Option<Statistics>) {
    let mut statistics = None;
    let mut result = emit_files_and_report_errors(input);
    if result.status != ExitStatus::Success {
        // compile exited early
        return (result, None);
    }
    result.times.borrow_mut().total_time = input.sys.since_start();

    if input.config_options().diagnostics.is_true()
        || input.config_options().extended_diagnostics.is_true()
    {
        // PORT: Go runs the GC twice and reads `runtime.MemStats`; see
        // `read_mem_stats`.
        let mem_stats = read_mem_stats();
        let program_statistics = statistics_from_program(&result.times.borrow(), &mem_stats);
        program_statistics.report_to(&input.writer, input.testing.clone());
        statistics = Some(program_statistics);
    }

    // Effect-TS/tsgo patch 009: Effect diagnostics that the plugin options
    // ignore for the exit code are still reported but do not count here.
    let diagnostics_for_exit_code = crate::effect::filter_diagnostics_for_exit_code(
        input.config_options().effect.as_deref(),
        &result.diagnostics,
    );
    if result.emit_result.emit_skipped && !diagnostics_for_exit_code.is_empty() {
        result.status = ExitStatus::DiagnosticsPresentOutputsSkipped;
    } else if !diagnostics_for_exit_code.is_empty() {
        result.status = ExitStatus::DiagnosticsPresentOutputsGenerated;
    }
    (result, statistics)
}

/// Go `input.Sys.Now().Sub(start)`. A clock that goes back gives zero.
fn since(sys: &dyn System, start: SystemTime) -> std::time::Duration {
    sys.now().duration_since(start).unwrap_or_default()
}

// Go: execute/tsc/emit.go:74 EmitFilesAndReportErrors
// PORT: Go times each bind, check and emit call with `sys.Now()`, and so
// does the port. When the incremental program started the check before
// (`Program::start_check`, `tsc -p` and `tsc -b`), the check time adds the
// time of that start (`Program::take_started_check_time`), and the check
// call times the wait for the rest.
pub fn emit_files_and_report_errors(input: &EmitInput) -> CompileAndEmitResult {
    let mut result = CompileAndEmitResult::default();
    result.times = input.compile_times.clone();
    let program_like = input.program_like;
    let times = result.times.clone();

    let mut all_diagnostics = get_diagnostics_of_any_program(
        None, // #4699: Go nil files
        false,
        &mut |file| {
            // Options diagnostics include global diagnostics (even though we collect them separately),
            // and global diagnostics create checkers, which then bind all of the files. Do this binding
            // early so we can track the time.
            let _trace = crate::tracing::get().map(|tr| {
                tr.push(
                    crate::tracing::Phase::Bind,
                    "bindSourceFiles",
                    Vec::new(),
                    true,
                )
            });
            let bind_start = input.sys.now();
            let diags = program_like.get_bind_diagnostics(file);
            times.borrow_mut().bind_time = since(input.sys, bind_start);
            diags
        },
        &mut |file| {
            let _trace = crate::tracing::get().map(|tr| {
                tr.push(
                    crate::tracing::Phase::Check,
                    "checkSourceFiles",
                    Vec::new(),
                    true,
                )
            });
            let check_start = input.sys.now();
            let diags = program_like.get_semantic_diagnostics(file);
            times.borrow_mut().check_time = since(input.sys, check_start);
            if let Some(program) = program_like.as_incremental_program() {
                times.borrow_mut().check_time += program.take_started_check_time();
                let nested_emit_time = program.take_nested_emit_time();
                let mut times = times.borrow_mut();
                if nested_emit_time > times.check_time {
                    times.check_time = std::time::Duration::ZERO;
                } else {
                    times.check_time -= nested_emit_time;
                }
                times.emit_time += nested_emit_time;
            }
            diags
        },
        &mut || program_like.get_global_diagnostics(),
        &mut |file| program_like.get_declaration_diagnostics(file),
        // ts#64452: Go `program.(*compiler.Program)`.
        program_like.as_incremental_program().is_none(),
    );

    let mut emit_result = EmitResult {
        emit_skipped: true,
        diagnostics: Vec::new(),
        ..EmitResult::default()
    };
    if !program_like.options().list_files_only.is_true() {
        let emit_start = input.sys.now();
        emit_result = program_like.emit(EmitOptions {
            write_file: input.write_file.clone(),
            ..EmitOptions::default()
        });
        result.times.borrow_mut().emit_time += since(input.sys, emit_start);
    }
    all_diagnostics.extend(emit_result.diagnostics.iter().cloned());
    // PORT: testing
    if let Some(testing) = &input.testing {
        testing.on_emitted_files(&emit_result, input.testing_m_times_cache);
    }

    let all_diagnostics = sort_and_deduplicate_diagnostics(all_diagnostics);
    // PORT: one report (`stdio::keep_writes`).
    let report = super::stdio::keep_writes();
    for diagnostic in &all_diagnostics {
        (input.report_diagnostic)(diagnostic);
    }

    list_files(input, &emit_result);

    (input.report_error_summary)(&all_diagnostics);
    drop(report);
    result.diagnostics = all_diagnostics;
    result.emit_result = emit_result;
    result.status = ExitStatus::Success;
    result
}

// Go: execute/tsc/emit.go:144 listFiles
// PORT: Go `fmt.Fprintln(w, "TSFILE:", x)` puts a space between operands.
fn list_files(input: &EmitInput, emit_result: &EmitResult) {
    // PORT: testing. Go `defer input.Testing.OnListFilesEnd(input.Writer)`
    // is at the end (this function has no early return).
    if let Some(testing) = &input.testing {
        testing.on_list_files_start(&input.writer);
    }
    let options = options();
    if options.list_emitted_files.is_true() {
        for file in &emit_result.emitted_files {
            write_str(
                &input.writer,
                &format!(
                    "TSFILE: {}\n",
                    crate::frontend::tspath::get_normalized_absolute_path(
                        file,
                        get_current_directory()
                    )
                ),
            );
        }
    }
    if options.explain_files.is_true() {
        // ts#64159: the names are relative to the system's current directory
        // (execute/tsc/emit.go:156).
        crate::program::explain_files_relative_to(
            &mut *input.writer.borrow_mut(),
            &input.config_locale(),
            &input.sys.get_current_directory(),
        );
    } else if options.list_files.is_true() || options.list_files_only.is_true() {
        for file in source_files() {
            write_str(&input.writer, &format!("{}\n", source_file_file_name(file)));
        }
    }
    // PORT: testing
    if let Some(testing) = &input.testing {
        testing.on_list_files_end(&input.writer);
    }
}
