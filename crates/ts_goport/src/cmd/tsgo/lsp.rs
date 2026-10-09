//! Go `cmd/tsc/lsp.go`.

use crate::cmd::tsgo::prelude::*;

#[cfg(not(unix))]
use crate::cmd::tsgo::isprocessalive_other::{PROCESS_ALIVE_SUPPORTED, is_process_alive};
#[cfg(unix)]
use crate::cmd::tsgo::isprocessalive_unix::{PROCESS_ALIVE_SUPPORTED, is_process_alive};
use crate::cmd::tsgo::main::{ErrorHandling, must_getwd, new_flag_set, notify_context};
use crate::execute::tsc::compile::{Writer, spawn_process};
use crate::execute::tsc::stdio;
use crate::frontend::bundled;
use crate::frontend::tspath;
use crate::frontend::vfs::osvfs;
use crate::gostd::context::{self, CancelFunc};
use crate::gostd::errors;
use std::io::Write;
use std::time::Duration;

// Go: cmd/tsc/lsp.go:21 runLSP
pub fn run_lsp(args: &[String]) -> i32 {
    let mut flag = new_flag_set("lsp", ErrorHandling::ContinueOnError);
    let stdio = flag.bool("stdio", false, "use stdio for communication");
    let pprof_dir = flag.string(
        "pprofDir",
        "",
        "Generate pprof CPU/memory profiles to the given directory.",
    );
    let pipe = flag.string("pipe", "", "use named pipe for communication");
    let _ = pipe;
    let socket = flag.string("socket", "", "use socket for communication");
    let _ = socket;
    let client_process_id = flag.int(
        "clientProcessId",
        0,
        "use the given PID for the parent process watchdog",
    );
    if flag.parse(args).is_err() {
        return 2;
    }

    if !stdio.get() {
        let _ = writeln!(stdio::Stderr, "only stdio is supported");
        return 1;
    }

    // Go: profileSession := pprof.BeginProfiling(*pprofDir, os.Stderr); defer profileSession.Stop()
    // PORT: the session stops when it drops at the return of `run_lsp`,
    // after `stop`, as the Go defers run (last in, first out).
    let _profile_session = if pprof_dir.borrow().is_empty() {
        None
    } else {
        let _ = writeln!(
            stdio::Stderr,
            "pprof profiles will be written to: {}",
            pprof_dir.borrow()
        );
        let stderr: Writer = Rc::new(RefCell::new(stdio::Stderr));
        Some(crate::pprof::begin_profiling(&pprof_dir.borrow(), stderr))
    };

    let fs = bundled::wrap_fs_exported(osvfs::osvfs_fs());
    let default_library_path = bundled::lib_path_exported();
    let typings_location = get_global_typings_cache_location();

    // Go: ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
    let (ctx, stop) = notify_context(&context::background());

    let s = crate::lsp::new_server(crate::lsp::ServerOptions {
        // Go: os.Stdin, os.Stdout and os.Stderr (see `execute::tsc::stdio`).
        in_: crate::lsp::to_reader(Box::new(std::io::BufReader::new(stdio::Stdin))),
        out: crate::lsp::to_writer(Box::new(stdio::Stdout)),
        err: Box::new(stdio::Stderr),
        cwd: must_getwd(),
        fs,
        default_library_path,
        typings_location,
        parse_cache: None,
        npm_install: Some(Box::new(|cwd: &str, args: &[String]| {
            // Go: cmd := exec.Command("npm", args...); cmd.Dir = cwd; return cmd.Output()
            // PORT: Go error texts are approximated (ATA only logs them).
            // `Output` reads stdin from the null device and keeps stdout and
            // stderr; `rlimit::spawn` gives npm the open-file limit as Go
            // does.
            let mut npm = std::process::Command::new("npm");
            npm.args(args)
                .current_dir(cwd)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            match crate::gostd::rlimit::spawn(&mut npm)
                .and_then(std::process::Child::wait_with_output)
            {
                Ok(output) => {
                    if output.status.success() {
                        (output.stdout, None)
                    } else {
                        let message = match output.status.code() {
                            Some(code) => format!("exit status {code}"),
                            None => output.status.to_string(),
                        };
                        (output.stdout, Some(errors::new(message)))
                    }
                }
                Err(err) => (Vec::new(), Some(errors::new(err.to_string()))),
            }
        })),
        // Go: Spawn: spawnProcess (tsgo#4712; Go cmd/tsc/sys.go spawnProcess is
        // ported in `execute::tsc::compile`).
        spawn: Some(Rc::new(spawn_process)),
        progress_delay: Duration::from_millis(250),
        set_parent_process_id: new_parent_process_watchdog(&ctx, &stop, client_process_id.get()),
    });

    let result = s.run(&ctx);
    // Go: defer stop()
    stop();
    if let Err(err) = result {
        let _ = writeln!(stdio::Stderr, "{}", err.error());
        return 1;
    }
    0
}

