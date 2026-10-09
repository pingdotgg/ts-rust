//! Go: execute/tsc.go, the `tsc` command line: `CommandLine`,
//! `tscBuildCompilation`, `tscCompilation`, `findConfigFile`,
//! `getTraceFromSys`, `performIncrementalCompilation`, `performCompilation`
//! and `showConfig`. `startTracingIfNeeded` and `stopTracing` write the
//! warnings of the process session in `crate::tracing`.
//!
//! Every bin runs this code. `tsgo` runs it as Go does (`GoTsc`).
//! `goport` and `goport_emit` pass their own `TscCompilationHooks`.
//! `goport_build` runs `tsc_build_compilation`.
//!
//! PORT: Go `ctx` reaches watch mode (execute/watcher.rs) and the build
//! orchestrator. Go `testing` is `TscCompilationHooks::testing` (`None`
//! outside tests, see compile.rs).
//!
//! PORT: a plain `tsc` run has one program for the process, so Go
//! `compiler.NewProgram` is `crate::program::install_new_program` (see
//! PORTING.md "Program"). Call `tsc_compilation` once per process, on the
//! thread that should own the program and the checker pool. A `-b` build
//! makes one program version per project instead (build/build_task.rs). Go
//! catches no panic here; the bins guard unported code.

use crate::frontend::prelude::*;

use std::sync::Arc;
use std::time::SystemTime;

use crate::contentmapper::{
    Host as ContentMapperHost, Project as ContentMapperProject,
    ProjectSpec as ContentMapperProjectSpec,
};
use crate::emitter::program_emit::{EmitOptions, WriteFile, WriteFileData};
use crate::execute::build::command_line::parse_build_command_line;
use crate::execute::build::host::TscExtendedConfigCache;
use crate::execute::build::orchestrator::{Options as OrchestratorOptions, new_orchestrator};
use crate::execute::incremental::emit_files::fs_error_text;
use crate::execute::incremental::incremental::{create_host, new_build_info_reader};
use crate::execute::incremental::program::{
    NestedEmitNow, Program as IncrementalProgram, new_program as new_incremental_program,
    read_build_info_program,
};
use crate::execute::tsc::{
    CommandLineResult, CompileTimes, CompilerProgram, DiagnosticReporter, DiagnosticsReporter,
    EmitInput, ExitStatus, ProgramLike, System, SystemParseConfigHost, create_diagnostic_reporter,
    create_report_error_summary, emit_and_report_statistics, get_trace_with_writer_from_sys,
    new_content_mapper_host, print_build_help, print_help, print_version, write_config_file,
    write_str,
};
#[cfg(not(target_family = "wasm"))]
use crate::execute::watcher::create_watcher;
use crate::frontend::json::json_marshal_indent_write;
use crate::frontend::tsoptions::convert_to_ts_config;
use crate::gostd::Context;
// PORT: testing
use crate::execute::tsc::CommandLineTesting;

/// The bin part of the compile step of `tscCompilation`.
// PORT: no Go equivalent. Go has one `tsc`. `tsgo` uses the defaults
// (`GoTsc`). `goport` and `goport_emit` run on read-only project inputs,
// so they change the config, guard each program step and write only where
// they allow.
pub trait TscCompilationHooks {
    /// Whether a `-b` command line runs Go `tscBuildCompilation`. When
    /// false, it is `unported!`: the build writes outputs.
    fn build_mode(&self) -> bool {
        true
    }

    /// Runs after the command line parse, before `tscCompilation` reads the
    /// result. The bin may edit it. The default keeps it as parsed.
    fn command_line_parsed(&self, _command_line: &mut ParsedCommandLine) {}

    /// Runs when the compile step starts, after the init, version, help and
    /// showConfig branches, before the compiler host and `NewProgram`.
    /// `config` is the config for the compilation, and the bin may edit
    /// it. `config_file_name` is "" when there is no config file. An `Err`
    /// ends the run with that status.
    fn prepare_compilation(
        &self,
        _sys: &dyn System,
        _command_line_options: &CompilerOptions,
        _config_file_name: &str,
        _config: &mut ParsedCommandLine,
    ) -> Result<(), ExitStatus> {
        Ok(())
    }

