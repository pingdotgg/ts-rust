//! `goport [tsc options]`: type checks a project with the Go port, like
//! `tsgo --noEmit`, and without `--pretty` like `tsgo --noEmit --pretty
//! false` (see `CheckBin`).
//!
//! Go: execute/tsc.go `CommandLine` and `tscCompilation`
//! (`execute::execute_tsc`): the Go command line parser, the Go
//! branches for errors, `--init`, `--version`, `--help`, `-p`, the config
//! search and `--showConfig`, then `performCompilation`, which reports
//! through execute/tsc/emit.go `EmitAndReportStatistics`. All output goes
//! to stdout, as in Go. The report is the shared `execute::tsc` one, as for
//! `goport_emit` and `goport_build`.
//!
//! goport never writes an output file: its compile step sets noEmit (see
//! `CheckBin`). `--init` writes a `tsconfig.json`, `--pprofDir` writes two
//! empty profile files (see `ts_goport::pprof`), and `--generateTrace` (or a
//! config `generateTrace`) writes the trace directory (see
//! `ts_goport::tracing`), as in Go.
//!
//! Each Go-ported stage runs under `catch_unwind`, so one unported path does
//! not hide the other diagnostics. Unported hits are printed to stderr as
//! `unported: <name> <count>` lines.
//!
//! Exit codes are the tsc ones (Go: execute/tsc/compile.go:30): 0, 1 when
//! there are diagnostics and the emit was skipped (also command line and
//! project errors), 2 when there are diagnostics and the emit was not
//! skipped (also config file read errors). Under noEmit, only a program
//! with no emittable file (no inputs, or only `.d.ts` files) has
//! diagnostics with exit 2. A run that hit unported code (or another
//! panic) exits `execute::tsc::EXIT_UNPORTED` (70), a code tsgo never
//! returns (Go uses 0 to 5). A Go panic that the port keeps
//! (`core::go_panic`) ends the run as in Go: the output so far, `panic:
//! <message>` on stderr and exit 2.

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use ts_goport::emitter::program_emit::{EmitOptions, EmitResult, WriteFile, emit_with};
use ts_goport::execute::execute_tsc::{TscCompilationHooks, command_line};
use ts_goport::execute::tsc::{
    EXIT_UNPORTED, ExitStatus, ProgramLike, System, Writer, new_os_system, write_go_output,
};
use ts_goport::frontend::tsoptions::ParsedCommandLine;
use ts_goport::gostd::context;
use ts_goport::prelude::*;

const UNPORTED_PREFIX: &str = "unported Go code";

/// jemalloc is the global allocator (default feature `jemalloc`). A build
/// without the feature uses glibc malloc. See `set_malloc_tunables`.
#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The jemalloc settings that `set_malloc_tunables` sets. `tsgo.rs` and
/// `goport_build.rs` have the same value, and `scripts/build-release.sh`
/// reads it from this line for its BOLT runs. `scripts/build-release.sh` builds
/// the same value into jemalloc (`JEMALLOC_SYS_WITH_MALLOC_CONF`); with
/// another value there, the bins exec themselves to set it.
#[cfg(all(target_os = "linux", target_env = "gnu", feature = "jemalloc"))]
const JEMALLOC_CONF: &str = "narenas:4,thp:always,metadata_thp:disabled,cache_oblivious:false";

fn main() {
    // First: it must run before the first heap allocation.
    ts_goport::thp_guard::thp_guard();
    // One budget sets the parse and bind threads and the malloc arenas.
    let budget = ThreadBudget::one_program(0);
    set_malloc_tunables(&budget);
    budget.install();
    // After the exec in `set_malloc_tunables` and before the work thread.
    #[cfg(all(feature = "jemalloc", target_os = "linux"))]
    ts_goport::jemalloc_layout::jemalloc_layout();
    // Go: `System.SinceStart` counts from the process start. The tunables
    // step above may exec the binary again, so the clock starts after it.
    let start = Instant::now();
    // #4734: Go `osutil.Args()[1:]`.
    let args: Vec<String> = ts_goport::frontend::osutil::args()[1..].to_vec();
    install_panic_hook();
    // Go: cmd/tsc/main.go runMain sends `--lsp` and `--api` to their own
    // entry points; everything else continues below. Nothing may write to
    // stdout before this point (stdout is the LSP channel). The panic hook
    // keeps recovered unported panics quiet. Exit: the Go status, or
    // EXIT_UNPORTED with the unported report on stderr when unported code
    // ran.
    if let Some(code) = ts_goport::cmd::tsgo::run_main(&args) {
        let unported = unported_report();
        if !unported.is_empty() {
            let mut stderr = std::io::stderr().lock();
            for (name, count) in &unported {
                let _ = writeln!(stderr, "unported: {name} {count}");
            }
            let _ = stderr.flush();
            std::process::exit(EXIT_UNPORTED);
        }
        std::process::exit(code);
    }
    // The loading thread keeps the frontend program and the checker pool, so
    // the whole run stays on it. The checkers run on their own threads.
    // The thread ends the process itself once `run` has written the output,
    // so the exit does not wait for the thread stacks (up to 1 GiB each,
    // `max_stack_size`) to unmap, the thread-local destructors or the join.
    let worker = std::thread::Builder::new()
        .name("goport".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || std::process::exit(run(&args, start)));
    // Reached only when the thread cannot start or `run` panics.
    let _ = worker.map(std::thread::JoinHandle::join);
    eprintln!("goport: worker thread failed");
    std::process::exit(EXIT_UNPORTED);
}

