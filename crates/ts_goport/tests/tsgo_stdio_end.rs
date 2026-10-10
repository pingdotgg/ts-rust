//! How the long-running modes of tsgo end on SIGINT, SIGTERM and the end of
//! stdin, as Go's tsgo ends them (cmd/tsc/main.go and api.go wait on
//! `signal.NotifyContext`).
//!
//! - `-w`: the watch loop selects on `ctx.Done()` (Go
//!   execute/watchmanager/watchmanager.go:354), so a signal ends the run at
//!   once with exit code 0. The port waited on the cycle channel with a
//!   50 ms timeout and ended up to 50 ms later.
//! - `--api`: the sync API (`ipc/conn_sync.go:55`) runs a request inline
//!   and checks the context after it, so a signal during a request ends
//!   the run right after the answer.
//!
//! `--api --async` is not here. Go's read loop waits in its read
//! (`ipc/conn_async.go:87`) while a request runs on its own goroutine
//! (`:98`), so a signal during that request ends the run after the next
//! message or at the end of stdin. The port ends right after the answer:
//! PORTING.md, "Not ported (plan level)", "The end on SIGINT or SIGTERM in
//! `--api --async`".
//!
//! `--lsp` is not here. Go's `Run` (lsp/server.go:859) does not wait for
//! the work that Go runs on goroutines (the async part of a request, an API
//! session), and the port waits for it: PORTING.md, "Not ported (plan
//! level)", "The end of a run".
//!
//! A request that the test holds open reads its tsconfig.json from a FIFO:
//! the read waits until the test writes the text, and the test's open for
//! writing returns once tsgo has opened the FIFO.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

/// The longest wait for a step that ends in milliseconds when it works.
const LIMIT: Duration = Duration::from_secs(20);

/// The longest time from the event that ends a run to its exit, for a run
/// that ends at once. tsgo ends in milliseconds; the rest is room for a
/// loaded host.
const AT_ONCE: Duration = Duration::from_secs(2);

#[test]
fn watch_ends_soon_after_sigint_or_sigterm() {
    let dir = TempDir::new("watch");
    dir.write(
        "tsconfig.json",
        r#"{"compilerOptions":{"strict":true,"outDir":"out"},"include":["src"]}"#,
    );
    dir.write("src/a.ts", "export const a: number = 1;\n");
    let mut took = Vec::new();
    for signal in [Signal::INT, Signal::TERM].into_iter().cycle().take(7) {
        let mut tsgo = Tsgo::start(&["-w", "-p", "tsconfig.json"], &dir.0);
        tsgo.wait_stdout("Watching for file changes");
        let start = Instant::now();
        tsgo.signal(signal);
        let (status, exited) = tsgo
            .wait_exit(LIMIT)
            .expect("tsgo -w did not end after the signal");
        took.push(exited - start);
        assert_eq!(status.code(), Some(0), "{signal:?}: {status:?}");
        assert_eq!(tsgo.stderr(), "", "{signal:?}");
    }
    took.sort();
    // From the signal to the first `try_wait` that sees the exit, the
    // median on zbook is 1 to 4 ms (Go: about 5 ms). The old wait with a
    // 50 ms timeout gives 50 to 53 ms: its wait starts when tsgo prints the
    // line above.
    assert!(
        took[3] < Duration::from_millis(25),
        "median {:?}, all {took:?}",
        took[3]
    );
}

