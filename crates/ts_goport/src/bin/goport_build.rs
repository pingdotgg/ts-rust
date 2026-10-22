//! `goport_build`: `tsc -b` with the Go port.
//!
//! - `goport_build -b [projects...] [options]` (also `--b`, `-build`,
//!   `--build`): Go `execute.CommandLine` with a build flag, which is
//!   `tscBuildCompilation` (execute/tsc.go:90). The exit code is the Go
//!   exit status. With `--watch` the run builds, then stays in the watch
//!   loop (Go `WatchManager.RunLoop`) until the process is killed: there is
//!   no signal handling (plan D-W3), so the loop never ends by itself.
//!   Output goes to stdout as it is written. Every project compiles in
//!   this process, as in Go. `--fast` and `GOPORT_FAST=1` work as with
//!   `tsgo -b` (`fast_mode::take_flag`).
//!
//! Only build mode is ported. Other command lines (Go `tscCompilation`)
//! exit with `EXIT_UNPORTED` (70).
//!
//! Unported Go code that a run reaches is listed on stderr as
//! `unported: <name> <count>`. Such a run and a panic exit with
//! `EXIT_UNPORTED` (70), a code tsgo never returns (Go uses 0 to 5), like
//! `goport` and `goport_emit`. A Go panic that the port keeps
//! (`core::go_panic`) ends the build as in Go: the output so far,
//! `panic: <message>` on stderr and exit 2.

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ts_goport::execute::execute_tsc::{is_build_flag, tsc_build_compilation};
use ts_goport::execute::tsc::compile::{
    EXIT_UNPORTED, ExitStatus, System, new_os_system, write_go_output,
};
use ts_goport::gostd::context;
use ts_goport::prelude::*;

const UNPORTED_PREFIX: &str = "unported Go code";

/// jemalloc is the global allocator (default feature `jemalloc`). A build
/// without the feature uses glibc malloc. See `goport.rs`
/// `set_malloc_tunables`.
#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Same as `goport.rs` `JEMALLOC_CONF`, so a build here uses the jemalloc
/// settings of `tsgo -b`.
/// `scripts/build-release.sh` reads it from this line for its BOLT runs.
#[cfg(all(target_os = "linux", target_env = "gnu", feature = "jemalloc"))]
const JEMALLOC_CONF: &str = "narenas:4,thp:always,metadata_thp:disabled,cache_oblivious:false";

fn main() {
    // First: it must run before the first heap allocation.
    ts_goport::thp_guard::thp_guard();
    // A build keeps the wide budget (`ThreadBudget::WIDE`): parse and bind
    // threads up to 8, and 16 arenas with glibc malloc.
    let budget = ThreadBudget::WIDE;
    set_malloc_tunables(&budget);
    budget.install();
    let mut args: Vec<String> = ts_goport::frontend::vfs::os_args();
    install_panic_hook();
    let work = std::thread::Builder::new()
        .name("goport_build".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || {
            // PORT: not in Go (fastmode1). As the tsc bin (`tsgo.rs`
            // `run_main`): `--fast` turns fast mode on, and the build flag
            // keeps Go's 4 checkers in fast mode (`fast_mode::checker_count`),
            // with `GOPORT_FAST=1` too. On the work thread, after the panic
            // hook, so a panic here exits 70, as one in the build does.
            ts_goport::fast_mode::take_flag(&mut args);
            run(&args)
        });
    let code = if let Ok(Ok(code)) = work.map(std::thread::JoinHandle::join) {
        code
    } else {
        eprintln!("goport_build: work thread failed");
        EXIT_UNPORTED
    };
    std::process::exit(code);
}

/// Copied from `goport.rs` `set_malloc_tunables`, which explains the
/// values. jemalloc gets `JEMALLOC_CONF`. With glibc malloc, a build runs
/// about 20 threads per program, so `arena_max` is 16 here
/// (`ThreadBudget::WIDE`). Under an address space or data limit, glibc
/// malloc gets `arena_max=1`, with jemalloc too. The variables stay set, so
/// the exec runs once. A jemalloc build with `JEMALLOC_CONF` built in execs
/// only under a limit.
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

fn run(args: &[String]) -> i32 {
    let sys = match new_os_system() {
        Ok(sys) => sys,
        Err(status) => return status.code(),
    };

    let sys: Rc<dyn System> = Rc::new(sys.with_writer(Rc::new(RefCell::new(StreamingStdout))));

    // Go: execute/tsc.go:53 CommandLine
    if !args.first().is_some_and(|arg| is_build_flag(arg)) {
        eprintln!("goport_build: only build mode (-b) is ported");
        return EXIT_UNPORTED;
    }

    let result = catch_unwind(AssertUnwindSafe(|| {
        // Go: cmd/tsc/main.go:29 `signal.NotifyContext(context.Background(), ...)`.
        // PORT: no signal handling (plan D-W3), so the context never ends.
        tsc_build_compilation(&context::background(), sys.clone(), args, None)
    }));
    let _ = sys.writer().borrow_mut().flush();
    let code = match result {
        Ok(result) => result.status.code(),
        Err(payload) if print_go_panic(payload.as_ref()) => EXIT_GO_PANIC,
        Err(payload) => {
            note_panic(payload.as_ref());
            ExitStatus::NotImplemented.code()
        }
    };
    finish(code)
}

/// Go `os.Stdout` is not buffered: each `fmt.Fprint` reaches the terminal
/// or pipe at once. Rust stdout keeps a partial line until the next
/// newline, so this writer flushes after each write. A `-b --watch` run
/// stays in the watch loop and prints as it goes. The bytes are the Go
/// bytes of the output, as `GoOutput` (the `new_os_system` writer) writes.
struct StreamingStdout;

impl Write for StreamingStdout {
    // One `write_all` of the whole text per `write!`, as Go's `fmt.Fprintf`
    // makes one write (std's default writes each format piece; the bytes are
    // the same).
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        self.write_all(std::fmt::format(args).as_bytes())
    }

    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut stdout = std::io::stdout().lock();
        write_go_output(&mut stdout, buf)?;
        stdout.flush()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

/// Prints the unported counts and returns `code`, or `EXIT_UNPORTED` when
/// the run reached unported code.
fn finish(code: i32) -> i32 {
    if report_unported() {
        return EXIT_UNPORTED;
    }
    code
}

/// Prints `unported: <name> <count>` lines to stderr. True when any.
fn report_unported() -> bool {
    let unported = unported_report();
    let mut stderr = std::io::stderr().lock();
    for (name, count) in &unported {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }
    !unported.is_empty()
}

/// Keeps unported panics quiet (they are counted) and prints other panics.
/// The run prints a Go panic.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if info.payload().is::<GoPanic>() {
            return;
        }
        let message = payload_message(info.payload());
        if message.starts_with(UNPORTED_PREFIX) {
            if std::env::var_os("GOPORT_TRACE").is_some() {
                eprintln!(
                    "trace: {message}\n{}",
                    ts_goport::backtrace::force_capture()
                );
            }
            return;
        }
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", go_site_file(l.file()), l.line()))
            .unwrap_or_default();
        eprintln!("goport_build: panic{location}: {message}");
        if std::env::var_os("GOPORT_TRACE").is_some() {
            eprintln!("{}", ts_goport::backtrace::force_capture());
        }
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

fn note_panic(payload: &(dyn Any + Send)) {
    if !payload_message(payload).starts_with(UNPORTED_PREFIX) {
        record_unported("panic");
    }
}
