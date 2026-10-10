//! `tsgo`: the Go port of cmd/tsc.
//!
//! Go: cmd/tsc/main.go `runMain`. Every command line other than `--lsp`
//! and `--api` goes to `execute_tsc::command_line` (Go
//! `execute.CommandLine`), which parses all arguments.
//!
//! The exit code is the Go `ExitStatus` (0 to 5). Unported Go code that a
//! run reaches is listed on stderr as `unported: <name> <count>`. Such a
//! run and a panic exit with `EXIT_UNPORTED` (70), a code tsgo never
//! returns, like the other goport bins. A Go panic that the port keeps
//! (`core::go_panic`) ends the run as in Go: the output so far,
//! `panic: <message>` on stderr and exit 2.
//!
//! `--lsp` and `--api` run `cmd::tsgo::lsp::run_lsp` and
//! `cmd::tsgo::api::run_api` (Go cmd/tsc/lsp.go and api.go), the entry
//! points that `goport --lsp` and `goport --api` run too.
//!
//! Go `signal.NotifyContext(ctx, SIGINT, SIGTERM)` is
//! `cmd::tsgo::main::notify_context`. Only watch and build mode read the
//! context; a plain compile goes on after a signal, as in Go.
//! Not Go: when the run gets 4 KiB pages, a worker copy of the binary does
//! the work, so the exit does not wait for its memory to unmap (`launch`).
//!
//! As the Go runtime does at start: each thread unblocks the signals that
//! Go must get (`GO_UNBLOCKED`), the signals that Go drops get a handler
//! that does nothing, SIGQUIT prints its name and exits 2, SIGHUP ends the
//! process by SIGHUP, and the soft open-file limit goes up
//! (`go_runtime_start`; PORTING.md "Process start").
//! PORT: Go `core.ApplyDebugStackLimit` (`TS_GO_DEBUG_STACK_LIMIT`) is a
//! debug setting and is skipped. The work runs on a thread with the stack
//! size of `gostd::stack::max_stack_size` (1 GiB with no address space or
//! data limit), like the other goport bins.
//! PORT: Go `osSys` and `newSystem` (cmd/tsc/sys.go) are ported as
//! `OsSystem` and `new_os_system` in execute/tsc/compile.rs.
//! PORT: `enablevtprocessing_windows.go` (the Windows console) is not
//! ported.

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

use ts_goport::cmd::tsgo::api::run_api;
use ts_goport::cmd::tsgo::lsp::run_lsp;
use ts_goport::cmd::tsgo::main::notify_context;
use ts_goport::execute::execute_tsc::{GoTsc, command_line};
use ts_goport::execute::tsc::{EXIT_UNPORTED, System, new_os_system};
use ts_goport::gostd::context;
use ts_goport::prelude::*;

const UNPORTED_PREFIX: &str = "unported Go code";

/// jemalloc is the global allocator (default feature `jemalloc`). A build
/// without the feature uses glibc malloc. See `goport.rs`
/// `set_malloc_tunables`.
#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Same as `goport.rs` `JEMALLOC_CONF`. `scripts/build-release.sh` reads it
/// from this line for its BOLT runs.
#[cfg(all(target_os = "linux", target_env = "gnu", feature = "jemalloc"))]
const JEMALLOC_CONF: &str = "narenas:4,thp:always,metadata_thp:disabled,cache_oblivious:false";

/// The `arg0` of a worker (see `launch`) is this word, the launcher's
/// process id, and the number, device and inode of the launcher's end of
/// the pipe that takes the exit code: `tsgo-worker 4242 3 15 81234`. The
/// arguments, not the environment, name a worker, so a process that the
/// worker starts gets nothing of it.
#[cfg(target_os = "linux")]
const WORKER_ARG0: &str = "tsgo-worker";

/// A worker's link to its launcher (see `launch`).
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct Worker {
    /// The number of the launcher's end of the pipe that takes the exit code.
    fd: u32,
    /// The device and inode of that pipe.
    dev: u64,
    ino: u64,
    /// The launcher.
    launcher: rustix::process::Pid,
}

