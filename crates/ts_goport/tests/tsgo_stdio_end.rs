//! How the long-running modes of tsgo end on SIGINT, SIGTERM and the end of
//! stdin, as Go's tsgo ends them (cmd/tsc/main.go, lsp.go and api.go wait on
//! `signal.NotifyContext`).
//!
//! - `-w`: the watch loop selects on `ctx.Done()` (Go
//!   execute/watchmanager/watchmanager.go:354), so a signal ends the run at
//!   once with exit code 0. The port waited on the cycle channel with a
//!   50 ms timeout and ended up to 50 ms later.
//! - `--api --async`: Go's read loop (`ipc/conn_async.go:82`) checks the
//!   context, then waits in the read while a request runs on its own
//!   goroutine. A signal during that request does not end the run: the run
//!   ends after the next message or at the end of stdin. The port ran the
//!   request inline and checked the context after it. The sync API
//!   (`ipc/conn_sync.go:55`) runs the request inline in Go too, so it ends
//!   right after the answer.
//! - `--lsp`: Go's `Run` (lsp/server.go:859) returns once its dispatch loop
//!   is back in `requestQueue.Get`; the async part of a request and an API
//!   session run on other goroutines, and `main` exits while they run. The
//!   port runs that work on the dispatch thread and waited for it.
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
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

/// The longest wait for a step that ends in milliseconds when it works.
const LIMIT: Duration = Duration::from_secs(20);

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
        let status = tsgo
            .wait_exit(LIMIT)
            .expect("tsgo -w did not end after the signal");
        took.push(start.elapsed());
        assert_eq!(status.code(), Some(0), "{signal:?}: {status:?}");
        assert_eq!(tsgo.stderr(), "", "{signal:?}");
    }
    took.sort();
    // The 50 ms wait gave 29 to 42 ms in the median on zbook; Go and the
    // select give 2 to 5 ms.
    assert!(
        took[3] < Duration::from_millis(25),
        "median {:?}, all {took:?}",
        took[3]
    );
}

#[test]
fn api_signal_during_a_request() {
    // (protocol flag, whether the run ends right after the answer)
    for (flags, ends_after_answer) in [(&["--async"][..], false), (&[][..], true)] {
        let dir = TempDir::new("api");
        dir.write("src/a.ts", "export const a: number = 1;\n");
        let config = dir.fifo("tsconfig.json");
        let cwd = dir.0.to_str().unwrap();
        let mut args = vec!["--api", "--cwd", cwd];
        args.extend_from_slice(flags);
        let mut tsgo = Tsgo::start(&args, &dir.0);
        let params = format!(r#"{{"openProjects":["{}"]}}"#, config.display());
        let answered = if flags.is_empty() {
            tsgo.send(&msgpack_request("createSnapshot", &params));
            // The msgpack answer names its method.
            "createSnapshot"
        } else {
            tsgo.send(&frame(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"createSnapshot","params":{params}}}"#
            )));
            r#""id":1"#
        };
        // The request now waits in the read of the FIFO.
        let mut writer = open_fifo_writer(&config);
        tsgo.signal(Signal::INT);
        writer
            .write_all(br#"{"compilerOptions":{"strict":true},"include":["src"]}"#)
            .unwrap();
        drop(writer);
        tsgo.wait_stdout(answered);
        if ends_after_answer {
            let status = tsgo.wait_exit(LIMIT).expect("the sync API did not end");
            assert_eq!(status.code(), Some(0), "sync: {status:?}");
        } else {
            assert!(
                tsgo.wait_exit(Duration::from_millis(500)).is_none(),
                "the async API ended after the answer; Go waits in the read"
            );
            tsgo.close_stdin();
            let status = tsgo
                .wait_exit(LIMIT)
                .expect("the async API did not end at EOF");
            assert_eq!(status.code(), Some(0), "async: {status:?}");
        }
        assert_eq!(tsgo.stderr(), "", "{flags:?}");
    }
}

#[test]
fn lsp_ends_at_once_while_an_api_session_request_runs() {
    // (event, exit code, stderr)
    let cases: [(Option<Signal>, i32, &str); 3] = [
        (Some(Signal::INT), 1, "context canceled\n"),
        (Some(Signal::TERM), 1, "context canceled\n"),
        (None, 0, ""),
    ];
    for (signal, code, stderr) in cases {
        let dir = TempDir::new("lsp");
        dir.write("src/a.ts", "export const a = 1;\n");
        dir.write("other/src/b.ts", "export const b = 1;\n");
        let config = dir.fifo("other/tsconfig.json");
        let mut tsgo = Tsgo::start(&["--lsp", "--stdio"], &dir.0);
        tsgo.answer_server_requests();
        let root = format!("file://{}", dir.0.display());
        tsgo.send(&frame(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root}","capabilities":{{}}}}}}"#
        )));
        tsgo.wait_stdout(r#""id":1,"result""#);
        tsgo.send(&frame(
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        ));
        let socket = SocketPath::new();
        tsgo.send(&frame(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"custom/initializeAPISession","params":{{"pipe":"{}"}}}}"#,
            socket.0.display()
        )));
        tsgo.wait_stdout(r#""sessionId""#);
        let mut api = connect(&socket.0);
        api.write_all(&frame(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"createSnapshot","params":{{"openProjects":["{}"]}}}}"#,
            config.display()
        )))
        .unwrap();
        // The API request now waits in the read of the FIFO, on the
        // dispatch thread. The writer stays open, so the read does not end.
        let writer = open_fifo_writer(&config);
        match signal {
            Some(signal) => tsgo.signal(signal),
            None => tsgo.close_stdin(),
        }
        let status = tsgo
            .wait_exit(LIMIT)
            .unwrap_or_else(|| panic!("{signal:?}: the LSP waited for the API request"));
        assert_eq!(status.code(), Some(code), "{signal:?}: {status:?}");
        assert_eq!(tsgo.stderr(), stderr, "{signal:?}");
        drop(writer);
    }
}