/// Sets the malloc tunables for this process.
///
/// jemalloc (default feature `jemalloc`): `_RJEM_MALLOC_CONF` is
/// `JEMALLOC_CONF`.
/// - `narenas:4` has the same speed as the default (4 arenas per CPU), with
///   less RSS (query: 140 MB against 160 MB).
/// - `thp:always` makes jemalloc ask for huge pages (`madvise`) for its
///   data. Without it, a kernel in THP `madvise` mode gives jemalloc none
///   (dbook: query 26k minor faults, effect 286k, against 1k and 6k for a
///   static glibc build with the tunables below). release2 measurement,
///   geometric mean against that static build: without it 12 to 14% slower
///   on dbook (THP `madvise`) and 3 to 7% slower on cup2 (THP `always`); with
///   it 0.5 to 6% faster on dbook and 2 to 4% slower on cup2. On cup2
///   jemalloc peak RSS is 2 to 11% above the static build. When little
///   memory is free in 2 MiB blocks, huge page faults wait in compaction;
///   `thp_guard` (called first in `main`) then turns THP off. rss1 (dbook,
///   check): with 4 KiB pages (`thp:default` there) query has 18 MiB less
///   peak RSS (130 to 112 MiB), but check is 4% slower on query, 17% on
///   hono and 11% to 13% on zod and effect.
/// - `metadata_thp:disabled` (the jemalloc default) and
///   `cache_oblivious:false` cut the RSS that huge pages add, at the same
///   speed (rss1, dbook, THP `madvise`, peak RSS of check: query 145 to
///   130 MiB, hono 325 to 300, zod 1007 to 940, effect 1106 to 1039; wall
///   time -0.6% to +0.7%). `metadata_thp:always` puts the jemalloc metadata
///   (7 MiB on query, 22 MiB on effect) in huge pages that it fills only in
///   part: 13 to 20 MiB more RSS. `cache_oblivious` (the jemalloc default)
///   gives each allocation of 16 KiB or more one more 4 KiB page for a
///   random start offset. Effect has about 12,000 of them live (47 MiB), and
///   huge pages make those pages resident. In THP `always` mode (cup2) the
///   kernel gives huge pages to the metadata anyway, and the change saves
///   4 to 50 MiB.
///
/// glibc malloc (a build without the `jemalloc` feature):
/// - `top_pad=67108864` (64 MiB) makes each thread heap read-write in full
///   when glibc makes it (malloc.c `new_heap` adds `top_pad`, up to the
///   64 MiB heap size). With THP `always` the kernel then maps the heaps with
///   2 MiB pages, as it does for the Go 64 MiB heap arenas. Without it, glibc
///   grows each heap by the bytes that it needs, and the first touch of each
///   4 KiB page faults (cup2, glibc 2.41: query 24k faults, effect 245k; with
///   the top pad 1k and 7k, and 27% to 38% less wall time).
/// - `hugetlb=1` grows the heaps in huge page steps. Before glibc 2.44 it
///   works only in THP `madvise` mode, so the top pad is necessary for THP
///   `always`.
/// - `arena_max` comes from `budget` (`ThreadBudget::one_program`), which
///   also caps the parse and bind threads. Here it has one arena for each
///   thread that is alive while the checkers run (main, the worker and the
///   4 checkers: 6), plus one for each parse worker that a large program
///   adds on these cores (3 at 8 or more cores: 9). With fewer arenas, two
///   checkers share one arena lock. With more threads than arenas, parse
///   threads share arena locks. Each arena in use raises peak RSS (query at
///   16 cores with 8 parse threads, with the top pad: 133 MB at 6 arenas,
///   137 MB at 7, 142 MB at 8; tsgo 119 MB), so a program that is not large
///   binds on fewer threads when there are spare arenas, and makes no more
///   arenas than with 6.
/// - Setting `top_pad` turns off the dynamic mmap threshold of glibc. A
///   fixed `mmap_threshold=33554432` had mixed results on query, so it is
///   not set.
///
/// Under an address space or data limit (`ulimit -v`, `ulimit -d`),
/// `GLIBC_TUNABLES` sets `arena_max=1`, with jemalloc too
/// (`ThreadBudget::glibc_tunables`).
///
/// The allocator reads these settings only at process start, so this runs
/// the same binary again once with them set. It leaves out each variable
/// that the caller already set (`_RJEM_MALLOC_CONF` or `GLIBC_TUNABLES`), so
/// the caller can override the values, and does nothing when none is left.
/// The run continues without the settings when the exec fails. A jemalloc
/// build with `JEMALLOC_CONF` built in (see there) has the jemalloc settings
/// from its start and execs only under a limit.
// PERF (perf11 qprof, dbook, perf10 release tsgo with `_RJEM_MALLOC_CONF`
// set by the caller): without the exec, `--version` takes 0.46 to 0.53 ms
// less and the time to `main` drops from 4.06 to 3.21 ms.
fn set_malloc_tunables(budget: &ThreadBudget) {
    // Unused off Linux.
    let _ = budget;
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::os::unix::process::CommandExt;
        let mut vars = Vec::new();
        // A build with `JEMALLOC_CONF` built into jemalloc
        // (`JEMALLOC_SYS_WITH_MALLOC_CONF`, set by `scripts/build-release.sh`)
        // needs no exec for it: jemalloc reads it at its start, and
        // `_RJEM_MALLOC_CONF` still overrides it.
        #[cfg(feature = "jemalloc")]
        if option_env!("JEMALLOC_SYS_WITH_MALLOC_CONF") != Some(JEMALLOC_CONF) {
            vars.push(("_RJEM_MALLOC_CONF", String::from(JEMALLOC_CONF)));
        }
        if let Some(value) = budget.glibc_tunables() {
            vars.push(("GLIBC_TUNABLES", value));
        }
        vars.retain(|(name, _)| std::env::var_os(name).is_none());
        if vars.is_empty() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut args = std::env::args_os();
        let mut command = std::process::Command::new(exe);
        if let Some(arg0) = args.next() {
            command.arg0(arg0);
        }
        // `exec` returns only when it fails.
        let _ = command.args(args).envs(vars).exec();
    }
}