// Go: cmd/tsc/main.go:14 main
fn main() {
    // First: before any thread starts (`thp_guard` can start one), so each
    // thread gets Go's mask. It allocates nothing.
    #[cfg(target_os = "linux")]
    unblock_go_signals();
    // Next: it must run before the first heap allocation.
    let huge_pages = ts_goport::thp_guard::thp_guard();
    #[cfg(target_os = "linux")]
    if let Some(code) = launch(huge_pages) {
        std::process::exit(code);
    }
    // Unused off Linux (`launch` is Linux only).
    #[cfg(not(target_os = "linux"))]
    let _ = huge_pages;
    // A worker gets its parent-death signal here, or ends when its
    // launcher has ended (`worker`).
    #[cfg(target_os = "linux")]
    let _ = worker();
    // One budget sets the parse and bind threads and the malloc arenas.
    // tsgo has one more thread with an arena than goport: the
    // `notify_context` signal thread.
    let budget = ThreadBudget::one_program(1);
    set_malloc_tunables(&budget);
    budget.install();
    // After the exec in `set_malloc_tunables`: an exec resets the handlers,
    // and a raised limit would read as the original one there.
    let signals = go_runtime_start();
    // After the exec in `set_malloc_tunables` and before the work thread.
    // After `go_runtime_start` (it starts no thread), so Go's signal
    // handlers are set before the layout's 0.3 ms.
    #[cfg(all(feature = "jemalloc", target_os = "linux"))]
    ts_goport::jemalloc_layout::jemalloc_layout();
    // Go: `System.SinceStart` counts from the process start. The tunables
    // step above may exec the binary again, so the clock starts after it.
    let start = Instant::now();
    install_panic_hook();
    // The thread ends the process itself once `run_main` has written the
    // output, so the exit does not wait for the thread stacks (up to 1 GiB
    // each, `max_stack_size`) to unmap, the thread-local destructors or the
    // join. Go runs `runMain` on the main goroutine. A thread that cannot
    // start ends the process as the Go runtime does (`GoThread`).
    let work = ts_goport::core::GoThread::new()
        .name("tsgo".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(|| exit(run_main(start))));
            // Reached only when `run_main` panics. The write drops its
            // error, as the panic hook's do: `eprintln!` panics when stderr
            // is a pipe with no reader. A Go panic does not raise SIGPIPE
            // (the runtime drops its write errors), so this keeps exit code
            // 70 with no reader (`tests/tsgo_panic_hook.rs`). The work
            // thread ends the process here: this thread waits for signals.
            let _ = writeln!(std::io::stderr(), "tsgo: work thread failed");
            std::process::exit(EXIT_UNPORTED)
        });
    // PERF (startexit1): this thread waits for the signals that end the
    // run (`go_runtime_start`), where a thread of its own did: each thread
    // is a stack, a malloc arena and a teardown at exit in every run.
    #[cfg(target_os = "linux")]
    if let Some(signals) = signals {
        wait_go_signals(signals);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = signals;
    // The work thread ends the process (above), so this join does not end.
    let _ = work.join();
    std::process::exit(EXIT_UNPORTED);
}

/// Copied from `goport.rs` `set_malloc_tunables`, which explains the
/// values. jemalloc gets `JEMALLOC_CONF`. With glibc malloc, `arena_max`
/// comes from `budget` (`ThreadBudget::one_program`): with the signal thread
/// it is 7 here, and 10 at 8 or more cores (3 spare arenas for the parse
/// workers that a large program adds). At 6, two checkers share one arena
/// lock (zod: 3.9k voluntary context switches, 0.5k at 7). Under an address
/// space or data limit, glibc malloc gets `arena_max=1`, with jemalloc too.
/// The variables stay set, so the exec runs once. A jemalloc build with
/// `JEMALLOC_CONF` built in execs only under a limit.
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
        // `exec` returns only when it fails (a binary that is gone, for
        // example). It has given SIGPIPE its default action for the new
        // image (std `Command`), so a write to a closed pipe or socket
        // would end this process, as the second SIGINT or SIGTERM did
        // (`notify_context` writes to its closed self-pipe). std ignores
        // SIGPIPE at start, and Go gets EPIPE there. A handler that does
        // nothing gives this process EPIPE again.
        // PORT: std and rustix have no safe `SIG_IGN`.
        let _ = command.args(args).envs(vars).exec();
        let ignore = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _ = signal_hook::flag::register(signal_hook::consts::SIGPIPE, ignore);
    }
}

/// Runs the work in a worker copy of this binary and returns its exit code
/// (perf16). The worker sends the code over a pipe once its output is
/// written (`exit`), so this process exits before the worker unmaps its
/// memory. With 4 KiB pages that unmap takes about 50 ms for effect (1.3 GB);
/// with huge pages it takes about 4 ms, less than a second process costs
/// (about 1 ms on query check). So by default the worker runs only when
/// `thp_guard` says the run gets 4 KiB pages (`huge_pages` false).
/// `GOPORT_LAUNCH=0` never starts a worker and `GOPORT_LAUNCH=1` always
/// does. None when this process runs the work: it is a worker, no worker is
/// wanted, `--lsp`, `--api` or watch mode (`long_running`: they end on
/// their own), or the worker cannot start. The launcher sends SIGINT,
/// SIGTERM, SIGHUP and the signals that Go throws on to the worker
/// (`forward_signals`), and drops the signals that Go drops, SIGCHLD too
/// (`drop_go_signals`: an ignored SIGCHLD would make `wait` fail). When a
/// signal kills the worker, the launcher ends by the same signal
/// (`end_by_signal`; a pid 1 exits 128 + N, as Go does there), so the
/// caller sees what a run without a worker would give.
#[cfg(target_os = "linux")]
fn launch(huge_pages: bool) -> Option<i32> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let wanted = match std::env::var_os("GOPORT_LAUNCH") {
        Some(v) if v == "0" => false,
        Some(v) if v == "1" => true,
        _ => !huge_pages,
    };
    if !wanted || worker().is_some() || ts_goport::thp_guard::long_running() {
        return None;
    }
    let mut args = std::env::args_os();
    args.next()?;
    let args: Vec<_> = args.collect();
    let exe = std::env::current_exe().ok()?;
    // Both ends keep their close-on-exec flag, so the worker and the
    // processes it starts get neither. The worker opens `write` again by
    // the number of `read` (`exit`). THP off (`prctl`) stays off in the
    // worker.
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).ok()?;
    let launcher = rustix::process::getpid().as_raw_pid();
    let stat = rustix::fs::fstat(&read).ok()?;
    // Before the worker starts, so a failed start leaves no worker behind.
    let forward = forward_signals();
    drop_go_signals();
    let started = std::process::Command::new(exe)
        .arg0(format!(
            "{WORKER_ARG0} {launcher} {} {} {}",
            read.as_raw_fd(),
            stat.st_dev,
            stat.st_ino
        ))
        .args(args)
        .spawn();
    let Ok(mut worker) = started else {
        // No worker: this process runs the work. `forward` drops here, so
        // its thread ends.
        restore_default_actions();
        return None;
    };
    if let Some(forward) = forward {
        let _ = forward.send(rustix::process::Pid::from_child(&worker));
    }
    // `write` stays open until the worker ends, so the read below ends when
    // the code comes or when the worker has ended without it. The thread
    // does not reap the worker (`NOWAIT`): `wait` below does. When the
    // thread cannot start, `write` closes now and the read ends at once:
    // the launcher then takes the code from the worker's exit, which waits
    // for the worker's memory to unmap. No code or signal is lost, so this
    // port-only thread falls back instead of ending the run (`GoThread`).
    let pid = rustix::process::Pid::from_child(&worker);
    let _ = std::thread::Builder::new()
        .name("worker-exit".to_string())
        .spawn(move || {
            use rustix::process::{WaitId, WaitIdOptions, waitid};
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
            while let Err(rustix::io::Errno::INTR) = waitid(WaitId::Pid(pid), options) {}
            drop(write);
        });
    let mut code = [0; 4];
    if std::fs::File::from(read).read_exact(&mut code).is_ok() {
        return Some(i32::from_le_bytes(code));
    }
    // The worker ended without sending a code.
    let status = worker.wait();
    if let Some(signal) = status.as_ref().ok().and_then(ExitStatusExt::signal) {
        // End by the same signal. Where that returns (a pid 1, or a
        // signal that the launcher catches), exit 128 + N below.
        end_by_signal(signal);
    }
    Some(match status {
        Ok(status) => status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
        Err(_) => EXIT_UNPORTED,
    })
}