/// The sync API only: `--api --async` is not here (file header).
#[test]
fn api_signal_during_a_request() {
    let dir = TempDir::new("api");
    dir.write("src/a.ts", "export const a: number = 1;\n");
    let config = dir.fifo("tsconfig.json");
    let cwd = dir.0.to_str().unwrap();
    let mut tsgo = Tsgo::start(&["--api", "--cwd", cwd], &dir.0);
    let params = format!(r#"{{"openProjects":["{}"]}}"#, config.display());
    tsgo.send(&msgpack_request("createSnapshot", &params));
    // The request now waits in the read of the FIFO.
    let mut writer = open_fifo_writer(&config);
    tsgo.signal(Signal::INT);
    writer
        .write_all(br#"{"compilerOptions":{"strict":true},"include":["src"]}"#)
        .unwrap();
    drop(writer);
    // The msgpack answer names its method.
    tsgo.wait_stdout("createSnapshot");
    tsgo.expect_end(Instant::now(), "the sync API after the answer", 0, "");
}

/// A tsgo child with piped stdio. Its stdout and stderr are read on
/// threads into buffers. Drop kills a tsgo that still runs.
struct Tsgo {
    child: Child,
    stdin: ChildStdin,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Option<ExitStatus>,
    /// The threads that read stdout and stderr. They end at the end of the
    /// pipes.
    readers: Vec<JoinHandle<()>>,
}

impl Tsgo {
    fn start(args: &[&str], dir: &Path) -> Tsgo {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut readers = Vec::new();
        {
            let mut out = child.stdout.take().unwrap();
            let stdout = stdout.clone();
            readers.push(std::thread::spawn(move || {
                let mut buf = vec![0; 65536];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    stdout.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            }));
        }
        {
            let mut err = child.stderr.take().unwrap();
            let stderr = stderr.clone();
            readers.push(std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                stderr.lock().unwrap().extend_from_slice(&buf);
            }));
        }
        Tsgo {
            child,
            stdin,
            stdout,
            stderr,
            status: None,
            readers,
        }
    }

    /// Checks that tsgo exits within `AT_ONCE` of `since` with `code` and
    /// `stderr`. `what` names the run in a failure.
    fn expect_end(&mut self, since: Instant, what: &str, code: i32, stderr: &str) {
        let (status, exited) = self
            .wait_exit(LIMIT)
            .unwrap_or_else(|| panic!("{what}: tsgo did not end"));
        assert!(
            exited - since < AT_ONCE,
            "{what}: tsgo took {:?} to end",
            exited - since
        );
        assert_eq!(status.code(), Some(code), "{what}: {status:?}");
        assert_eq!(self.stderr(), stderr, "{what}");
    }

    fn send(&mut self, bytes: &[u8]) {
        self.stdin.write_all(bytes).unwrap();
        self.stdin.flush().unwrap();
    }

    fn signal(&self, signal: Signal) {
        let pid = Pid::from_raw(self.child.id().cast_signed()).unwrap();
        kill_process(pid, signal).unwrap();
    }

    fn wait_stdout(&self, text: &str) {
        let end = Instant::now() + LIMIT;
        loop {
            if String::from_utf8_lossy(&self.stdout.lock().unwrap()).contains(text) {
                return;
            }
            assert!(
                Instant::now() < end,
                "no {text:?} on stdout: {:?}; stderr: {:?}",
                String::from_utf8_lossy(&self.stdout.lock().unwrap()),
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The exit status and the time when `try_wait` first saw the exit, or
    /// None when tsgo still runs after `limit`. On an exit it also waits
    /// for the reader threads, so stdout and stderr are whole.
    fn wait_exit(&mut self, limit: Duration) -> Option<(ExitStatus, Instant)> {
        let end = Instant::now() + limit;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                let exited = Instant::now();
                self.status = Some(status);
                let end = exited + LIMIT;
                while !self.readers.iter().all(JoinHandle::is_finished) && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(1));
                }
                return Some((status, exited));
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }
}

impl Drop for Tsgo {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let status = self.child.wait();
            if let Ok(status) = status
                && status.signal().is_none()
            {
                eprintln!("tsgo ended with {status:?}");
            }
        }
    }
}

/// A request of the msgpack API: `[1, method, params]` with both as bin.
fn msgpack_request(method: &str, params: &str) -> Vec<u8> {
    let mut out = vec![0x93, 0x01];
    for part in [method.as_bytes(), params.as_bytes()] {
        out.push(0xc5);
        out.extend_from_slice(&u16::try_from(part.len()).unwrap().to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// Opens `fifo` for writing. The open returns once a reader has opened it.
fn open_fifo_writer(fifo: &Path) -> std::fs::File {
    let (tx, rx) = mpsc::channel();
    let path = fifo.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(std::fs::OpenOptions::new().write(true).open(path));
    });
    rx.recv_timeout(LIMIT)
        .expect("tsgo did not open the FIFO")
        .unwrap()
}

static NEXT: AtomicU32 = AtomicU32::new(0);

fn unique(prefix: &str) -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("tsgo_stdio_end-{}-{prefix}{n}", std::process::id()))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> TempDir {
        let path = unique(prefix);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn fifo(&self, rel: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mode = rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR;
        rustix::fs::mkfifoat(rustix::fs::CWD, &path, mode).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