/// Keeps panics from unported code quiet (they are counted instead) and
/// prints all other panics to stderr. `run` prints a Go panic.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if info.payload().is::<GoPanic>() {
            return;
        }
        let message = payload_message(info.payload());
        if message.starts_with(UNPORTED_PREFIX) {
            // GOPORT_TRACE=1 prints where each unported hit came from.
            if std::env::var_os("GOPORT_TRACE").is_some() {
                eprintln!(
                    "trace: {message}\n{}",
                    std::backtrace::Backtrace::force_capture()
                );
            }
            return;
        }
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        eprintln!("goport: panic{location}: {message}");
    }));
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        String::new()
    }
}

/// Counts a caught panic. Unported panics already counted themselves; any
/// other panic is counted as `panic`.
fn note_panic(payload: &(dyn Any + Send)) {
    if !payload_message(payload).starts_with(UNPORTED_PREFIX) {
        record_unported("panic");
    }
}

/// Runs `f`, or returns the default value when it panics. A Go panic goes
/// on.
fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            note_panic(resume_go_panic(payload).as_ref());
            T::default()
        }
    }
}

// Go: cmd/tsc/main.go runMain: `execute.CommandLine` with the process
// system and arguments, then `os.Exit` with the status.
// PORT: the report goes to a buffer that is written to stdout at the end,
// also when a step panics. A panic ends the run; the steps inside
// `GuardedProgram` are guarded on their own. A Go panic (`go_panic`) ends
// the run with the Go runtime exit code.
fn run(args: &[String], start: Instant) -> i32 {
    let sys = match new_os_system() {
        Ok(sys) => sys,
        Err(status) => return status.code(),
    };
    let buffer: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let writer: Writer = buffer.clone();
    let sys: Rc<dyn System> = Rc::new(sys.with_start(start).with_writer(writer));

    let result = catch_unwind(AssertUnwindSafe(|| {
        command_line(&context::background(), sys.clone(), args, &CheckBin)
    }));

    let mut stdout = std::io::stdout().lock();
    let _ = write_go_output(&mut stdout, &buffer.borrow());
    let _ = stdout.flush();

    let code = match result {
        Ok(result) => result.status.code(),
        Err(payload) if print_go_panic(payload.as_ref()) => EXIT_GO_PANIC,
        Err(payload) => {
            note_panic(payload.as_ref());
            ExitStatus::Success.code()
        }
    };

    let unported = unported_report();
    let mut stderr = std::io::stderr().lock();
    for (name, count) in &unported {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }

    if !unported.is_empty() {
        return EXIT_UNPORTED;
    }
    code
}