/// This process as a worker (see `launch`): its `arg0` is `WORKER_ARG0`
/// with a launcher and its pipe, and its parent is that launcher. The
/// first call decides, at the start of `main`. A worker gets a
/// parent-death SIGKILL, so it ends when its launcher ends, as a killed Go
/// tsgo stops at once.
/// A process whose `arg0` names a launcher that is not its parent runs as
/// a plain tsgo, unless that launcher has ended (`has_ended`). Then this
/// process is the launcher's worker and the launcher died before the
/// parent check (a SIGKILL soon after the start), so it kills itself, as
/// the parent-death signal would have. std and rustix have no safe way to
/// set that signal between fork and exec, so the launcher can also die
/// after the check and before the signal is set: the second parent check
/// finds that.
#[cfg(target_os = "linux")]
fn worker() -> Option<Worker> {
    use rustix::process::{Signal, getpid, getppid, kill_process, set_parent_process_death_signal};
    static WORKER: std::sync::OnceLock<Option<Worker>> = std::sync::OnceLock::new();
    *WORKER.get_or_init(|| {
        let arg0 = std::env::args_os().next()?;
        let rest = arg0
            .to_str()?
            .strip_prefix(WORKER_ARG0)?
            .strip_prefix(' ')?;
        let mut fields = rest.split(' ');
        let launcher = rustix::process::Pid::from_raw(fields.next()?.parse().ok()?)?;
        let fd = fields.next()?.parse().ok()?;
        let dev = fields.next()?.parse().ok()?;
        let ino = fields.next()?.parse().ok()?;
        if fields.next().is_some() {
            return None;
        }
        if getppid() == Some(launcher) {
            let _ = set_parent_process_death_signal(Some(Signal::KILL));
            if getppid() == Some(launcher) {
                return Some(Worker {
                    fd,
                    dev,
                    ino,
                    launcher,
                });
            }
        } else if !has_ended(launcher) {
            return None;
        }
        let _ = kill_process(getpid(), Signal::KILL);
        std::process::exit(EXIT_UNPORTED)
    })
}

/// Whether the process `pid` has ended: it is gone, or it is a zombie (it
/// has ended and its parent has not reaped it yet). The state is the field
/// after the command name in /proc/<pid>/stat. Without that file, or
/// without this process's own /proc (`own_proc`: there the file is of
/// another process), `kill` with no signal tells only whether the process
/// is gone: a zombie launcher then gives a plain tsgo.
#[cfg(target_os = "linux")]
fn has_ended(pid: rustix::process::Pid) -> bool {
    let path = format!("/proc/{}/stat", pid.as_raw_pid());
    match own_proc().then(|| std::fs::read(path)) {
        // `<pid> (<name>) <state> ...`: the name can hold ") ".
        Some(Ok(stat)) => {
            let name_end = stat.iter().rposition(|&b| b == b')');
            let state = name_end.and_then(|end| stat.get(end + 2));
            matches!(state, Some(b'Z' | b'X'))
        }
        _ => rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH),
    }
}

/// Whether /proc is the /proc of this process's PID namespace, so that
/// /proc/<pid> is the process `pid`. In a PID namespace that has the /proc
/// of another one (`bwrap --unshare-pid` without `--proc`, `unshare -pf`
/// without `--mount-proc`), /proc/<pid> is another process or none: for
/// the pid of a worker it can be a kernel thread whose parent has the pid
/// of the launcher. The `NSpid` line of /proc/self/status has this process's
/// pid in each PID namespace from the one of /proc down to its own, so it
/// is the one pid that `getpid` gives only in its own /proc. A kernel
/// without that line (before 4.1) has the `Pid` line, the pid in the
/// namespace of /proc. False without /proc (`own_status`).
#[cfg(target_os = "linux")]
fn own_proc() -> bool {
    own_status().own_proc
}