    /// Runs after `NewProgram`, before `EmitAndReportStatistics`. An `Err`
    /// ends the run with that status.
    fn program_created(&self) -> Result<(), ExitStatus> {
        Ok(())
    }

    /// The Go `ProgramLike` that `EmitAndReportStatistics` gets. `None` is
    /// Go's: the program, or for an incremental config the
    /// `incremental.Program`. `Some` replaces both, and then the
    /// incremental compile does not call `incremental.ReadBuildInfoProgram`
    /// or `incremental.NewProgram`, so it reads and writes no build info.
    fn program_like(&self) -> Option<&dyn ProgramLike> {
        None
    }

    /// The Go host `WriteFile` for the emit. `None` is the emit host
    /// default, which refuses every write (program.rs emitHost
    /// `write_file`).
    fn write_file(&self) -> Option<WriteFile>;

    /// Go `testing tsc.CommandLineTesting` of `CommandLine`. `None` is the
    /// Go nil that every bin passes; a test passes its test system.
    // PORT: testing. Go passes it as a parameter; a hook keeps the bins
    // unchanged.
    fn testing(&self) -> Option<Rc<dyn CommandLineTesting>> {
        None
    }

    /// The reporter of each diagnostic in a compile that is not `-b`.
    /// `reporter` is Go's (`CreateDiagnosticReporter`), which writes the
    /// diagnostic text to the system writer. The default keeps it. The
    /// wasm package (crates/ts_wasm) returns one that keeps the diagnostics
    /// as data.
    // PORT: not in Go.
    fn diagnostic_reporter(&self, reporter: DiagnosticReporter) -> DiagnosticReporter {
        reporter
    }
}

/// Go `tsc`: no bin step, the Go program, and emit writes through the OS
/// file system. `tsgo` runs this.
pub struct GoTsc;

impl TscCompilationHooks for GoTsc {
    fn write_file(&self) -> Option<WriteFile> {
        Some(os_write_file())
    }
}

/// `sys.Now().Sub(start)`.
fn since(sys: &dyn System, start: SystemTime) -> std::time::Duration {
    sys.now().duration_since(start).unwrap_or_default()
}

/// `tsc.CommandLineResult{Status: status}`.
fn result(status: ExitStatus) -> CommandLineResult {
    CommandLineResult {
        status,
        watcher: None,
    }
}

/// Ends a run that asks for `option`, which the wasm build cannot run:
/// watch mode needs threads and file events, and `--pprofDir` profiles the
/// process. crates/ts_wasm refuses both options before tsc runs, with the
/// same text and status; a response file (`@file`) can still set them.
/// The gates that call this let the link leave the watch and profile code
/// out of the wasm module.
// PORT: not in Go.
#[cfg(target_family = "wasm")]
fn wasm_unsupported(option: &str) -> CommandLineResult {
    eprintln!("error: {option} is not supported by the wasm build");
    result(ExitStatus::DiagnosticsPresentOutputsSkipped)
}

// Go: execute/tsc.go:28 startTracingIfNeeded, the warning part. The session
// is the process session in `crate::tracing`.
// PORT: testing. `testing` is Go `testing != nil`.
fn start_tracing_if_needed(sys: &dyn System, config: &ParsedCommandLine, testing: bool) {
    if let Some(warning) = crate::tracing::start_tracing_if_needed(config, testing) {
        write_str(&sys.writer(), &warning);
    }
}

// Go: execute/tsc.go:44 stopTracing
fn stop_tracing(sys: &dyn System) {
    if let Some(warning) = crate::tracing::stop_tracing() {
        write_str(&sys.writer(), &warning);
    }
}

// Go: execute/tsc.go:53 CommandLine
// PORT: Go parses the build command line here and passes it on; the port
// passes the arguments, because `goport_build` calls
// `tsc_build_compilation` with them too. `hooks.build_mode` and
// `hooks.command_line_parsed` are the bin's steps (not in Go).
pub fn command_line(
    ctx: &Context,
    sys: Rc<dyn System>,
    command_line_args: &[String],
    hooks: &dyn TscCompilationHooks,
) -> CommandLineResult {
    // Effect-TS/tsgo patch 009: Effect rules know they run under tsc.
    let _effect_cli_mode = crate::effect::etscore::enter_command_line_mode();
    if let Some(first) = command_line_args.first() {
        match first.to_lowercase().as_str() {
            "-b" | "--b" | "-build" | "--build" => {
                if !hooks.build_mode() {
                    unported!("tscBuildCompilation");
                }
                return tsc_build_compilation(ctx, sys, command_line_args, hooks.testing());
            }
            // case "-f":
            // 	return fmtMain(sys, commandLineArgs[1], commandLineArgs[1])
            _ => {}
        }
    }

    let mut parsed = parse_command_line(command_line_args, &SystemParseConfigHost(&*sys));
    hooks.command_line_parsed(&mut parsed);
    tsc_compilation(ctx, sys, parsed, hooks)
}

