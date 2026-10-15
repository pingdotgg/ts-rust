//! Go: execute/tsc/compile.go (the `tsc` system interface, exit status and
//! compile result types), plus the `osSys` system of cmd/tsc/sys.go.
//!
//! PORT: Go `io.Writer` is `Writer` (a shared `std::io::Write`). The
//! frontend file system is `Rc`, so the system and the writers are `Rc`
//! too and stay on one thread. A build compiles every project on the
//! orchestrator thread, so no writer is shared across threads. The one
//! exception is Go `ErrorWriter()` (`ErrorWriter`): the content mapper
//! logger writes to it from the thread that reads a mapper's stderr.

use crate::prelude::*;

use std::time::{Duration, SystemTime};

use crate::contentmapper::{
    self, Host as ContentMapperHost, HostOptions as ContentMapperHostOptions,
    Logger as ContentMapperLogger, ProcessExitState, Spawner as ContentMapperSpawner,
};
use crate::emitter::program_emit::EmitResult;
use crate::execute::tsc::stdio;
use crate::frontend::tsoptions::ParseConfigHost;
use crate::frontend::vfs::Fs;
use crate::fswatch::syscall::io_error_text;
use crate::gostd::{Context, GoError};
// PORT: testing (`CommandLineTesting`)
use crate::frontend::compiler::TraceFn;
use crate::frontend::tspath::Path;
use crate::locale::Locale;
use std::sync::{Arc, Mutex, PoisonError};

/// Go `io.Writer`. A caller that wants the text back (Go `bytes.Buffer`)
/// keeps its own `Rc<RefCell<Vec<u8>>>` and passes a clone as a `Writer`.
pub type Writer = Rc<RefCell<dyn std::io::Write>>;

/// Writes `text` to `w`. Go ignores the `fmt.Fprint` error, so this does too.
// PORT: `text` is the port form of a Go string (see
// `scanner_util::GO_STRING_MARKER`), and a writer keeps that form. The
// process output writes the Go bytes (`GoOutput`, `write_go_output`).
pub fn write_str(w: &Writer, text: &str) {
    let _ = w.borrow_mut().write_all(text.as_bytes());
}

/// Writes the Go bytes of the port form output `bytes` to `out` (see
/// `scanner_util::GO_STRING_MARKER`). Bytes that are not UTF-8 are written
/// unchanged.
pub fn write_go_output(out: &mut dyn std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
    match std::str::from_utf8(bytes) {
        Ok(text) => out.write_all(&go_string_bytes(text)),
        Err(_) => out.write_all(bytes),
    }
}

/// Go `os.Stdout` as the system writer: it writes the Go bytes of each port
/// form write (see `write_go_output`) through `stdio::CliStdout`, which
/// waits on a non-blocking pipe and ends the process by SIGPIPE when the
/// reader is gone, as Go does. `write_str` writes whole strings, so a write
/// never splits a unit.
pub struct GoOutput;

impl std::io::Write for GoOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        write_go_output(&mut stdio::CliStdout, buf)?;
        Ok(buf.len())
    }

    // One write also for an empty `buf`, as Go `fmt.Fprint`.
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        write_go_output(&mut stdio::CliStdout, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        stdio::CliStdout.flush()
    }
}

/// Go `os.Stderr` as the system error writer: it writes the Go bytes of
/// each port form write through `stdio::Stderr`, as `GoOutput` does for
/// stdout.
pub struct GoErrorOutput;

impl std::io::Write for GoErrorOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        write_go_output(&mut stdio::Stderr, buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        stdio::Stderr.flush()
    }
}

/// Go `io.Writer` of `System.ErrorWriter()`. The content mapper logger
/// writes to it from a mapper's stderr thread, so it is `Send` and has its
/// own lock.
pub type ErrorWriter = Arc<Mutex<dyn std::io::Write + Send>>;

// Go: execute/tsc/compile.go:22 System
// PORT: Go `FS()` returns the shared `vfs.FS`. Go `time.Time` is
// `SystemTime` and `time.Duration` is `Duration`. Go `Spawn` returns an
// `io.ReadWriteCloser`; here it is the content mapper's
// `ProcessExitState` (an `ipc::ReadWriteCloser`, see there), and Go
// `stderr` `io.Discard` is `None`, as in `contentmapper::Spawner`. Go
// `GetEnvironmentVariable` returns `(string, bool)` as `os.LookupEnv` does
// (ts#63941): the bool says the variable is set, also when it is empty.
pub trait System {
    fn writer(&self) -> Writer;
    fn error_writer(&self) -> ErrorWriter;
    fn fs(&self) -> Rc<dyn Fs>;
    fn default_library_path(&self) -> String;
    fn get_current_directory(&self) -> String;
    fn write_output_is_tty(&self) -> bool;
    fn get_width_of_terminal(&self) -> i32;
    fn get_environment_variable(&self, name: &str) -> (String, bool);
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError>;

    fn now(&self) -> SystemTime;
    fn since_start(&self) -> Duration;

    /// PORT: not in Go. True when a write through the osvfs of any thread
    /// (`osvfs_fs()`) reaches `fs()`, so the emit of `tsc -b` can write from
    /// the checker threads (build/build_task.rs `new_task_write_file`): the
    /// OS system, and a test system, whose tests install their file system
    /// as the osvfs. False when only this thread can reach `fs()`.
    fn emit_writes_through_osvfs(&self) -> bool {
        true
    }

    /// PORT: not in Go. How each emit write reaches the client when it is
    /// a client call (the API build with `--callbacks writeFile`), else
    /// None. `tsc -b` writes after a failed write by it
    /// (`execute::incremental::emit_files::flush_client_writes`).
    fn client_writes(&self) -> Option<crate::execute::incremental::emit_files::ClientWrites> {
        None
    }
}

// Go: execute/tsc/compile.go:37 newContentMapperLogger
// PORT: Go `mu` guards the writes; the `ErrorWriter` lock does that here.
// Go `fmt.Fprintln(writer, message)` is one write of the line.
pub(crate) fn new_content_mapper_logger(sys: &dyn System) -> Option<ContentMapperLogger> {
    let (value, _) = sys.get_environment_variable("TS_CONTENT_MAPPER_DEBUG");
    if value.is_empty() {
        return None;
    }
    let writer = sys.error_writer();
    Some(Arc::new(move |message: &str| {
        let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writer.write_all(format!("{message}\n").as_bytes());
    }))
}

/// Go `tsc.System` as the `contentmapper.Spawner` of the host (Go passes
/// `sys`, whose `Spawn` method makes it a `Spawner`).
struct SystemSpawner(Rc<dyn System>);