/// A tsgo child with piped stdio. Its stdout and stderr are read on
/// threads into buffers. Drop kills a tsgo that still runs.
struct Tsgo {
    child: Child,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Option<ExitStatus>,
    answer: Arc<Mutex<bool>>,
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
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new(Mutex::new(false));
        {
            let mut out = child.stdout.take().unwrap();
            let (stdout, stdin, answer) = (stdout.clone(), stdin.clone(), answer.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0; 65536];
                let mut answered = 0;
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut stdout = stdout.lock().unwrap();
                    stdout.extend_from_slice(&buf[..n]);
                    if *answer.lock().unwrap() {
                        answered = answer_requests(&stdout, answered, &stdin);
                    }
                }
            });
        }
        {
            let mut err = child.stderr.take().unwrap();
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                stderr.lock().unwrap().extend_from_slice(&buf);
            });
        }
        Tsgo {
            child,
            stdin,
            stdout,
            stderr,
            status: None,
            answer,
        }
    }

    /// Answers each LSP request of the server with a null result, as an
    /// editor does, so that no handler waits for the client.
    fn answer_server_requests(&mut self) {
        *self.answer.lock().unwrap() = true;
    }

    fn send(&self, bytes: &[u8]) {
        let mut stdin = self.stdin.lock().unwrap();
        let stdin = stdin.as_mut().expect("stdin is open");
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn close_stdin(&self) {
        *self.stdin.lock().unwrap() = None;
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

    /// The exit status, or None when tsgo still runs after `limit`.
    fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        let end = Instant::now() + limit;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.status = Some(status);
                // The reader threads end at the end of the pipes.
                std::thread::sleep(Duration::from_millis(20));
                return Some(status);
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

/// Answers the server requests among the LSP messages of `stdout` from
/// offset `from` on, and returns the offset after the last whole message.
fn answer_requests(stdout: &[u8], mut from: usize, stdin: &Mutex<Option<ChildStdin>>) -> usize {
    while let Some((body, next)) = next_frame(stdout, from) {
        from = next;
        let keys = top_level_keys(body);
        let id = keys.iter().find(|(key, _)| key == "id");
        if let Some((_, id)) = id
            && keys.iter().any(|(key, _)| key == "method")
            && let Some(stdin) = stdin.lock().unwrap().as_mut()
        {
            let answer = frame(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#));
            let _ = stdin.write_all(&answer);
            let _ = stdin.flush();
        }
    }
    from
}

/// The body of the LSP message at `from` and the offset after it.
fn next_frame(buf: &[u8], from: usize) -> Option<(&str, usize)> {
    let rest = &buf[from..];
    let header_end = rest.windows(4).position(|w| w == b"\r\n\r\n")?;
    let header = std::str::from_utf8(&rest[..header_end]).ok()?;
    let length: usize = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))?
        .trim()
        .parse()
        .ok()?;
    let start = header_end + 4;
    let body = rest.get(start..start + length)?;
    Some((std::str::from_utf8(body).ok()?, from + start + length))
}

/// The keys of a JSON object with the raw text of their values, for the
/// top level only.
fn top_level_keys(json: &str) -> Vec<(String, String)> {
    let bytes = json.as_bytes();
    let mut keys = Vec::new();
    let (mut depth, mut i) = (0, 0);
    let mut key: Option<String> = None;
    let mut value_start = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i + 1;
                i += 1;
                while bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if depth == 1 && value_start.is_none() && key.is_none() {
                    key = Some(json[start..i].to_string());
                }
            }
            b':' if depth == 1 && key.is_some() && value_start.is_none() => {
                value_start = Some(i + 1);
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' | b',' => {
                if depth == 1
                    && let (Some(k), Some(s)) = (key.take(), value_start.take())
                {
                    keys.push((k, json[s..i].trim().to_string()));
                }
                if bytes[i] != b',' {
                    depth -= 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// An LSP base-protocol message (Content-Length framing), as the JSON-RPC
/// API and the LSP use.
fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
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

fn connect(path: &Path) -> std::os::unix::net::UnixStream {
    let end = Instant::now() + LIMIT;
    loop {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(stream) => return stream,
            Err(err) => {
                assert!(Instant::now() < end, "connect {}: {err}", path.display());
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
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

/// A short socket path (a Unix socket path has at most 107 bytes).
struct SocketPath(PathBuf);

impl SocketPath {
    fn new() -> SocketPath {
        SocketPath(unique("s").with_extension("sock"))
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