// Go: execute/tsc.go:66 fmtMain
// PORT: not ported. Its only call (the `-f` case in `CommandLine`) is
// commented out in Go, so it is dead code, and the formatter is not ported.

// Go: execute/tsc.go:92 tscBuildCompilation
// PORT: Go `CommandLine` parses the build command line and passes it in;
// here it is the first step, which runs in the same order.
// `command_line_args` is the full command line (Go `commandLineArgs`).
// PORT: the orchestrator compiles every project in this process, on this
// thread (build/build_task.rs).
pub fn tsc_build_compilation(
    ctx: &Context,
    sys: Rc<dyn System>,
    command_line_args: &[String],
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> CommandLineResult {
    let build_command = parse_build_command_line(command_line_args, &SystemParseConfigHost(&*sys));
    let locale = build_command.locale();
    let report_diagnostic = create_diagnostic_reporter(
        &*sys,
        sys.writer(),
        &locale,
        &build_command.compiler_options,
    );

    if !build_command.errors.is_empty() {
        for err in &build_command.errors {
            report_diagnostic(err);
        }
        return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
    }

    // PORT: not in Go. A wasm build cannot watch or profile (see
    // `wasm_unsupported`).
    #[cfg(target_family = "wasm")]
    if build_command.compiler_options.watch.is_true() {
        return wasm_unsupported("--watch");
    }
    #[cfg(target_family = "wasm")]
    if !build_command.compiler_options.pprof_dir.is_empty() {
        return wasm_unsupported("--pprofDir");
    }

    // PORT: Go `defer profileSession.Stop()`. The session stops when it
    // drops at the end of this function (see `crate::pprof`).
    #[cfg(not(target_family = "wasm"))]
    let _profile_session = if build_command.compiler_options.pprof_dir.is_empty() {
        None
    } else {
        // !!! stderr?
        Some(crate::pprof::begin_profiling(
            &build_command.compiler_options.pprof_dir,
            sys.writer(),
        ))
    };

    if build_command.compiler_options.help.is_true() {
        print_version(&*sys, &locale);
        print_build_help(&*sys, &locale, BUILD_OPTS.as_slice());
        return result(ExitStatus::Success);
    }

    let orchestrator = Box::new(new_orchestrator(OrchestratorOptions {
        sys,
        command: Rc::new(build_command),
        testing,
    }));
    orchestrator.start_exported(ctx)
}

// Go: execute/tsc.go:123 tscCompilation
// PORT: `hooks.prepare_compilation` is the bin's step (not in Go).
pub fn tsc_compilation(
    ctx: &Context,
    sys: Rc<dyn System>,
    command_line: ParsedCommandLine,
    hooks: &dyn TscCompilationHooks,
) -> CommandLineResult {
    // PORT: testing. Go `testing` parameter.
    let testing = hooks.testing();
    let mut config_file_name = String::new();
    let locale = command_line.locale();
    let mut report_diagnostic: DiagnosticReporter =
        hooks.diagnostic_reporter(create_diagnostic_reporter(
            &*sys,
            sys.writer(),
            &locale,
            command_line.compiler_options(),
        ));

    if !command_line.errors.is_empty() {
        for e in &command_line.errors {
            report_diagnostic(e);
        }
        return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
    }

    // PORT: not in Go. A wasm build cannot profile (see `wasm_unsupported`).
    #[cfg(target_family = "wasm")]
    if !command_line.compiler_options().pprof_dir.is_empty() {
        return wasm_unsupported("--pprofDir");
    }

    // PORT: Go `defer profileSession.Stop()`. The session stops when it
    // drops at the end of this function (see `crate::pprof`).
    #[cfg(not(target_family = "wasm"))]
    let _profile_session = if command_line.compiler_options().pprof_dir.is_empty() {
        None
    } else {
        // !!! stderr?
        Some(crate::pprof::begin_profiling(
            &command_line.compiler_options().pprof_dir,
            sys.writer(),
        ))
    };

    if command_line.compiler_options().init.is_true() {
        // Go: `commandLine.Raw.(*collections.OrderedMap[string, any])`, a
        // type assertion that panics on another type. `parse_command_line`
        // always stores a map.
        let CompilerOptionsValue::Map(raw) = &command_line.raw else {
            panic!(
                "interface conversion: commandLine.Raw is not *collections.OrderedMap[string,any]"
            );
        };
        write_config_file(&*sys, &locale, &report_diagnostic, raw);
        return result(ExitStatus::Success);
    }

    if command_line.compiler_options().version.is_true() {
        print_version(&*sys, &locale);
        return result(ExitStatus::Success);
    }

    if command_line.compiler_options().help.is_true()
        || command_line.compiler_options().all.is_true()
    {
        print_help(&*sys, &locale, &command_line);
        return result(ExitStatus::Success);
    }

    if command_line.compiler_options().watch.is_true()
        && command_line.compiler_options().list_files_only.is_true()
    {
        report_diagnostic(&new_compiler_diagnostic(
            diag::Options_0_and_1_cannot_be_combined,
            args!["watch", "listFilesOnly"],
        ));
        return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
    }

    if !command_line.compiler_options().project.is_empty() {
        if !command_line.file_names().is_empty() {
            report_diagnostic(&new_compiler_diagnostic(
                diag::Option_project_cannot_be_mixed_with_source_files_on_a_command_line,
                Vec::new(),
            ));
            return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
        }

        let file_or_directory = normalize_path(&command_line.compiler_options().project);
        if sys.fs().directory_exists(&file_or_directory) {
            config_file_name = combine_paths(&file_or_directory, &["tsconfig.json"]);
            if !sys.fs().file_exists(&config_file_name) {
                report_diagnostic(&new_compiler_diagnostic(
                    diag::Cannot_find_a_tsconfig_json_file_at_the_current_directory_Colon_0,
                    args![config_file_name],
                ));
                return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
            }
        } else {
            config_file_name = file_or_directory.clone();
            if !sys.fs().file_exists(&config_file_name) {
                report_diagnostic(&new_compiler_diagnostic(
                    diag::The_specified_path_does_not_exist_Colon_0,
                    args![file_or_directory],
                ));
                return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
            }
        }
    } else if !command_line.compiler_options().ignore_config.is_true()
        || command_line.file_names().is_empty()
    {
        let search_path = normalize_path(&sys.get_current_directory());
        let fs = sys.fs();
        config_file_name =
            find_config_file(&search_path, |name| fs.file_exists(name), "tsconfig.json");
        if !command_line.file_names().is_empty() {
            if !config_file_name.is_empty() {
                // Error to not specify config file
                report_diagnostic(&new_compiler_diagnostic(
                    diag::X_tsconfig_json_is_present_but_will_not_be_loaded_if_files_are_specified_on_commandline_Use_ignoreConfig_to_skip_this_error,
                    Vec::new(),
                ));
                return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
            }
        } else if config_file_name.is_empty() {
            if command_line.compiler_options().show_config.is_true() {
                report_diagnostic(&new_compiler_diagnostic(
                    diag::Cannot_find_a_tsconfig_json_file_at_the_current_directory_Colon_0,
                    args![normalize_path(&sys.get_current_directory())],
                ));
            } else {
                print_version(&*sys, &locale);
                print_help(&*sys, &locale, &command_line);
            }
            return result(ExitStatus::DiagnosticsPresentOutputsSkipped);
        }
    }

    // !!! convert to options with absolute paths is usually done here, but for ease of implementation, it's done in `tsoptions.ParseCommandLine()`
    let compiler_options_from_command_line = command_line.compiler_options().clone();
    let extended_config_cache = Rc::new(TscExtendedConfigCache::default());
    // PORT: Go `*tsc.CompileTimes` is shared with the emit result (see
    // `CompileAndEmitResult.times`).
    let compile_times = Rc::new(RefCell::new(CompileTimes::default()));
    let mut command_line_raw: Option<IndexMap<String, CompilerOptionsValue>> = None;
    let mut config_for_compilation = if !config_file_name.is_empty() {
        let config_start = sys.now();
        if let CompilerOptionsValue::Map(raw) = &command_line.raw {
            // Wrap command line options in a "compilerOptions" key to match tsconfig.json structure
            let mut wrapped = IndexMap::new();
            wrapped.insert(
                "compilerOptions".to_string(),
                CompilerOptionsValue::Map(raw.clone()),
            );
            command_line_raw = Some(wrapped);
        }
        let extended_config_cache_ref: &dyn ExtendedConfigCache = &*extended_config_cache;
        let (config_parse_result, errors) = get_parsed_command_line_of_config_file(
            &config_file_name,
            Some(&*compiler_options_from_command_line),
            command_line_raw.as_ref(),
            &SystemParseConfigHost(&*sys),
            Some(extended_config_cache_ref),
        );
        compile_times.borrow_mut().config_time = since(&*sys, config_start);
        if !errors.is_empty() {
            // these are unrecoverable errors--exit to report them as diagnostics
            for e in &errors {
                report_diagnostic(e);
            }
            return result(ExitStatus::DiagnosticsPresentOutputsGenerated);
        }
        // Updater to reflect pretty
        report_diagnostic = hooks.diagnostic_reporter(create_diagnostic_reporter(
            &*sys,
            sys.writer(),
            &locale,
            command_line.compiler_options(),
        ));
        // PORT: Go returns a non-nil config whenever there are no errors.
        config_parse_result
            .expect("GetParsedCommandLineOfConfigFile returns a config without errors")
    } else {
        command_line
    };

    let report_error_summary: DiagnosticsReporter = create_report_error_summary(
        &*sys,
        &locale,
        Some(&**config_for_compilation.compiler_options()),
    );
    if compiler_options_from_command_line.show_config.is_true() {
        show_config(&*sys, &config_for_compilation, &config_file_name);
        return result(ExitStatus::Success);
    }
    // PORT: the compile step of the bin starts here (see
    // `TscCompilationHooks::prepare_compilation`). `goport` forces noEmit
    // here, so `--showConfig` above shows the options as Go does.
    if let Err(status) = hooks.prepare_compilation(
        &*sys,
        &compiler_options_from_command_line,
        &config_file_name,
        &mut config_for_compilation,
    ) {
        return result(status);
    }
    if config_for_compilation.compiler_options().watch.is_true() {
        // PORT: not in Go. A wasm build cannot watch (see
        // `wasm_unsupported`).
        #[cfg(target_family = "wasm")]
        return wasm_unsupported("--watch");
        #[cfg(not(target_family = "wasm"))]
        {
            let mut watcher = create_watcher(
                sys,
                Rc::new(config_for_compilation),
                compiler_options_from_command_line,
                command_line_raw,
                report_diagnostic,
                report_error_summary,
                testing,
            );
            watcher.start(ctx);
            return CommandLineResult {
                status: ExitStatus::Success,
                watcher: Some(Box::new(watcher)),
            };
        }
    } else if config_for_compilation.compiler_options().is_incremental() {
        return perform_incremental_compilation(
            ctx,
            &sys,
            config_for_compilation,
            report_diagnostic,
            report_error_summary,
            extended_config_cache,
            compile_times,
            testing,
            hooks,
        );
    }
    perform_compilation(
        ctx,
        &sys,
        config_for_compilation,
        report_diagnostic,
        report_error_summary,
        extended_config_cache,
        compile_times,
        testing,
        hooks,
    )
}

// Go: execute/tsc.go:274 findConfigFile
fn find_config_file(
    search_path: &str,
    file_exists: impl Fn(&str) -> bool,
    config_name: &str,
) -> String {
    let (result, ok) = for_each_ancestor_directory(search_path, |ancestor| {
        let full_config_name = combine_paths(ancestor, &[config_name]);
        if file_exists(full_config_name.as_str()) {
            return (full_config_name, true);
        }
        (full_config_name, false)
    });
    if !ok {
        return String::new();
    }
    result
}

// Go: execute/tsc.go:288 getTraceFromSys
pub(crate) fn get_trace_from_sys(
    sys: &dyn System,
    locale: crate::locale::Locale,
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> TraceFn {
    get_trace_with_writer_from_sys(sys.writer(), locale, testing)
}

/// Go `compiler.NewProgram(compiler.ProgramOptions{Config, Host, Tracing})`.
// PORT: the new program is installed for the process (see the module
// comment). Go `NewProgram` cannot fail; the Rust install fails only when
// the current directory cannot be read, and that ends the run.
pub(crate) fn install_program(host: Rc<dyn CompilerHost>, config: Rc<ParsedCommandLine>) {
    if let Err(message) = crate::program::install_new_program(program_options(host, config)) {
        panic!("cannot load program: {message}");
    }
}

/// Go `compiler.NewProgram(compiler.ProgramOptions{Config, Host})` in a
/// process that makes a new program for each build (watch mode, `tsc -b`):
/// the frontend program. The caller marks its new parses freeable
/// (`program::mark_freeable_parses`) and makes its program version
/// (`program::new_program_version`), which is not current, and which the
/// caller releases (`program::release_program`).
pub(crate) fn new_frontend_program(
    host: Rc<dyn CompilerHost>,
    config: Rc<ParsedCommandLine>,
) -> Rc<NewProgram> {
    // The frontend parses with no current program, like the first load.
    let _scope = crate::core::enter_program(None);
    Rc::new(crate::frontend::compiler::new_program(program_options(
        host, config,
    )))
}

/// The Go `compiler.ProgramOptions` of a tsc program.
fn program_options(host: Rc<dyn CompilerHost>, config: Rc<ParsedCommandLine>) -> ProgramOptions {
    ProgramOptions {
        host,
        config,
        use_source_of_project_reference: false,
        single_threaded: Tristate::Unknown,
        typings_location: String::new(),
        project_name: String::new(),
        // ts#64299: Go leaves `CreateModuleResolver` nil here.
        create_module_resolver: None,
        // ts#64024: Go leaves `SkipModuleResolution` false here.
        skip_module_resolution: false,
    }
}

// Go: execute/tsc.go:292 performIncrementalCompilation
// PORT: the incremental program reads back the installed program. A bin
// that replaces the program (`TscCompilationHooks::program_like`) skips
// `incremental.ReadBuildInfoProgram` and `incremental.NewProgram`: `goport`
// and `goport_emit` run on read-only Query inputs, which are composite and
// incremental, and the Go incremental program would write build info next
// to them. Their steps are still timed (empty), so the statistics table
// has the same rows as Go. Without the incremental program there is no
// `testing.OnProgram` (only a bin replaces it, and bins pass no testing).
// PORT: perf. The incremental program sends its check, and when it can,
// the emit, before
// `EmitAndReportStatistics` (`Program::start_check_and_emit`). Each checker
// then emits when its own check ends. `EmitAndReportStatistics` makes the
// same calls as in Go and waits for that work. Its check time is the time
// that `start_check` spent on the affected files plus the wait for the
// check (`Program::take_started_check_time`), and its emit time the wait
// for the rest of the emit.
// PORT: `sys_rc` is the `Rc` so that the incremental program can keep
// `sys.Now` (Go passes the method value) and the content mapper host can
// keep `sys` as its spawner. The body uses `sys: &dyn System`.
fn perform_incremental_compilation(
    ctx: &Context,
    sys_rc: &Rc<dyn System>,
    config: ParsedCommandLine,
    report_diagnostic: DiagnosticReporter,
    report_error_summary: DiagnosticsReporter,
    extended_config_cache: Rc<TscExtendedConfigCache>,
    compile_times: Rc<RefCell<CompileTimes>>,
    testing: Option<Rc<dyn CommandLineTesting>>,
    hooks: &dyn TscCompilationHooks,
) -> CommandLineResult {
    let sys: &dyn System = &**sys_rc;
    start_lib_prefetch(sys, &config, testing.is_some());
    let content_mapper_host = new_content_mapper_host(ctx, sys_rc, config.compiler_options());
    let content_mapper_project = get_content_mapper_project(content_mapper_host.as_ref(), &config);
    let _close_content_mapper_project = CloseContentMapperProject(content_mapper_project.clone());
    let host = new_cached_fs_compiler_host(
        &sys.get_current_directory(),
        sys.fs(),
        &sys.default_library_path(),
        Some(extended_config_cache as Rc<dyn ExtendedConfigCache>),
        Some(get_trace_from_sys(sys, config.locale(), testing.clone())),
        content_mapper_project,
    );
    let config = Rc::new(config);
    let replacement = hooks.program_like();
    let build_info_read_start = sys.now();
    let old_program = match replacement {
        Some(_) => None,
        None => read_build_info_program(&config, &*new_build_info_reader(host.clone()), &*host),
    };
    compile_times.borrow_mut().build_info_read_time = since(sys, build_info_read_start);

    start_tracing_if_needed(sys, &config, testing.is_some());

    let parse_start = sys.now();
    install_program(host.clone(), config.clone());
    compile_times.borrow_mut().parse_time = since(sys, parse_start);
    let changes_compute_start = sys.now();
    // Go: sys.Now
    let nested_emit_now: NestedEmitNow = {
        let sys = sys_rc.clone();
        Rc::new(move || sys.now())
    };
    let incremental_program = match replacement {
        Some(_) => None,
        None => Some(new_incremental_program(
            old_program.as_ref(),
            create_host(host),
            Some(nested_emit_now),
            testing.is_some(),
        )),
    };
    compile_times.borrow_mut().changes_compute_time = since(sys, changes_compute_start);
    if let Some(content_mapper_host) = &content_mapper_host {
        compile_times.borrow_mut().content_mapper_times = content_mapper_host.timings();
    }
    // PORT: the bin check after NewProgram (see `TscCompilationHooks`).
    if let Err(status) = hooks.program_created() {
        stop_tracing(sys);
        return result(status);
    }
    let write_file = hooks.write_file();
    if let Some(incremental_program) = &incremental_program {
        // The options of the emit call in `EmitFilesAndReportErrors`.
        incremental_program.start_check_and_emit(EmitOptions {
            write_file: write_file.clone(),
            ..EmitOptions::default()
        });
    }
    let program_like: &dyn ProgramLike = match &incremental_program {
        Some(incremental_program) => incremental_program,
        None => replacement.expect("a bin without an incremental program replaces it"),
    };
    let (emit_result, _) = emit_and_report_statistics(&EmitInput {
        sys,
        program_like,
        config: Some(&*config),
        report_diagnostic,
        report_error_summary,
        writer: sys.writer(),
        write_file,
        compile_times,
        testing: testing.clone(),
        testing_m_times_cache: None,
    });
    debug_assert!(
        incremental_program
            .as_ref()
            .is_none_or(IncrementalProgram::start_check_used),
        "the emit did not use the check and emit that started with the program"
    );

    stop_tracing(sys);

    // PORT: testing
    if let (Some(testing), Some(incremental_program)) = (&testing, &incremental_program) {
        testing.on_program(incremental_program);
    }
    result(emit_result.status)
}

// Go: execute/tsc.go:356 performCompilation
// PORT: `sys_rc` is the `Rc` so that the content mapper host can keep
// `sys` as its spawner. The body uses `sys: &dyn System`.
fn perform_compilation(
    ctx: &Context,
    sys_rc: &Rc<dyn System>,
    config: ParsedCommandLine,
    report_diagnostic: DiagnosticReporter,
    report_error_summary: DiagnosticsReporter,
    extended_config_cache: Rc<TscExtendedConfigCache>,
    compile_times: Rc<RefCell<CompileTimes>>,
    testing: Option<Rc<dyn CommandLineTesting>>,
    hooks: &dyn TscCompilationHooks,
) -> CommandLineResult {
    let sys: &dyn System = &**sys_rc;
    start_lib_prefetch(sys, &config, testing.is_some());
    let content_mapper_host = new_content_mapper_host(ctx, sys_rc, config.compiler_options());
    let content_mapper_project = get_content_mapper_project(content_mapper_host.as_ref(), &config);
    let _close_content_mapper_project = CloseContentMapperProject(content_mapper_project.clone());
    let host = new_cached_fs_compiler_host(
        &sys.get_current_directory(),
        sys.fs(),
        &sys.default_library_path(),
        Some(extended_config_cache as Rc<dyn ExtendedConfigCache>),
        Some(get_trace_from_sys(sys, config.locale(), testing.clone())),
        content_mapper_project,
    );
    let config = Rc::new(config);

    start_tracing_if_needed(sys, &config, testing.is_some());

    let parse_start = sys.now();
    install_program(host, config.clone());
    compile_times.borrow_mut().parse_time = since(sys, parse_start);
    if let Some(content_mapper_host) = &content_mapper_host {
        compile_times.borrow_mut().content_mapper_times = content_mapper_host.timings();
    }
    // PORT: the bin check after NewProgram (see `TscCompilationHooks`).
    if let Err(status) = hooks.program_created() {
        stop_tracing(sys);
        return result(status);
    }
    let (emit_result, _) = emit_and_report_statistics(&EmitInput {
        sys,
        program_like: hooks.program_like().unwrap_or(&CompilerProgram),
        config: Some(&*config),
        report_diagnostic,
        report_error_summary,
        writer: sys.writer(),
        write_file: hooks.write_file(),
        compile_times,
        testing,
        testing_m_times_cache: None,
    });

    stop_tracing(sys);

    result(emit_result.status)
}

/// Starts the parse of the default lib files of `config` on the parse
/// workers, before the compiler host, the build info read and the loader
/// setup (`start_default_lib_prefetch`). The program loaded next on this
/// thread takes the parses; its loader checks each one. `tsc -b` calls it
/// too (`BuildTask::compile_and_emit_start`).
// PORT: not in Go (see `start_default_lib_prefetch`). A test run
// (`testing`) keeps the Go order.
pub(crate) fn start_lib_prefetch(sys: &dyn System, config: &ParsedCommandLine, testing: bool) {
    if testing {
        return;
    }
    start_default_lib_prefetch(
        config,
        &sys.get_current_directory(),
        sys.fs().use_case_sensitive_file_names(),
        &sys.default_library_path(),
    );
}

/// Go `defer contentMapperProject.Close()`: closes the project when the
/// compile function returns.
struct CloseContentMapperProject(Option<Rc<dyn ContentMapperProject>>);

impl Drop for CloseContentMapperProject {
    fn drop(&mut self) {
        if let Some(project) = &self.0 {
            let _ = project.close();
        }
    }
}

// Go: execute/tsc.go:411 getContentMapperProject (tsgo#4712)
fn get_content_mapper_project(
    host: Option<&Rc<dyn ContentMapperHost>>,
    config: &ParsedCommandLine,
) -> Option<Rc<dyn ContentMapperProject>> {
    let host = host?;
    if config.content_mappers().is_empty() {
        return None;
    }
    host.project(ContentMapperProjectSpec {
        config_file_name: config.config_name().to_string(),
        mappers: config.content_mappers().to_vec(),
        compiler_options: Some(config.compiler_options().clone()),
    })
}

// Go: execute/tsc.go:422 showConfig
fn show_config(sys: &dyn System, config: &ParsedCommandLine, config_file_name: &str) {
    let ts_config = convert_to_ts_config(config, config_file_name);
    let writer = sys.writer();
    // PORT: one report (`stdio::keep_writes`).
    let _report = crate::execute::tsc::stdio::keep_writes();
    let _ = json_marshal_indent_write(&mut *writer.borrow_mut(), &ts_config, "", "    ");
}

// PORT: added for `GoTsc`. Go passes no `WriteFile`, so `emitHost.WriteFile`
// (compiler/emitHost.go:120) writes through `program.Host().FS()`
// (cachedvfs over bundled over osvfs). The port's emit host refuses a
// write without a callback (program.rs emitHost `write_file`), and emit
// runs on the checker threads, so the callback must be `Send` and cannot
// hold the `Rc` file system. Both wrappers pass a write of a real path to
// osvfs (cachedvfs.go:144 WriteFile), so this writes with the osvfs of the
// calling thread, like `new_task_write_file` in build/build_task.rs without
// the build info tracking. There is no outDir or input guard: tsgo writes
// next to the sources or into outDir, as Go does.
pub(crate) fn os_write_file() -> WriteFile {
    Arc::new(
        |file_name: &str, text: &str, _data: &mut WriteFileData| -> Result<(), String> {
            osvfs_fs()
                .write_file(file_name, text)
                .map_err(|err| fs_error_text(&err))
        },
    )
}