impl ContentMapperSpawner for SystemSpawner {
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        self.0.spawn(command, dir, stderr)
    }
}

// Go: execute/tsc/compile.go:89 NewContentMapperHost
// NewContentMapperHost creates a content mapper host when content mappers are enabled via the
// --runExternalCode flag, spawning mapper processes through the system's Spawn. It returns
// nil otherwise, in which case no content-mapped files can be loaded. The caller owns the host and must
// Close it when the compilation session ends.
// PORT: Go nil is `None`. `sys` is the `Rc` so that the host can keep it
// as its spawner.
pub fn new_content_mapper_host(
    ctx: &Context,
    sys: &Rc<dyn System>,
    options: &CompilerOptions,
) -> Option<Rc<dyn ContentMapperHost>> {
    if !options.run_external_code.is_true() {
        return None;
    }
    let (diagnostic_locale, _) = crate::locale::parse(&options.locale);
    Some(contentmapper::new_host_with_options(
        ctx,
        Rc::new(SystemSpawner(sys.clone())),
        diagnostic_locale,
        ContentMapperHostOptions {
            logger: new_content_mapper_logger(&**sys),
        },
    ))
}

/// Go `tsc.System` as a `tsoptions.ParseConfigHost` (Go passes `sys`
/// where a `ParseConfigHost` is needed; it has `FS()` and
/// `GetCurrentDirectory()`).
pub struct SystemParseConfigHost<'a>(pub &'a dyn System);

impl ParseConfigHost for SystemParseConfigHost<'_> {
    fn fs(&self) -> Rc<dyn Fs> {
        self.0.fs()
    }

    fn get_current_directory(&self) -> String {
        self.0.get_current_directory()
    }
}

// Go: execute/tsc/compile.go:50 ExitStatus
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(i32)]
pub enum ExitStatus {
    #[default]
    Success = 0,
    DiagnosticsPresentOutputsSkipped = 1,
    DiagnosticsPresentOutputsGenerated = 2,
    InvalidProjectOutputsSkipped = 3,
    ProjectReferenceCycleOutputsSkipped = 4,
    NotImplemented = 5,
}

impl ExitStatus {
    /// The process exit code (Go `os.Exit(int(status))`).
    pub fn code(self) -> i32 {
        self as i32
    }

    /// The status for an exit code, or `None` for an unknown code. The Go
    /// baseline runner uses it to read a child's status back.
    pub fn from_code(code: i32) -> Option<ExitStatus> {
        Some(match code {
            0 => ExitStatus::Success,
            1 => ExitStatus::DiagnosticsPresentOutputsSkipped,
            2 => ExitStatus::DiagnosticsPresentOutputsGenerated,
            3 => ExitStatus::InvalidProjectOutputsSkipped,
            4 => ExitStatus::ProjectReferenceCycleOutputsSkipped,
            5 => ExitStatus::NotImplemented,
            _ => return None,
        })
    }
}

/// The exit code of a goport bin that hit unported code or another panic,
/// or whose work thread failed. It is outside the Go `ExitStatus` range (0
/// to 5), so it never looks like a tsgo status. 70 is `EX_SOFTWARE`
/// (internal software error).
pub const EXIT_UNPORTED: i32 = 70;

/// PORT: a panic on a Go goroutine without `recover` ends the Go process.
/// A Go panic that the port keeps (`core::go_panic`) ends it as the Go
/// runtime does: `panic: <message>` on stderr and `EXIT_GO_PANIC` (2).
/// Any other panic is a port gap and ends it the way `bin/goport.rs` ends a
/// failed run: the unported report on stderr and `EXIT_UNPORTED` (70). The
/// panic hook already printed such a panic.
pub fn go_crash(payload: Box<dyn std::any::Any + Send>) -> ! {
    use std::io::Write as _;
    if crate::core::print_go_panic(payload.as_ref()) {
        std::process::exit(crate::core::EXIT_GO_PANIC);
    }
    let mut stderr = std::io::stderr().lock();
    for (name, count) in crate::core::unported_report() {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }
    let _ = stderr.flush();
    std::process::exit(EXIT_UNPORTED);
}

// Go: execute/tsc/compile.go:61 Watcher
pub trait Watcher {
    fn do_cycle(&mut self);

    /// PORT: not in Go. Go tests assert the concrete type
    /// (`result.Watcher.(*execute.Watcher)`); a Rust test downcasts this.
    fn as_any(&self) -> &dyn std::any::Any;
}

// Go: execute/tsc/compile.go:65 CommandLineResult
#[derive(Default)]
pub struct CommandLineResult {
    pub status: ExitStatus,
    pub watcher: Option<Box<dyn Watcher>>,
}

// Go: execute/tsc/compile.go:70 CommandLineTesting
// PORT: testing. The Go test harness hook (tsctests/sys.go `TestSys`).
// Every real run passes `None` (Go nil), so it takes the Go nil paths. Go
// `io.Writer` is `Writer`. Go `*collections.SyncMap[tspath.Path,
// time.Time]` is the build host `m_times`, a `Mutex` (`None` is the Go zero
// time).
pub trait CommandLineTesting {
    // Ensure that all emitted files are timestamped in order to ensure they are deterministic for test baseline
    fn on_emitted_files(
        &self,
        result: &EmitResult,
        m_times_cache: Option<&Mutex<FxHashMap<Path, Option<SystemTime>>>>,
    );
    fn on_list_files_start(&self, w: &Writer);
    fn on_list_files_end(&self, w: &Writer);
    fn on_statistics_start(&self, w: &Writer);
    fn on_statistics_end(&self, w: &Writer);
    fn on_build_status_report_start(&self, w: &Writer);
    fn on_build_status_report_end(&self, w: &Writer);
    fn on_watch_status_report_start(&self);
    fn on_watch_status_report_end(&self);
    fn get_trace(&self, w: Writer, locale: Locale) -> TraceFn;
    fn on_program(&self, program: &crate::execute::incremental::program::Program);
}

// Go: execute/tsc/compile.go:99 CompileTimes
// PORT: Go keeps `bindTime`, `checkTime`, `totalTime` and `emitTime`
// unexported; the bins set them, so all fields are public.
#[derive(Clone, Debug, Default)]
pub struct CompileTimes {
    pub config_time: Duration,
    pub parse_time: Duration,
    pub content_mapper_times: contentmapper::Timings,
    pub bind_time: Duration,
    pub check_time: Duration,
    pub total_time: Duration,
    pub emit_time: Duration,
    pub build_info_read_time: Duration,
    pub changes_compute_time: Duration,
}