/// The goport part of the compile step (see `TscCompilationHooks`).
struct CheckBin;

impl TscCompilationHooks for CheckBin {
    // PORT: `-b` is unported here: goport never writes an output.
    // `goport_build` runs build mode.
    fn build_mode(&self) -> bool {
        false
    }

    // PORT: without `--pretty` on the command line, goport runs as with
    // `--pretty false`, the tsgo command line that the regression gate and
    // the measure scripts compare it with. The command line value also
    // overrides a config `pretty`, so the diagnostics are plain and there is
    // no error summary, on a TTY too. `--showConfig` keeps the unset value,
    // so it shows the options as Go does.
    fn command_line_parsed(&self, command_line: &mut ParsedCommandLine) {
        let options = command_line.compiler_options();
        if options.pretty.is_unknown() && !options.show_config.is_true() {
            let mut options = (**options).clone();
            options.pretty = Tristate::False;
            command_line.set_compiler_options(Rc::new(options));
        }
    }

    // PORT: goport runs on read-only project inputs and never writes an
    // output, so its compile step sets noEmit on the config, the options
    // the program gets with `--noEmit`. The init, version, help and
    // showConfig branches run before this, so `--showConfig` shows no
    // forced noEmit. Without `--noEmit`, tsgo emits the outputs and goport
    // does not (emit-modes check-without-noEmit-flag, a known divergence).
    fn prepare_compilation(
        &self,
        _sys: &dyn System,
        _command_line_options: &CompilerOptions,
        _config_file_name: &str,
        config: &mut ParsedCommandLine,
    ) -> Result<(), ExitStatus> {
        let mut options = (**config.compiler_options()).clone();
        options.no_emit = Tristate::True;
        config.set_compiler_options(Rc::new(options));
        Ok(())
    }

    fn program_like(&self) -> Option<&dyn ProgramLike> {
        Some(&GuardedProgram)
    }

    // PORT: tsc passes no `WriteFile`. Under noEmit nothing is written.
    fn write_file(&self) -> Option<WriteFile> {
        None
    }
}

/// The installed program as a Go `ProgramLike`, with each step guarded on
/// its own so one unported path does not hide the other diagnostics.
struct GuardedProgram;

impl ProgramLike for GuardedProgram {
    fn options(&self) -> &'static CompilerOptions {
        options()
    }
    fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        guard(|| get_bind_diagnostics(file))
    }
    fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        guard(get_global_diagnostics)
    }
    fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        collect_checker_diagnostics_with(file, check_file_guarded)
    }
    fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        guard(|| get_declaration_diagnostics(file))
    }
    /// Each file is guarded on its own.
    fn emit(&self, emit_options: EmitOptions) -> EmitResult {
        // Go: execute/tsc.go:317 an incremental program is an
        // `incremental.Program`. Its Emit under noEmit
        // (execute/incremental/program.go:243, #4407 `HandleNoEmitOptions`)
        // emits no file and returns the result of the build info write, or
        // an empty one: the emit is not skipped.
        // PORT: the build info is not written, so this adds no diagnostics
        // (see `perform_incremental_compilation` in
        // `execute::execute_tsc`).
        if options().is_incremental() {
            return EmitResult::default();
        }
        // PORT: tsc passes no `WriteFile` (`CheckBin::write_file`). Under
        // noEmit nothing is written.
        guard(|| emit_with(emit_options, |emit_file| guard(emit_file)))
    }
}

/// Semantic diagnostics for one file. A panic drops that file's results and
/// replaces the checker, whose caches may be half written. A Go panic goes
/// on.
fn check_file_guarded(checker: &mut Checker, file: Node) -> Vec<Diagnostic> {
    match catch_unwind(AssertUnwindSafe(|| {
        get_semantic_diagnostics_with_checker(
            &ts_goport::gostd::context::background(),
            checker,
            file,
        )
    })) {
        Ok(diagnostics) => diagnostics,
        Err(payload) => {
            note_panic(resume_go_panic(payload).as_ref());
            let index = (checker.id - 1) as usize;
            *checker = Checker::new(index);
            Vec::new()
        }
    }
}