/// Whether SIGHUP was ignored (`SIG_IGN`) when this process started, from
/// the `SigIgn` line of /proc/self/status (a hex mask with bit N-1 for
/// signal N). Go then keeps it ignored (`sigInstallGoHandler`), and so does
/// tsgo: it sets no handler for it (`go_runtime_start`, `forward_signals`).
/// /proc/self is this process in any /proc that shows it.
/// PORT: std, rustix and signal-hook have no safe way to read an action.
/// Without /proc, SIGHUP counts as not ignored.
#[cfg(target_os = "linux")]
fn hup_ignored() -> bool {
    own_status().hup_ignored
}

/// What /proc/self/status says (`own_proc`, `hup_ignored`). The first call
/// reads the file, before this process sets a handler for SIGHUP; the
/// later calls use its result. All false without the file.
#[cfg(target_os = "linux")]
fn own_status() -> OwnStatus {
    static STATUS: std::sync::OnceLock<OwnStatus> = std::sync::OnceLock::new();
    *STATUS.get_or_init(|| {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return OwnStatus::default();
        };
        let pid = rustix::process::getpid().as_raw_pid().to_string();
        let field = |name: &str| status.lines().find_map(|line| line.strip_prefix(name));
        let own_proc = match field("NSpid:") {
            Some(pids) => pids.split_whitespace().eq([pid.as_str()]),
            None => field("Pid:").map(str::trim) == Some(pid.as_str()),
        };
        let ignored = field("SigIgn:").and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok());
        let hup = 1u64 << (signal_hook::consts::SIGHUP - 1);
        OwnStatus {
            own_proc,
            hup_ignored: ignored.is_some_and(|ignored| ignored & hup != 0),
        }
    })
}

/// See `own_status`.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Default)]
struct OwnStatus {
    own_proc: bool,
    hup_ignored: bool,
}

/// Sends each SIGINT, SIGTERM and SIGHUP and each signal that Go throws
/// (`GO_THROWN`) that the launcher gets on to the worker, on a thread. So a
/// signal reaches the work as in a run without a worker: `notify_context`
/// catches SIGINT and SIGTERM there, and a plain compile goes on, as in
/// Go; SIGQUIT prints its name there once, also when it went to the whole
/// process group; SIGHUP ends the worker (`die_from_signal`) and then the
/// launcher by the same signal. Without this, the signal would end the
/// launcher and then the parent-death signal would kill the worker, and a
/// launcher that is pid 1 would not get SIGHUP at all (`end_by_signal`).
/// The thread takes SIGHUP only after the worker has started and only when
/// it was not ignored at start (`hup_ignored`): an exec keeps an ignored
/// signal but gives a caught one its default action, so the worker gets
/// the caller's action for SIGHUP, as `go_runtime_start` needs.
/// `launch` calls it before it starts the worker and sends the worker's pid
/// to the returned sender; a signal that comes first waits for it. When no
/// worker starts, `launch` drops the sender and the thread ends.
/// Each signal also waits until the worker catches it (`caught`), for at
/// most `HOLD_LIMIT` after the worker starts, and only with this process's
/// own /proc (`own_proc`). Otherwise it goes on at once. Each signal waits
/// on its own, so one that waits does not hold a later one: a SIGQUIT that
/// comes after a SIGINT that came before the worker's `notify_context` goes
/// on once the worker catches SIGQUIT (`go_runtime_start`), not once it
/// catches SIGINT.
/// The thread starts as a Go runtime thread does (`GoThread`): when the OS
/// refuses it, the launcher ends with Go's text and exit 2, before there is
/// a worker. Go has no launcher, so its process gets every signal; a
/// launcher that went on without this thread would drop them (dropping
/// `signals` removes their actions, not their handlers).
#[cfg(target_os = "linux")]
fn forward_signals() -> Option<std::sync::mpsc::Sender<rustix::process::Pid>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let thrown = GO_THROWN.iter().map(|(signal, _)| signal.as_raw());
    let mut signals =
        signal_hook::iterator::Signals::new([SIGINT, SIGTERM].into_iter().chain(thrown)).ok()?;
    let (send, receive) = std::sync::mpsc::channel();
    ts_goport::core::GoThread::new()
        .name("forward-signals".to_string())
        .spawn(move || {
            let Ok(pid) = receive.recv() else {
                return;
            };
            // The worker has started.
            let limit = own_proc().then(|| Instant::now() + HOLD_LIMIT);
            if !hup_ignored() {
                let _ = signals.add_signal(signal_hook::consts::SIGHUP);
            }
            // The signals that wait, in the order they came, each once.
            let mut held: Vec<rustix::process::Signal> = Vec::new();
            // The files are read at once, then after pauses of 1, 2, 4
            // and 8 ms, then every 8 ms.
            let mut pause = std::time::Duration::from_millis(1);
            loop {
                let came = if held.is_empty() {
                    signals.wait()
                } else {
                    signals.pending()
                };
                for signal in came.filter_map(rustix::process::Signal::from_named_raw) {
                    if !held.contains(&signal) {
                        held.push(signal);
                        pause = std::time::Duration::from_millis(1);
                    }
                }
                held.retain(|&signal| {
                    let wait = limit.is_some_and(|until| caught(pid, signal, until) == Some(false));
                    if !wait {
                        let _ = rustix::process::kill_process(pid, signal);
                    }
                    wait
                });
                if !held.is_empty() {
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(std::time::Duration::from_millis(8));
                }
            }
        });
    Some(send)
}