// Go: execute/tsc/compile.go:110 CompileAndEmitResult
// PORT: Go `*compiler.EmitResult` is never nil after `EmitFilesAndReportErrors`,
// so it is a value here (`Default` for the Go zero result). Go `times` is a
// pointer shared with the caller's `CompileTimes`.
#[derive(Clone, Default)]
pub struct CompileAndEmitResult {
    pub diagnostics: Vec<Diagnostic>,
    pub emit_result: EmitResult,
    pub status: ExitStatus,
    pub(crate) times: Rc<RefCell<CompileTimes>>,
}

// Go: cmd/tsc/sys.go:19 osSys
// PORT: added here so the library and the bins share one system.
pub struct OsSystem {
    writer: Writer,
    fs: Rc<dyn Fs>,
    default_library_path: String,
    cwd: String,
    start: std::time::Instant,
}

// Go: cmd/tsc/sys.go:124 newSystem
// PORT: Go exits with `ExitStatusInvalidProject_OutputsSkipped` when the
// current directory cannot be read; this returns that status instead. The
// error goes to Go `os.Stderr` (`stdio::Stderr`): with a stderr pipe that
// has no reader, the write ends the process by SIGPIPE, as Go's
// `epipecheck` does (`tests/tsgo_panic_hook.rs`).
pub fn new_os_system() -> Result<OsSystem, ExitStatus> {
    let cwd = match crate::frontend::vfs::os_current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            use std::io::Write as _;
            // Go: fmt.Fprintf ignores the write error.
            let _ = writeln!(
                stdio::Stderr,
                "Error getting current directory: {}",
                crate::frontend::vfs::getwd_error_text(&err)
            );
            return Err(ExitStatus::InvalidProjectOutputsSkipped);
        }
    };
    Ok(OsSystem {
        cwd: crate::frontend::tspath::normalize_path(&cwd),
        fs: crate::frontend::bundled::wrap_fs(crate::frontend::vfs::osvfs_fs()),
        default_library_path: crate::frontend::bundled::lib_path(),
        writer: Rc::new(RefCell::new(GoOutput)),
        start: std::time::Instant::now(),
    })
}

impl OsSystem {
    /// The system with `writer` in place of stdout. A bin that keeps its
    /// output in a buffer, or streams it, uses this.
    pub fn with_writer(mut self, writer: Writer) -> OsSystem {
        self.writer = writer;
        self
    }

    /// The system with `start` as its start time. Go `SinceStart` counts
    /// from the process start, which a bin can read before it makes the
    /// system.
    pub fn with_start(mut self, start: std::time::Instant) -> OsSystem {
        self.start = start;
        self
    }
}

impl System for OsSystem {
    // Go: cmd/tsc/sys.go:47 Writer
    fn writer(&self) -> Writer {
        self.writer.clone()
    }
    // Go: cmd/tsc/sys.go:51 ErrorWriter (tsgo#4712)
    fn error_writer(&self) -> ErrorWriter {
        Arc::new(Mutex::new(GoErrorOutput))
    }
    // Go: cmd/tsc/sys.go:35 FS
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }
    // Go: cmd/tsc/sys.go:39 DefaultLibraryPath
    fn default_library_path(&self) -> String {
        self.default_library_path.clone()
    }
    // Go: cmd/tsc/sys.go:43 GetCurrentDirectory
    fn get_current_directory(&self) -> String {
        self.cwd.clone()
    }
    // Go: cmd/tsc/sys.go:55 WriteOutputIsTTY
    fn write_output_is_tty(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdout().is_terminal()
    }
    // Go: cmd/tsc/sys.go:59 GetWidthOfTerminal
    // Go `term.GetSize(int(os.Stdout.Fd()))` is the TIOCGWINSZ ioctl on
    // stdout, and gives width 0 on error (golang.org/x/term v0.44.0
    // term_unix.go:59 getSize).
    // PORT: `rustix::termios::tcgetwinsize` makes the same ioctl, so this
    // crate needs no unsafe code. Off unix (Go: the Windows console call in
    // term_windows.go) the port has no size call and gives 0, as Go does on
    // an error.
    #[cfg(unix)]
    fn get_width_of_terminal(&self) -> i32 {
        match rustix::termios::tcgetwinsize(std::io::stdout()) {
            Ok(ws) => i32::from(ws.ws_col),
            Err(_) => 0,
        }
    }
    #[cfg(not(unix))]
    fn get_width_of_terminal(&self) -> i32 {
        0
    }
    // Go: cmd/tsc/sys.go:64 GetEnvironmentVariable (ts#63941)
    // PORT: Go `os.LookupEnv`. A set variable whose value is not UTF-8 is
    // set here too; its value is the lossy UTF-8 text (Go keeps the bytes).
    fn get_environment_variable(&self, name: &str) -> (String, bool) {
        match std::env::var_os(name) {
            Some(value) => (value.to_string_lossy().into_owned(), true),
            None => (String::new(), false),
        }
    }
    // Go: cmd/tsc/sys.go:68 Spawn (tsgo#4712)
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        spawn_process(command, dir, stderr)
    }
    // Go: cmd/tsc/sys.go:31 Now
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    // Go: cmd/tsc/sys.go:27 SinceStart
    fn since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Go `cmd.WaitDelay = time.Second` in `spawnProcess`: how long `Close`
/// waits for the stderr copy after the process exits.
const CHILD_PROCESS_WAIT_DELAY: Duration = Duration::from_secs(1);