// Go: cmd/tsc/lsp.go:81 newParentProcessWatchdog
// newParentProcessWatchdog returns a SetParentProcessID callback if the platform
// supports process-alive checking and no client process ID override was provided,
// or nil otherwise.
// PORT: Go `clientProcessID` is an `int` (64 bits, see `FlagValue`); the
// watchdog takes the `i32` pid, as Linux `kill` takes a 32-bit `pid_t`.
pub fn new_parent_process_watchdog(
    ctx: &Context,
    stop: &CancelFunc,
    client_process_id: i64,
) -> Option<Box<dyn Fn(i32) + Send + Sync>> {
    if !PROCESS_ALIVE_SUPPORTED {
        return None;
    }
    if client_process_id > 0 {
        start_parent_process_watchdog(ctx, stop, client_process_id as i32);
        return None;
    }
    let ctx = ctx.clone();
    let stop = stop.clone();
    Some(Box::new(move |parent_pid: i32| {
        start_parent_process_watchdog(&ctx, &stop, parent_pid);
    }))
}

// Go: cmd/tsc/lsp.go:97 startParentProcessWatchdog
// startParentProcessWatchdog starts a goroutine that monitors the parent process
// and cancels the context if the parent dies. This prevents orphaned language
// server processes when the editor crashes or is killed.
pub fn start_parent_process_watchdog(ctx: &Context, stop: &CancelFunc, parent_pid: i32) {
    if parent_pid <= 0 {
        return;
    }
    let ctx = ctx.clone();
    let stop = stop.clone();
    crate::core::GoThread::new()
        .name("lsp-watchdog".to_string())
        .spawn(move || {
            // Go: ticker := time.NewTicker(5 * time.Second); defer ticker.Stop()
            // PORT: the `select` on `ctx.Done()` and `ticker.C` is a 5 s wait
            // on `ctx.Done()`.
            let done = ctx.done();
            loop {
                let closed = match &done {
                    Some(done) => done.wait_timeout(Duration::from_secs(5)),
                    None => {
                        std::thread::sleep(Duration::from_secs(5));
                        false
                    }
                };
                if closed {
                    return;
                }
                if !is_process_alive(parent_pid) {
                    let _ = writeln!(
                        stdio::Stderr,
                        "Parent process {parent_pid} has exited, shutting down."
                    );
                    stop();
                    return;
                }
            }
        });
}

// Go: vfs/osvfs/os.go:191 GetGlobalTypingsCacheLocation
// PORT: ported here; `vfs/osvfs` is an accepted compiler file.
pub fn get_global_typings_cache_location() -> String {
    let cache_dir = match user_cache_dir() {
        Ok(dir) => dir,
        Err(_) => temp_dir(),
    };

    let subdir = if cfg!(windows) {
        "Microsoft/TypeScript"
    } else {
        "typescript"
    };
    tspath::combine_paths(&cache_dir, &[subdir, crate::core::version_major_minor()])
}

// Go: os/file.go:504 UserCacheDir
// PORT: the Windows, darwin and Unix branches (not plan9).
fn user_cache_dir() -> Result<String, GoError> {
    let getenv = |key: &str| std::env::var(key).unwrap_or_default();
    let mut dir;

    if cfg!(windows) {
        dir = getenv("LocalAppData");
        if dir.is_empty() {
            return Err(errors::new("%LocalAppData% is not defined"));
        }
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        dir = getenv("HOME");
        if dir.is_empty() {
            return Err(errors::new("$HOME is not defined"));
        }
        dir.push_str("/Library/Caches");
    } else {
        // Unix
        dir = getenv("XDG_CACHE_HOME");
        if dir.is_empty() {
            dir = getenv("HOME");
            if dir.is_empty() {
                return Err(errors::new("neither $XDG_CACHE_HOME nor $HOME are defined"));
            }
            dir.push_str("/.cache");
        } else if !dir.starts_with('/') {
            // Go: !filepathlite.IsAbs(dir)
            return Err(errors::new("path in $XDG_CACHE_HOME is relative"));
        }
    }

    Ok(dir)
}

// Go: os/file_unix.go:390 tempDir
fn temp_dir() -> String {
    let mut dir = std::env::var("TMPDIR").unwrap_or_default();
    if dir.is_empty() {
        if cfg!(target_os = "android") {
            dir = "/data/local/tmp".to_string();
        } else {
            dir = "/tmp".to_string();
        }
    }
    dir
}