/// Whether the worker `pid` catches `signal` now (`forward_signals`), so a
/// forwarded signal finds the handlers that the worker sets at its start
/// (`go_runtime_start`, `notify_context`) and does not end it by the
/// default action. A signal that came before them (soon after the start,
/// or while the start of `forward_signals` was tried again) waits: Go
/// sets its handlers before `main`, and `runMain` calls `NotifyContext`
/// before any work (cmd/tsc/main.go:29).
/// The kernel lists the caught signals in /proc/<pid>/status (`SigCgt`, a
/// hex mask with bit N-1 for signal N). The caller calls it only with this
/// process's own /proc (`own_proc`). None when the signal must not wait
/// any longer: the worker has ended, that file does not show a live child
/// of this process, or it is `until`.
/// PORT: signal-hook sets the handler (the bit) a moment before it
/// publishes the action that the handler runs (signal-hook-registry 1.4.8
/// `register_unchecked_impl`), so a signal sent on in between does nothing.
/// Followups9 round b also waited for the thread that the worker starts
/// after it has registered the signal. That thread gets its name only when
/// it first runs, so the wait held signals longer, and under CPU load a
/// short run with a launcher lost more of them (PORTING.md "Process start").
#[cfg(target_os = "linux")]
fn caught(
    pid: rustix::process::Pid,
    signal: rustix::process::Signal,
    until: Instant,
) -> Option<bool> {
    if Instant::now() >= until {
        return None;
    }
    let status = std::fs::read_to_string(format!("/proc/{}/status", pid.as_raw_pid())).ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
    };
    let launcher = rustix::process::getpid().as_raw_pid().to_string();
    let child = field("PPid:") == Some(launcher.as_str());
    let live = field("State:").is_some_and(|state| !state.starts_with(['Z', 'X']));
    let mask = field("SigCgt:").and_then(|mask| u64::from_str_radix(mask, 16).ok());
    let mask = mask.filter(|_| child && live)?;
    let bit = 1u64 << (signal.as_raw() - 1).unsigned_abs();
    Some(mask & bit != 0)
}

/// How long after the worker starts a forwarded signal can wait for the
/// worker's handlers (`caught`). The worker sets them a few milliseconds
/// after its start. After this time a signal goes on at once, as without
/// the wait, so no wait lasts the whole run (a worker that is stopped at
/// its start, for example).
#[cfg(target_os = "linux")]
const HOLD_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Gives SIGINT, SIGTERM and the signals that Go throws (`GO_THROWN`) their
/// default actions back when `launch` starts no worker after
/// `forward_signals` took them: dropping its `Signals` removes their
/// actions, not their handlers, so they would do nothing until the run
/// sets its own handlers. A run that never was a launcher has the default
/// actions there. Each one does its default action while its flag is set
/// (`register_conditional_default`); `go_runtime_start` and `run_main`
/// clear the flags once their handlers are set (`end_default_actions`).
/// PORT: std and rustix have no safe `SIG_DFL`; signal-hook sets it and
/// raises the signal again. SIGSTKFLT is not in signal-hook's table, so
/// it does nothing there until `go_runtime_start`.
#[cfg(target_os = "linux")]
fn restore_default_actions() {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag::register_conditional_default;
    // In a pid 1 the default actions do nothing (`end_by_signal`), as the
    // handlers without actions do. signal-hook's would abort there.
    if rustix::process::getpid().is_init() {
        return;
    }
    let set = || std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let notify = NOTIFY_DEFAULT.get_or_init(set);
    let thrown = THROWN_DEFAULT.get_or_init(set);
    for signal in [SIGINT, SIGTERM] {
        let _ = register_conditional_default(signal, notify.clone());
    }
    for (signal, _) in GO_THROWN {
        let _ = register_conditional_default(signal.as_raw(), thrown.clone());
    }
}

/// The flags of the default actions of SIGINT and SIGTERM
/// (`NOTIFY_DEFAULT`) and of the signals that Go throws (`THROWN_DEFAULT`),
/// set only by `restore_default_actions`.
#[cfg(target_os = "linux")]
static NOTIFY_DEFAULT: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();
#[cfg(target_os = "linux")]
static THROWN_DEFAULT: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