// Go: cmd/tsc/sys.go:74 spawnProcess (tsgo#4712)
// spawnProcess launches a process and adapts its stdio to an io.ReadWriteCloser (Read is its stdout,
// Write is its stdin).
// PORT: Go `exec.Command` looks a name without a slash up in PATH
// (`look_path`), and `Start` checks `Dir` first. Their Go error texts reach
// the content mapper diagnostics, so this makes the same texts. The child's
// stdin, stdout and stderr are Unix socket pairs, not pipes: Go's `Close`
// closes the parent ends while the connection may still read and the
// stderr copy may still run, and a socket `shutdown` ends those blocked
// calls in safe Rust. Go `stderr` `io.Discard` is `None` (the null device
// here). Go `cmd.Env` nil and the argv[0] of the name are kept. With a
// `dir`, Go's `Cmd.environ` adds `PWD=` its absolute path, and so does this.
#[cfg(unix)]
pub fn spawn_process(
    command: &[String],
    dir: &str,
    stderr: Option<Box<dyn std::io::Write + Send>>,
) -> Result<Arc<dyn ProcessExitState>, GoError> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let name = command.first().map_or("", String::as_str);
    // Go `Start`: "exec: no command" for an empty path.
    if name.is_empty() {
        return Err(crate::gostd::errors::new("exec: no command"));
    }
    // Go `exec.Command` calls `LookPath` only when the name has no slash.
    let program = if name.contains('/') {
        name.to_string()
    } else {
        look_path(name)?
    };
    // Go os/exec/exec.go `(*Cmd).environ`, called by `Start` before the
    // process starts: when `Dir` is set, the child's `PWD` is
    // `filepath.Abs(Dir)`, and an `Abs` error ends `Start`.
    let pwd = if dir.is_empty() {
        None
    } else {
        Some(go_abs(dir)?)
    };
    // Go os/exec_posix.go startProcess: the `Dir` check with op "chdir".
    if !dir.is_empty()
        && let Err(err) = std::fs::metadata(dir)
    {
        return Err(crate::gostd::errors::new(format!(
            "chdir {dir}: {}",
            io_error_text(&err)
        )));
    }
    let io_error = |err: std::io::Error| crate::gostd::errors::new(io_error_text(&err));
    let (stdin, child_stdin) = UnixStream::pair().map_err(io_error)?;
    let (stdout, child_stdout) = UnixStream::pair().map_err(io_error)?;
    let mut cmd = Command::new(&program);
    cmd.arg0(name).args(&command[1..]);
    if let Some(pwd) = pwd {
        cmd.current_dir(dir);
        cmd.env("PWD", pwd);
    }
    cmd.stdin(Stdio::from(OwnedFd::from(child_stdin)));
    cmd.stdout(Stdio::from(OwnedFd::from(child_stdout)));
    let mut stderr_copy = None;
    match stderr {
        Some(writer) => {
            let (ours, child_stderr) = UnixStream::pair().map_err(io_error)?;
            let reader = ours.try_clone().map_err(io_error)?;
            cmd.stderr(Stdio::from(OwnedFd::from(child_stderr)));
            stderr_copy = Some((ours, reader, writer));
        }
        None => {
            cmd.stderr(Stdio::null());
        }
    }
    let spawned = crate::gostd::rlimit::spawn(&mut cmd);
    // The command holds the child's ends; drop them so that a read sees the
    // end of the stream when the child exits.
    drop(cmd);
    let child = spawned.map_err(|err| {
        crate::gostd::errors::new(format!("fork/exec {program}: {}", io_error_text(&err)))
    })?;
    // Go copies a non-file `cmd.Stderr` on a goroutine.
    let stderr = stderr_copy.map(|(stream, reader, mut writer)| {
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut &reader, &mut writer);
            let _ = done_tx.send(());
        });
        ChildStderr { stream, done }
    });
    Ok(Arc::new(ChildProcess {
        child: Mutex::new(Some(child)),
        stdin,
        stdout,
        stderr: Mutex::new(stderr),
        exit_code: Mutex::new(None),
    }))
}

// Go: cmd/tsc/sys.go:95 childProcess (tsgo#4712)
// childProcess adapts a spawned process's stdout (read) and stdin (write) into one io.ReadWriteCloser.
// Close kills and reaps the process.
// PORT: Go `cmd.ProcessState` after `Wait` is `exit_code` (the Go
// `ExitCode()` value). `child` is `None` after `Close`.
#[cfg(unix)]
struct ChildProcess {
    child: Mutex<Option<std::process::Child>>,
    stdin: std::os::unix::net::UnixStream,
    stdout: std::os::unix::net::UnixStream,
    stderr: Mutex<Option<ChildStderr>>,
    exit_code: Mutex<Option<i32>>,
}

/// The parent end of the child's stderr and the end signal of its copy.
#[cfg(unix)]
struct ChildStderr {
    stream: std::os::unix::net::UnixStream,
    done: std::sync::mpsc::Receiver<()>,
}

#[cfg(unix)]
impl crate::ipc::ReadWriteCloser for ChildProcess {
    // Go: cmd/tsc/sys.go:101 childProcess.Read
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut &self.stdout, buf)
    }

    // Go: cmd/tsc/sys.go:102 childProcess.Write
    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut &self.stdin, buf)
    }

    fn flush(&self) -> std::io::Result<()> {
        std::io::Write::flush(&mut &self.stdin)
    }

    // Go: cmd/tsc/sys.go:111 childProcess.Close
    // PORT: Go `Wait` closes the stdout pipe after the process exits, and
    // waits up to `WaitDelay` for the stderr copy; then it closes that pipe
    // and returns `ErrWaitDelay`, which Close ignores. An `ExitError` is
    // `Ok` here too. A second Close is Go's second `Wait`.
    fn close(&self) -> Result<(), GoError> {
        use std::net::Shutdown;
        let _ = self.stdin.shutdown(Shutdown::Both);
        let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Err(crate::gostd::errors::new("exec: Wait was already called"));
        };
        let _ = child.kill();
        let waited = child.wait();
        let _ = self.stdout.shutdown(Shutdown::Both);
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(stderr) = stderr {
            let _ = stderr.done.recv_timeout(CHILD_PROCESS_WAIT_DELAY);
            let _ = stderr.stream.shutdown(Shutdown::Both);
        }
        match waited {
            Ok(status) => {
                // Go `ProcessState.ExitCode()`: -1 for a signal.
                *self
                    .exit_code
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(status.code().unwrap_or(-1));
                Ok(())
            }
            Err(err) => Err(crate::gostd::errors::new(format!(
                "wait: {}",
                io_error_text(&err)
            ))),
        }
    }
}

#[cfg(any(unix, windows))]
impl ProcessExitState for ChildProcess {
    // Go: cmd/tsc/sys.go:104 childProcess.ExitCode
    fn exit_code(&self) -> (i32, bool) {
        match *self
            .exit_code
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            Some(code) => (code, true),
            None => (0, false),
        }
    }
}

// Go: cmd/tsc/sys.go:74 spawnProcess, built for GOOS=wasip1
// PORT: wasm cannot start processes. Go calls `cmd.StdinPipe()` before
// `cmd.Start()`, and wasip1 `os.Pipe` (os/pipe_wasm.go) returns
// `NewSyscallError("pipe", syscall.ENOSYS)`, whose text comes from
// syscall/tables_wasip1.go. So Go gives this text for every command, before
// a LookPath or fork/exec error can happen.
#[cfg(target_family = "wasm")]
pub fn spawn_process(
    _command: &[String],
    _dir: &str,
    _stderr: Option<Box<dyn std::io::Write + Send>>,
) -> Result<Arc<dyn ProcessExitState>, GoError> {
    Err(crate::gostd::errors::new("pipe: Not implemented on wasip1"))
}

/// Go `exec.LookPath(file)` (os/exec/lp_unix.go, go1.27.1) for a name without
/// a slash, with the Go `exec.Error` texts.
// PORT: Go `execerrdot` is its default: a match in a relative PATH entry is
// the `ErrDot` error.
#[cfg(unix)]
fn look_path(file: &str) -> Result<String, GoError> {
    let exec_error = |text: &str| {
        crate::gostd::errors::new(format!(
            "exec: {}: {text}",
            crate::gostd::strconv::quote(file)
        ))
    };
    let path = std::env::var("PATH").unwrap_or_default();
    // Go `filepath.SplitList("")` is empty.
    if !path.is_empty() {
        for dir in path.split(':') {
            // Unix shell semantics: path element "" means "."
            let dir = if dir.is_empty() { "." } else { dir };
            let candidate = go_path_clean(&format!("{dir}/{file}"));
            if find_executable(&candidate) {
                if !candidate.starts_with('/') {
                    return Err(exec_error(
                        "cannot run executable found relative to current directory",
                    ));
                }
                return Ok(candidate);
            }
        }
    }
    Err(exec_error("executable file not found in $PATH"))
}

/// Go os/exec/lp_unix.go `findExecutable(file) == nil`.
#[cfg(unix)]
fn find_executable(file: &str) -> bool {
    use rustix::fs::{Access, AtFlags, CWD, accessat};
    use rustix::io::Errno;
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(file) else {
        return false;
    };
    if metadata.is_dir() {
        return false;
    }
    match accessat(CWD, file, Access::EXEC_OK, AtFlags::EACCESS) {
        Ok(()) => true,
        // ENOSYS means Eaccess is not available or not implemented.
        // EPERM can be returned by Linux containers employing seccomp.
        // In both cases, fall back to checking the permission bits.
        Err(Errno::NOSYS | Errno::PERM) => metadata.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// Go `filepath.Abs(path)` on Unix (path/filepath/path_unix.go `unixAbs`).
#[cfg(unix)]
fn go_abs(path: &str) -> Result<String, GoError> {
    if path.starts_with('/') {
        return Ok(go_path_clean(path));
    }
    let wd = crate::frontend::vfs::os_current_dir()
        .map_err(|err| crate::gostd::errors::new(crate::frontend::vfs::getwd_error_text(&err)))?;
    Ok(go_path_clean(&format!("{wd}/{path}")))
}

/// Go `path.Clean` (the Unix `filepath.Clean`).
#[cfg(unix)]
fn go_path_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let path = path.as_bytes();
    let rooted = path[0] == b'/';
    let n = path.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.push(b'/');
        (r, dotdot) = (1, 1);
    }
    while r < n {
        if path[r] == b'/' {
            r += 1;
        } else if path[r] == b'.' && (r + 1 == n || path[r + 1] == b'/') {
            r += 1;
        } else if path[r] == b'.' && path[r + 1] == b'.' && (r + 2 == n || path[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                // Go drops bytes up to and with the last '/'.
                let mut last = out.pop();
                while out.len() > dotdot && last != Some(b'/') {
                    last = out.pop();
                }
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && path[r] != b'/' {
                out.push(path[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Go: cmd/tsc/sys.go:74 spawnProcess (tsgo#4712), on Windows
// spawnProcess launches a process and adapts its stdio to an io.ReadWriteCloser (Read is its stdout,
// Write is its stdin).
// PORT: Go `exec.Command` and `Start` find the program with
// os/exec/lp_windows.go (`win_exec`): `LookPath` for a bare name, else
// `lookExtensions` (in `Command` for an absolute name, in `Start` against
// `Dir` for a relative one). syscall `StartProcess` then makes the path
// absolute against `Dir`. Their Go error texts reach the content mapper
// diagnostics, so this makes the same texts. The child's stdin, stdout and
// stderr are anonymous pipes (`std::io::pipe`), as in Go. `Close` kills and
// reaps the child, which closes the child's ends, so a blocked read of its
// stdout or stderr ends. Go `stderr` `io.Discard` is `None` (the null
// device here). Go's `Cmd.environ` adds no `PWD` on Windows.
// PORT divergence: std's `Command` starts the command line with the path
// it runs, where Go keeps the name as the first word.
#[cfg(windows)]
pub fn spawn_process(
    command: &[String],
    dir: &str,
    stderr: Option<Box<dyn std::io::Write + Send>>,
) -> Result<Arc<dyn ProcessExitState>, GoError> {
    use std::process::{Command, Stdio};

    let name = command.first().map_or("", String::as_str);
    // Go `exec.Command`: `LookPath` when the name is its own `Base`, and
    // `lookExtensions(name, "")` for an absolute name. An error there is
    // `cmd.Err`, which `Start` returns.
    let (path, looked_up) = if win_exec::base(name) == name {
        (win_exec::look_path(name)?, None)
    } else if win_exec::is_abs(name) {
        (name.to_string(), Some(win_exec::look_extensions(name, "")?))
    } else {
        (name.to_string(), None)
    };
    // Go `Start`: "exec: no command" for an empty path.
    if path.is_empty() {
        return Err(crate::gostd::errors::new("exec: no command"));
    }
    // Go `Start` on Windows: the extension lookup that `Command` did not
    // make, against `Dir`.
    let lp = match looked_up {
        Some(lp) => lp,
        None => win_exec::look_extensions(&path, dir)?,
    };
    // Go os/exec_posix.go startProcess: the `Dir` check with op "chdir".
    if !dir.is_empty()
        && let Err(err) = std::fs::metadata(crate::frontend::vfs::os_path(dir))
    {
        return Err(crate::gostd::errors::new(format!(
            "chdir {dir}: {}",
            io_error_text(&err)
        )));
    }
    let fork_exec_error =
        |text: String| crate::gostd::errors::new(format!("fork/exec {lp}: {text}"));
    // Go syscall/exec_windows.go StartProcess: CreateProcess looks for the
    // program before it changes to `Dir`, so a path relative to `Dir` is
    // made absolute first.
    let program = if dir.is_empty() {
        lp.clone()
    } else {
        win_exec::join_exe_dir_and_fname(dir, &lp).map_err(fork_exec_error)?
    };
    let io_error = |err: std::io::Error| crate::gostd::errors::new(io_error_text(&err));
    let (child_stdin, stdin) = std::io::pipe().map_err(io_error)?;
    let (stdout, child_stdout) = std::io::pipe().map_err(io_error)?;
    let mut cmd = Command::new(crate::frontend::vfs::os_path(&program).as_os_str());
    cmd.args(&command[1..]);
    if !dir.is_empty() {
        cmd.current_dir(crate::frontend::vfs::os_path(dir));
    }
    cmd.stdin(Stdio::from(child_stdin));
    cmd.stdout(Stdio::from(child_stdout));
    let mut stderr_copy = None;
    match stderr {
        Some(writer) => {
            let (reader, child_stderr) = std::io::pipe().map_err(io_error)?;
            cmd.stderr(Stdio::from(child_stderr));
            stderr_copy = Some((reader, writer));
        }
        None => {
            cmd.stderr(Stdio::null());
        }
    }
    let spawned = cmd.spawn();
    // The command holds the child's ends; drop them so that a read sees the
    // end of the stream when the child exits.
    drop(cmd);
    let child = spawned.map_err(|err| fork_exec_error(io_error_text(&err)))?;
    // Go copies a non-file `cmd.Stderr` on a goroutine.
    let stderr_done = stderr_copy.map(|(mut reader, mut writer)| {
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut writer);
            let _ = done_tx.send(());
        });
        done
    });
    Ok(Arc::new(ChildProcess {
        child: Mutex::new(Some(child)),
        stdin: Mutex::new(Some(Arc::new(stdin))),
        stdout,
        stderr_done: Mutex::new(stderr_done),
        exit_code: Mutex::new(None),
    }))
}

// Go: cmd/tsc/sys.go:95 childProcess (tsgo#4712), on Windows
// PORT: `stdin` is `None` after `Close`; a write that runs meanwhile keeps
// its own reference, so `Close` never waits for it. `stderr_done` is the
// end signal of the stderr copy.
#[cfg(windows)]
struct ChildProcess {
    child: Mutex<Option<std::process::Child>>,
    stdin: Mutex<Option<Arc<std::io::PipeWriter>>>,
    stdout: std::io::PipeReader,
    stderr_done: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    exit_code: Mutex<Option<i32>>,
}

#[cfg(windows)]
impl crate::ipc::ReadWriteCloser for ChildProcess {
    // Go: cmd/tsc/sys.go:101 childProcess.Read
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut &self.stdout, buf)
    }

    // Go: cmd/tsc/sys.go:102 childProcess.Write
    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        let stdin = self
            .stdin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match stdin {
            Some(stdin) => std::io::Write::write(&mut &*stdin, buf),
            None => Err(std::io::ErrorKind::BrokenPipe.into()),
        }
    }

    fn flush(&self) -> std::io::Result<()> {
        Ok(())
    }

    // Go: cmd/tsc/sys.go:111 childProcess.Close
    // PORT: as on Unix (see there). Go's `Process.Kill` is TerminateProcess
    // with exit code 1, and so is std's `kill`.
    fn close(&self) -> Result<(), GoError> {
        drop(
            self.stdin
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Err(crate::gostd::errors::new("exec: Wait was already called"));
        };
        let _ = child.kill();
        let waited = child.wait();
        let stderr_done = self
            .stderr_done
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(done) = stderr_done {
            let _ = done.recv_timeout(CHILD_PROCESS_WAIT_DELAY);
        }
        match waited {
            Ok(status) => {
                *self
                    .exit_code
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(status.code().unwrap_or(-1));
                Ok(())
            }
            Err(err) => Err(crate::gostd::errors::new(format!(
                "wait: {}",
                io_error_text(&err)
            ))),
        }
    }
}

/// Go os/exec/lp_windows.go (go1.27.1) and the Windows `path/filepath` and
/// syscall helpers that it and `StartProcess` use. Each function returns
/// the Go error text of its Go counterpart.
// PORT: Go `execerrdot` is its default: a match in the current directory
// or in a relative PATH entry is the `ErrDot` error.
#[cfg(windows)]
mod win_exec {
    use crate::frontend::vfs::{
        filepath_clean, filepath_volume_name_len, go_string_from_os, os_path,
        win_is_path_separator as is_slash,
    };
    use crate::fswatch::syscall::io_error_text;
    use crate::gostd::{GoError, errors, strconv};

    /// Go `exec.ErrNotFound` on Windows.
    const ERR_NOT_FOUND: &str = "executable file not found in %PATH%";
    /// Go `exec.ErrDot`.
    const ERR_DOT: &str = "cannot run executable found relative to current directory";
    /// Go `syscall.EINVAL` on Windows.
    const EINVAL: &str = "invalid argument";

    /// Go `&exec.Error{Name: name, Err: err}`.
    fn exec_error(name: &str, err: &str) -> GoError {
        errors::new(format!("exec: {}: {err}", strconv::quote(name)))
    }

    // Go: os/exec/exec.go validateLookPath
    fn validate_look_path(s: &str) -> Result<(), GoError> {
        match s {
            "" | "." | ".." => Err(exec_error(s, ERR_NOT_FOUND)),
            _ => Ok(()),
        }
    }

    // Go: lp_windows.go chkStat
    // PORT: Go `os.Stat` fails with op "GetFileAttributesEx" when the file
    // does not exist, else with op "CreateFile".
    fn chk_stat(file: &str) -> Result<(), String> {
        match std::fs::metadata(os_path(file)) {
            Ok(d) if d.is_dir() => Err("permission denied".to_string()),
            Ok(_) => Ok(()),
            Err(err) => {
                let op = if err.kind() == std::io::ErrorKind::NotFound {
                    "GetFileAttributesEx"
                } else {
                    "CreateFile"
                };
                Err(format!("{op} {file}: {}", io_error_text(&err)))
            }
        }
    }

    // Go: lp_windows.go hasExt
    fn has_ext(file: &str) -> bool {
        match file.rfind('.') {
            None => false,
            Some(i) => file.rfind([':', '\\', '/']).is_none_or(|j| j < i),
        }
    }

    // Go: lp_windows.go findExecutable
    fn find_executable(file: &str, exts: &[String]) -> Result<String, String> {
        if exts.is_empty() {
            return chk_stat(file).map(|()| file.to_string());
        }
        if has_ext(file) && chk_stat(file).is_ok() {
            return Ok(file.to_string());
        }
        for e in exts {
            let f = format!("{file}{e}");
            if chk_stat(&f).is_ok() {
                return Ok(f);
            }
        }
        if has_ext(file) {
            return Err("file does not exist".to_string());
        }
        Err(ERR_NOT_FOUND.to_string())
    }

    // Go: lp_windows.go lookPath (exec.LookPath)
    pub fn look_path(file: &str) -> Result<String, GoError> {
        validate_look_path(file)?;
        look_path_exts(file, &path_ext())
    }

    // Go: lp_windows.go lookExtensions
    /// The path of `path` with the extension that `LookPath` would add,
    /// looked up against `dir` when `path` is relative to it.
    pub fn look_extensions(path: &str, dir: &str) -> Result<String, GoError> {
        validate_look_path(path)?;
        let path = if base(path) == path {
            format!(".\\{path}")
        } else {
            path.to_string()
        };
        let exts = path_ext();
        let ext = ext(&path);
        if !ext.is_empty() && exts.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return Ok(path);
        }
        if dir.is_empty()
            || filepath_volume_name_len(path.as_bytes()) != 0
            || (path.len() > 1 && is_slash(path.as_bytes()[0]))
        {
            return look_path_exts(&path, &exts);
        }
        let dirandpath = join(dir, &path);
        // We assume that LookPath will only add file extension.
        let lp = look_path_exts(&dirandpath, &exts)?;
        let ext = lp.strip_prefix(dirandpath.as_str()).unwrap_or(&lp);
        Ok(format!("{path}{ext}"))
    }

    // Go: lp_windows.go pathExt
    fn path_ext() -> Vec<String> {
        match std::env::var("PATHEXT") {
            Ok(x) if !x.is_empty() => x
                .to_lowercase()
                .split(';')
                .filter(|e| !e.is_empty())
                .map(|e| {
                    if e.starts_with('.') {
                        e.to_string()
                    } else {
                        format!(".{e}")
                    }
                })
                .collect(),
            _ => [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec(),
        }
    }

    // Go: lp_windows.go lookPathExts
    // PORT: Go compares the two matches with `os.Lstat` and `os.SameFile`;
    // `same_file` opens both (it follows a link, Lstat does not).
    fn look_path_exts(file: &str, exts: &[String]) -> Result<String, GoError> {
        if file.contains([':', '\\', '/']) {
            return find_executable(file, exts).map_err(|err| exec_error(file, &err));
        }
        // The first match that `ErrDot` rejects: one in the current
        // directory, or in a relative PATH entry.
        let mut dot: Option<String> = None;
        if std::env::var_os("NoDefaultCurrentDirectoryInExePath").is_none()
            && let Ok(f) = find_executable(&join(".", file), exts)
        {
            dot = Some(f);
        }
        let path = std::env::var("path").unwrap_or_default();
        for dir in split_list(&path) {
            if dir.is_empty() {
                continue;
            }
            let Ok(f) = find_executable(&join(&dir, file), exts) else {
                continue;
            };
            if let Some(dotf) = &dot
                && !same_file::is_same_file(os_path(dotf), os_path(&f)).unwrap_or(false)
            {
                return Err(exec_error(file, ERR_DOT));
            }
            if !is_abs(&f) {
                dot.get_or_insert(f);
                continue;
            }
            return Ok(f);
        }
        if dot.is_some() {
            return Err(exec_error(file, ERR_DOT));
        }
        Err(exec_error(file, ERR_NOT_FOUND))
    }

    // Go: path/filepath/path_windows.go splitList
    fn split_list(path: &str) -> Vec<String> {
        if path.is_empty() {
            return Vec::new();
        }
        // Split path, respecting but preserving quotes.
        let mut list = Vec::new();
        let mut start = 0;
        let mut quo = false;
        for (i, c) in path.bytes().enumerate() {
            match c {
                b'"' => quo = !quo,
                b';' if !quo => {
                    list.push(&path[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        list.push(&path[start..]);
        // Remove quotes.
        list.into_iter().map(|s| s.replace('"', "")).collect()
    }

    // Go: path/filepath/path_windows.go join, for two elements
    fn join(a: &str, b: &str) -> String {
        let mut out = String::new();
        let mut last = 0u8;
        for e in [a, b] {
            let mut e = e;
            // The first non-empty path element is added unchanged. After a
            // colon, the path stays relative to the current directory on a
            // drive and no separator is added.
            if !out.is_empty() && is_slash(last) {
                // If the path ends in a slash, strip any leading slashes from the next
                // path element to avoid creating a UNC path (any path starting with "\\")
                // from non-UNC elements.
                e = e.trim_start_matches(['\\', '/']);
                // If the path is \ and the next path element is ??,
                // add an extra .\ to create \.\?? rather than \??\
                // (a Root Local Device path).
                if out.len() == 1
                    && e.starts_with("??")
                    && (e.len() == 2 || is_slash(e.as_bytes()[2]))
                {
                    out.push_str(".\\");
                }
            } else if !out.is_empty() && last != b':' {
                // In all other cases, add a separator between elements.
                out.push('\\');
                last = b'\\';
            }
            if let Some(&l) = e.as_bytes().last() {
                out.push_str(e);
                last = l;
            }
        }
        if out.is_empty() {
            return out;
        }
        filepath_clean(&out)
    }

    // Go: internal/filepathlite/path.go Base (Windows)
    pub fn base(path: &str) -> &str {
        if path.is_empty() {
            return ".";
        }
        // Strip trailing slashes.
        let path = path.trim_end_matches(['\\', '/']);
        // Throw away volume name
        let path = &path[filepath_volume_name_len(path.as_bytes()).min(path.len())..];
        // Find the last element
        let path = match path.rfind(['\\', '/']) {
            Some(i) => &path[i + 1..],
            None => path,
        };
        // If empty now, it had only slashes.
        if path.is_empty() { "\\" } else { path }
    }

    // Go: internal/filepathlite/path.go Ext
    fn ext(path: &str) -> &str {
        for (i, c) in path.bytes().enumerate().rev() {
            if is_slash(c) {
                break;
            }
            if c == b'.' {
                return &path[i..];
            }
        }
        ""
    }

    // Go: internal/filepathlite/path_windows.go IsAbs
    pub fn is_abs(path: &str) -> bool {
        let b = path.as_bytes();
        let l = filepath_volume_name_len(b);
        if l == 0 {
            return false;
        }
        // If the volume name starts with a double slash, this is an absolute path.
        if is_slash(b[0]) && is_slash(b[1]) {
            return true;
        }
        b.get(l).is_some_and(|&c| is_slash(c))
    }

    // Go: syscall/exec_windows.go FullPath (GetFullPathNameW)
    fn full_path(name: &str) -> Result<String, String> {
        std::path::absolute(os_path(name))
            .map(go_string_from_os)
            .map_err(|err| io_error_text(&err))
    }

    // Go: syscall/exec_windows.go normalizeDir
    fn normalize_dir(dir: &str) -> Result<String, String> {
        let ndir = full_path(dir)?;
        let b = ndir.as_bytes();
        if b.len() > 2 && is_slash(b[0]) && is_slash(b[1]) {
            // dir cannot have \\server\share\path form
            return Err(EINVAL.to_string());
        }
        Ok(ndir)
    }

    // Go: syscall/exec_windows.go joinExeDirAndFName
    /// `p` made absolute against `dir`.
    pub fn join_exe_dir_and_fname(dir: &str, p: &str) -> Result<String, String> {
        let b = p.as_bytes();
        if b.is_empty() {
            return Err(EINVAL.to_string());
        }
        if b.len() > 2 && is_slash(b[0]) && is_slash(b[1]) {
            // \\server\share\path form
            return Ok(p.to_string());
        }
        if b.len() > 1 && b[1] == b':' {
            // has drive letter
            if b.len() == 2 {
                return Err(EINVAL.to_string());
            }
            if is_slash(b[2]) {
                return Ok(p.to_string());
            }
            let d = normalize_dir(dir)?;
            if b[0].to_ascii_uppercase() == d.as_bytes()[0].to_ascii_uppercase() {
                full_path(&format!("{d}\\{}", &p[2..]))
            } else {
                full_path(p)
            }
        } else {
            // no drive letter
            let d = normalize_dir(dir)?;
            if is_slash(b[0]) {
                full_path(&format!("{}{p}", d.get(..2).unwrap_or(&d)))
            } else {
                full_path(&format!("{d}\\{p}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    // Go: execute/tsc/emit_test.go:22 contentMapperLoggingTestSystem (tsgo#4712)
    // PORT: Go embeds `timingTestSystem`. The logger reads only
    // `GetEnvironmentVariable` and `ErrorWriter`; the file system is Go's
    // nil `fs` there, and here a call to it fails the test.
    struct ContentMapperLoggingTestSystem {
        enabled: Cell<bool>,
        stderr: Arc<Mutex<Vec<u8>>>,
    }

    impl System for ContentMapperLoggingTestSystem {
        fn writer(&self) -> Writer {
            Rc::new(RefCell::new(std::io::sink()))
        }
        fn error_writer(&self) -> ErrorWriter {
            self.stderr.clone()
        }
        fn fs(&self) -> Rc<dyn Fs> {
            unreachable!("the content mapper logger reads no file system")
        }
        fn default_library_path(&self) -> String {
            "/lib".to_string()
        }
        fn get_current_directory(&self) -> String {
            "/project".to_string()
        }
        fn write_output_is_tty(&self) -> bool {
            false
        }
        fn get_width_of_terminal(&self) -> i32 {
            0
        }
        fn get_environment_variable(&self, name: &str) -> (String, bool) {
            if name == "TS_CONTENT_MAPPER_DEBUG" && self.enabled.get() {
                return ("1".to_string(), true);
            }
            (String::new(), false)
        }
        fn spawn(
            &self,
            _command: &[String],
            _dir: &str,
            _stderr: Option<Box<dyn std::io::Write + Send>>,
        ) -> Result<Arc<dyn ProcessExitState>, GoError> {
            Err(crate::gostd::errors::new(
                "spawn not implemented in timingTestSystem",
            ))
        }
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
        fn since_start(&self) -> Duration {
            Duration::ZERO
        }
    }

    // Go: execute/tsc/emit_test.go:39 TestContentMapperLoggerEnvironmentVariable (tsgo#4712)
    #[test]
    fn test_content_mapper_logger_environment_variable() {
        let sys = ContentMapperLoggingTestSystem {
            enabled: Cell::new(false),
            stderr: Arc::default(),
        };
        assert!(new_content_mapper_logger(&sys).is_none());
        sys.enabled.set(true);
        let logger = new_content_mapper_logger(&sys).expect("logger");
        std::thread::scope(|scope| {
            for _ in 0..10 {
                let logger = logger.clone();
                scope.spawn(move || logger("mapper log"));
            }
        });
        let stderr = sys.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(String::from_utf8_lossy(&stderr), "mapper log\n".repeat(10));
    }

    // Go: cmd/tsc/sys_unix_test.go:20 TestChildProcessCloseDoesNotWaitForLauncherDescendants (tsgo#4712, ts#64082)
    // PORT: Go reads the first line with `bufio.Reader`; this reads bytes
    // up to the newline. Go `syscall.Kill` is rustix `kill_process`.
    // PORT: Go N (ts#64082) runs the test binary itself as the launcher and
    // the descendant, so that `go test` needs no shell. libtest prints its
    // own lines on stdout before a test body runs, so the pid would not be
    // the first line; the port keeps the `sh -c "nohup sleep 60 & echo $!;
    // wait"` launcher of the older Go test. The checks are the same.
    #[cfg(unix)]
    #[test]
    fn test_child_process_close_does_not_wait_for_launcher_descendants() {
        // cmd/tsgo/isprocessalive_unix.rs `is_process_alive` (cmd is in the upper
        // crate): `kill(pid, 0)`, alive on success or EPERM.
        fn is_process_alive(pid: i32) -> bool {
            Pid::from_raw(pid.max(0)).is_some_and(|pid| {
                matches!(
                    rustix::process::test_kill_process(pid),
                    Ok(()) | Err(rustix::io::Errno::PERM)
                )
            })
        }
        use rustix::process::{Pid, Signal, kill_process};

        let command: Vec<String> = ["sh", "-c", "nohup sleep 60 & echo $!; wait"]
            .iter()
            .map(|arg| (*arg).to_string())
            .collect();
        let process =
            spawn_process(&command, "", Some(Box::new(Vec::<u8>::new()))).expect("spawnProcess");
        let mut pid_text = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = process.read(&mut byte).expect("ReadString");
            assert_ne!(n, 0, "EOF before the pid line");
            if byte[0] == b'\n' {
                break;
            }
            pid_text.push(byte[0]);
        }
        let descendant_pid: i32 = String::from_utf8_lossy(&pid_text)
            .trim()
            .parse()
            .expect("strconv.Atoi");
        let kill = |pid: i32| {
            if let Some(pid) = Pid::from_raw(pid) {
                let _ = kill_process(pid, Signal::KILL);
            }
        };
        let (done_tx, done) = std::sync::mpsc::channel();
        {
            let process = process.clone();
            std::thread::spawn(move || {
                let _ = done_tx.send(process.close());
            });
        }

        let completed = match done.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => {
                assert!(
                    result.is_ok(),
                    "Close: {:?}",
                    result.err().map(|e| e.error())
                );
                true
            }
            Err(_) => {
                kill(descendant_pid);
                let _ = done.recv();
                false
            }
        };
        assert!(
            completed,
            "child process shutdown waited for a launcher descendant"
        );
        if is_process_alive(descendant_pid) {
            kill(descendant_pid);
        }
    }
}