/// Ends the default actions of `restore_default_actions` that `flag` sets,
/// once the run has set its own handlers for those signals.
#[cfg(target_os = "linux")]
fn end_default_actions(flag: &std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>>) {
    if let Some(flag) = flag.get() {
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Signals that the Go runtime catches and drops when no `signal.Notify`
/// asks for them: `_SigNotify` alone or with `_SigUnblock` or `_SigIgn`,
/// without `_SigDefault`, in `runtime/sigtab_linux_generic.go` (go1.27.1).
/// The default action of SIGCHLD, SIGURG and SIGWINCH (`_SigIgn`) does
/// nothing too, but Go sets its handler also when they were ignored at
/// start. The real-time signals 35 to 64 (`GO_DROPPED_RT`) are such
/// signals too; Go leaves 32 to 34 to the C library. SIGPIPE is apart: Go
/// has its own rule for stdout and stderr. Go sets no handler for SIGCONT
/// and the stop signals (`_SigDefault`).
#[cfg(target_os = "linux")]
const GO_DROPPED: [rustix::process::Signal; 12] = {
    use rustix::process::Signal;
    [
        Signal::USR1,
        Signal::USR2,
        Signal::ALARM,
        Signal::CHILD,
        Signal::URG,
        Signal::XCPU,
        Signal::XFSZ,
        Signal::VTALARM,
        Signal::PROF,
        Signal::WINCH,
        Signal::IO,
        Signal::POWER,
    ]
};

/// The real-time signals that Go drops (see `GO_DROPPED`).
#[cfg(target_os = "linux")]
const GO_DROPPED_RT: std::ops::RangeInclusive<i32> = 35..=64;

/// Signals for which the Go runtime prints the name from its table and
/// exits 2 (`_SigThrow` in `runtime/sigtab_linux_generic.go`), also when the
/// signal was ignored at start: Go keeps an inherited `SIG_IGN` only for
/// SIGHUP and SIGINT (`runtime/signal_unix.go` `sigInstallGoHandler`).
/// PORT: Go then prints the goroutines; the port prints only the name.
/// PORT: SIGABRT and SIGTRAP keep their default actions: Rust's abort (a
/// stack overflow too) raises SIGABRT, and debuggers use SIGTRAP.
#[cfg(target_os = "linux")]
const GO_THROWN: [(rustix::process::Signal, &str); 3] = {
    use rustix::process::Signal;
    [
        (Signal::QUIT, "SIGQUIT: quit"),
        (Signal::STKFLT, "SIGSTKFLT: stack fault"),
        (Signal::SYS, "SIGSYS: bad system call"),
    ]
};

/// The signals that the Go runtime unblocks on each of its threads, so a
/// caller cannot block them (`runtime/signal_unix.go` `minitSignalMask` and
/// `blockableSig`, go1.27.1): `_SigUnblock`, `_SigKill` or `_SigThrow` in
/// `runtime/sigtab_linux_generic.go`, and SIGURG, its preemption signal
/// (`unblock_go_signals`).
/// PORT: Go also unblocks the signals 32 to 34 (`_SigUnblock`). nix's
/// `SigSet` has no real-time signals, so tsgo keeps them as the caller set
/// them (glibc does not block 32 and 33, its own signals).
/// PORT: with `GODEBUG=asyncpreemptoff=1` Go lets a caller block SIGURG.
/// tsgo does not read `GODEBUG`; both drop SIGURG (`GO_DROPPED`).
#[cfg(target_os = "linux")]
const GO_UNBLOCKED: [nix::sys::signal::Signal; 15] = {
    use nix::sys::signal::Signal;
    [
        Signal::SIGHUP,
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGILL,
        Signal::SIGTRAP,
        Signal::SIGABRT,
        Signal::SIGBUS,
        Signal::SIGFPE,
        Signal::SIGSEGV,
        Signal::SIGTERM,
        Signal::SIGSTKFLT,
        Signal::SIGCHLD,
        Signal::SIGURG,
        Signal::SIGPROF,
        Signal::SIGSYS,
    ]
};

// Go: runtime/signal_unix.go minitSignalMask (go1.27.1)
/// Unblocks the signals that Go unblocks on each of its threads
/// (`GO_UNBLOCKED`). `main` calls it first, before any thread starts, so
/// each thread gets the new mask: in tsgo, in a launcher and in its worker.
/// A process that tsgo starts gets the mask of the thread that starts it
/// (std `Command` keeps it), so the caller's mask without these signals,
/// as from Go: Go saves the mask of the thread before the fork and the
/// child sets it (runtime/proc.go `syscall_runtime_BeforeFork`,
/// `syscall_runtime_AfterForkInChild`).
#[cfg(target_os = "linux")]
fn unblock_go_signals() {
    use nix::sys::signal::SigSet;
    let _ = GO_UNBLOCKED
        .into_iter()
        .collect::<SigSet>()
        .thread_unblock();
}

/// Gives each signal that Go drops (`GO_DROPPED`, `GO_DROPPED_RT`) a handler
/// that does nothing. Not `SIG_IGN`: an exec resets a caught signal to its
/// default action, so a process that tsgo starts gets the default actions,
/// as from Go, also when tsgo got them ignored. An ignored SIGCHLD also
/// changes tsgo itself: the kernel then reaps each child when it ends, so
/// a wait for it fails (ECHILD). `launch` calls it before it starts the
/// worker, and `go_runtime_start` before the run starts a process.
#[cfg(target_os = "linux")]
fn drop_go_signals() {
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signals = GO_DROPPED.iter().map(|signal| signal.as_raw());
    for signal in signals.chain(GO_DROPPED_RT) {
        let _ = signal_hook::flag::register(signal, dropped.clone());
    }
}

/// The start of a process that runs the work, as the Go runtime and the Go
/// `syscall` package start: the signals that Go drops get a handler that
/// does nothing (`drop_go_signals`), a signal that Go throws (`GO_THROWN`)
/// ends the process (`throw`), SIGHUP ends it by SIGHUP, unless it was
/// ignored at start (`die_from_signal`, `hup_ignored`; as a pid 1 it exits
/// 129, where the default action would do nothing), the soft open-file
/// limit goes up to one below the hard limit
/// (`gostd::rlimit::raise_open_file_limit`), and fd 1 is checked for
/// `O_NONBLOCK`, as Go `os.NewFile` does at start (`stdio::init`).
/// Returns the thrown signals and SIGHUP, for which the main thread waits
/// on a pipe (`wait_go_signals`). Go throws in the signal handler, with no
/// thread. None when they cannot be registered.
/// PORT: other systems than Linux keep the default actions (Go's tables
/// differ there).
fn go_runtime_start() -> Option<GoSignals> {
    ts_goport::execute::tsc::stdio::init();
    ts_goport::gostd::rlimit::raise_open_file_limit();
    #[cfg(target_os = "linux")]
    {
        drop_go_signals();
        let thrown = GO_THROWN.iter().map(|(signal, _)| signal.as_raw());
        let hup = (!hup_ignored()).then_some(signal_hook::consts::SIGHUP);
        let signals = signal_hook::iterator::Signals::new(thrown.chain(hup)).ok()?;
        end_default_actions(&THROWN_DEFAULT);
        Some(signals)
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// The signals that `go_runtime_start` registers (Linux), or none.
#[cfg(target_os = "linux")]
type GoSignals = signal_hook::iterator::Signals;
#[cfg(not(target_os = "linux"))]
type GoSignals = std::convert::Infallible;

/// Waits for a signal of `signals` (`go_runtime_start`) and ends the
/// process by it: a signal that Go throws (`throw`) or SIGHUP
/// (`die_from_signal`). Returns only when the wait fails.
#[cfg(target_os = "linux")]
fn wait_go_signals(mut signals: GoSignals) {
    if let Some(signal) = signals.forever().next() {
        match GO_THROWN.iter().find(|(s, _)| s.as_raw() == signal) {
            Some((_, name)) => throw(name),
            None => die_from_signal(signal),
        }
    }
}

/// Ends the process after a signal that Go throws (`GO_THROWN`), as the Go
/// runtime does: it writes `name` to fd 2 with a raw write, sends the code
/// to the launcher in a worker (`send_code`) and exits 2 (`EXIT_GO_PANIC`,
/// the exit code of a Go fatal error). An error of the write (a closed or
/// broken stderr) is ignored, as in Go.
/// First it writes the stdout bytes that a report keeps on a regular file
/// (`stdio::flush_cli_stdout_at_exit`), so the file has the pieces written
/// so far, as Go's has (`os.Stdout` has no buffer). It skips them when
/// another thread holds their lock. It waits for no lock, so it cannot
/// wait for the work thread: that thread can hold the stdout or stderr
/// lock in a write that blocks on a full pipe. So it ends with `_exit`:
/// `std::process::exit` flushes the std stdout buffer when no other thread
/// holds its lock, and waits when another thread is in its cleanup.
#[cfg(target_os = "linux")]
fn throw(name: &str) -> ! {
    ts_goport::execute::tsc::stdio::flush_cli_stdout_at_exit();
    let line = format!("{name}\n");
    let _ = rustix::io::write(rustix::stdio::stderr(), line.as_bytes());
    if let Some(worker) = worker() {
        send_code(worker, EXIT_GO_PANIC);
    }
    signal_hook::low_level::exit(EXIT_GO_PANIC)
}

/// Ends the process after SIGHUP as the Go runtime ends it after a signal
/// with `_SigKill` in its table when no `signal.Notify` asks for it
/// (`dieFromSignal`; tsgo asks only for SIGINT and SIGTERM). First it
/// writes the kept stdout bytes, as `throw` does, so a file has the pieces
/// written so far, as Go's has. Then it ends by the signal
/// (`end_by_signal`), or, as a pid 1, exits 128 + N, as Go does there. It
/// sends no code: a launcher takes the signal from the worker's exit and
/// ends by it too. It ends with `_exit`, as `throw` does.
#[cfg(target_os = "linux")]
fn die_from_signal(signal: i32) -> ! {
    ts_goport::execute::tsc::stdio::flush_cli_stdout_at_exit();
    end_by_signal(signal);
    signal_hook::low_level::exit(128 + signal)
}

/// Ends this process by `signal` with the default action of the signal, as
/// Go `dieFromSignal` does, so the caller sees a process that the signal
/// ended. It sets the default action, unblocks the signal and raises it.
/// It returns in the pid 1 of a PID namespace (`docker run` without
/// `--init`, `unshare -pf`, `bwrap --as-pid-1`): the kernel drops a signal
/// with the default action that such a process sends itself. Go then exits
/// 128 + N, as a shell reports a process that a signal ended, and so does
/// each caller. It also returns for a signal that has no entry in
/// signal-hook's table (SIGPWR, SIGSTKFLT, the real-time signals) or that
/// the table takes as ignored (SIGIO).
/// PORT: std and rustix have no safe `SIG_DFL`. signal-hook sets it, and
/// it calls `abort` when the raise returns; in a pid 1, glibc's `abort`
/// then ends the process by SIGSEGV (rc 139, maybe a core file). So a pid
/// 1 does not raise the signal. Go raises it, and the kernel drops it.
#[cfg(target_os = "linux")]
fn end_by_signal(signal: i32) {
    if rustix::process::getpid().is_init() {
        return;
    }
    let _ = signal_hook::low_level::emulate_default_handler(signal);
}

/// Ends the process with `code` once the work has written its output. A
/// worker (see `launch`) flushes stdout and sends the code (`send_code`).
fn exit(code: i32) -> ! {
    #[cfg(target_os = "linux")]
    if let Some(worker) = worker() {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        send_code(worker, code);
    }
    std::process::exit(code)
}

/// Sends `code` to the launcher of `worker` (see `launch`). First it points
/// stdout and stderr at /dev/null, so a reader of the launcher's output gets
/// its end of file when the launcher ends, while this process unmaps its
/// memory. It takes no std lock (`throw`).
#[cfg(target_os = "linux")]
fn send_code(worker: Worker, code: i32) {
    use rustix::fs::{FileType, Mode, OFlags, fstat, open};
    use std::os::fd::AsRawFd;
    if let Ok(null) = std::fs::File::options().write(true).open("/dev/null") {
        let _ = rustix::stdio::dup2_stdout(&null);
        let _ = rustix::stdio::dup2_stderr(&null);
    }
    // The launcher's end of the pipe, by its number, through /proc. Only
    // with this process's own /proc (`own_proc`): in another one,
    // /proc/<launcher> is another process or none. The code goes only to a
    // FIFO with the device and inode that the launcher passed. The first
    // open (`O_PATH`) only names the file and opens no FIFO or device, so
    // it cannot wait or change another file. The open for writing opens
    // the checked file again through /proc/self/fd (this process's own
    // files in any /proc that shows it), not the path, and does not wait
    // for a reader (`O_NONBLOCK`). Each new file has a close-on-exec flag.
    // Without its own /proc, or when an open fails, the launcher takes the
    // code from the worker's exit.
    if !own_proc() {
        return;
    }
    let path = format!("/proc/{}/fd/{}", worker.launcher.as_raw_pid(), worker.fd);
    let Ok(file) = open(path, OFlags::PATH | OFlags::CLOEXEC, Mode::empty()) else {
        return;
    };
    let checked = fstat(&file).is_ok_and(|stat| {
        FileType::from_raw_mode(stat.st_mode) == FileType::Fifo
            && stat.st_dev == worker.dev
            && stat.st_ino == worker.ino
    });
    let again = format!("/proc/self/fd/{}", file.as_raw_fd());
    let flags = OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
    if checked && let Ok(pipe) = open(again, flags, Mode::empty()) {
        let _ = std::fs::File::from(pipe).write_all(&code.to_le_bytes());
    }
}

// Go: cmd/tsc/main.go:18 runMain
// PORT: the arguments are the port form of the Go `osutil.Args()` bytes (see
// `scanner_util::GO_STRING_MARKER`). The system writer writes the Go bytes
// of the output (`GoOutput`).
fn run_main(start: Instant) -> i32 {
    let args: Vec<String> = ts_goport::frontend::osutil::args()[1..].to_vec();

    if let Some(first) = args.first() {
        match first.as_str() {
            "--lsp" => return finish(catch_unwind(AssertUnwindSafe(|| run_lsp(&args[1..])))),
            "--api" => return finish(catch_unwind(AssertUnwindSafe(|| run_api(&args[1..])))),
            _ => {}
        }
    }

    // Go: ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
    let (ctx, stop) = notify_context(&context::background());
    #[cfg(target_os = "linux")]
    end_default_actions(&NOTIFY_DEFAULT);
    // PORT: Go `newSystem()` calls `os.Exit` on this error, so `stop` does
    // not run there either.
    let sys = match new_os_system() {
        Ok(sys) => sys,
        Err(status) => return status.code(),
    };
    let sys: Rc<dyn System> = Rc::new(sys.with_start(start));
    let result = catch_unwind(AssertUnwindSafe(|| {
        command_line(&ctx, sys.clone(), &args, &GoTsc).status.code()
    }));
    // Go: defer stop()
    stop();
    // `--showConfig` output has no trailing newline, and
    // `std::process::exit` runs no destructors, so flush here.
    let _ = sys.writer().borrow_mut().flush();
    finish(result)
}

/// Prints a Go panic and the unported counts and returns the exit code:
/// `EXIT_UNPORTED` when the run panicked or reached unported code,
/// `EXIT_GO_PANIC` after a Go panic, else `result`.
fn finish(result: std::thread::Result<i32>) -> i32 {
    let code = match result {
        Ok(code) => code,
        Err(payload) if print_go_panic(payload.as_ref()) => EXIT_GO_PANIC,
        Err(payload) => {
            note_panic(payload.as_ref());
            EXIT_UNPORTED
        }
    };
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
/// The run prints a Go panic. A panic that a Go `recover()` catches
/// (`core::go_recover`: an API request answers it) prints nothing, as in Go.
/// The writes drop their errors: `eprintln!` panics when stderr is a pipe
/// with no reader, and a panic inside the hook aborts the process, also for
/// a panic that a caller catches (`tests/tsgo_panic_hook.rs`).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if info.payload().is::<GoPanic>() {
            return;
        }
        let message = payload_message(info.payload());
        if message.starts_with(UNPORTED_PREFIX) || ts_goport::core::in_go_recover() {
            if std::env::var_os("GOPORT_TRACE").is_some() {
                let _ = writeln!(
                    std::io::stderr(),
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
        // The output so far comes first, also in one file with stderr.
        ts_goport::execute::tsc::stdio::flush_cli_stdout_at_exit();
        let _ = writeln!(std::io::stderr(), "tsgo: panic{location}: {message}");
        if std::env::var_os("GOPORT_TRACE").is_some() {
            let _ = writeln!(
                std::io::stderr(),
                "{}",
                std::backtrace::Backtrace::force_capture()
            );
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

/// Counts a caught panic. Unported panics already counted themselves; any
/// other panic is counted as `panic`.
fn note_panic(payload: &(dyn Any + Send)) {
    if !payload_message(payload).starts_with(UNPORTED_PREFIX) {
        record_unported("panic");
    }
}
