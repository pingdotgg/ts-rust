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
//!   the run right after the answer. A call to the client that waits does
//!   not wake at the signal (`conn_sync.go:185-189`).
//! - `--api --async`: Go's `Run` (`ipc/conn_async.go:68-113`) reads every
//!   message on its own goroutine (the read is `:87`) and checks the
//!   context before each read (`:83`). After a signal:
//!   - Phase A, until `Run` reads the next client message: a `Call` that
//!     waits returns `context canceled` at once, and a new `Call` writes
//!     its request and returns it too (`:281-291`). So a request answers
//!     at the signal, and a panic of a callback on a worker goroutine ends
//!     the process with exit code 2 at the signal.
//!   - `Run` reads exactly 1 more message, dispatches it and returns.
//!   - Phase B: the deferred `closePendingCalls` sets `terminal`
//!     (`ipc: connection closed` joined with `context canceled`), and a
//!     `Call` returns it and writes nothing (`:262-267`). This is a Go
//!     race that the deferred function wins in practice, also for the
//!     request read after the signal.
//!   - The end code is 0 (`api/server.go:134-139`).
//!
//!   A panic answer is compared up to its stack (Go's goroutine stack and
//!   the port's backtrace differ). `TSGO_STDIO_END_BIN` runs the tests on
//!   another tsgo, such as Go's.
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

/// The sync API. The `--api --async` tests come after it.
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

/// The first lines of the answers in `--api --async` after a signal: a
/// callback error in Phase A and in Phase B (file header).
const PANIC_CANCELED: &str = r#""message":"panic: context canceled\n"#;
const PANIC_CLOSED: &str = r#""message":"panic: ipc: connection closed\ncontext canceled\n"#;
/// A worker goroutine's panic in Phase A, the first line of stderr.
const REPANICKED_CANCELED: &str = "panic: context canceled [recovered, repanicked]\n";

/// How long a tsgo that waits for a client message must stay alive.
const ALIVE: Duration = Duration::from_millis(300);

/// A signal while a request waits for a client callback (readFile of
/// tsconfig.json): the request answers at once with the callback's error,
/// and tsgo ends right after the next client message, a reply or a ping.
#[test]
fn api_async_signal_while_a_callback_waits() {
    for next in ["reply", "ping"] {
        let what = format!("--api --async, readFile waits, SIGINT, then the {next}");
        let dir = project("cbwait");
        let mut tsgo = api_async(&dir, &["--callbacks", "readFile"]);
        tsgo.send_json(&create_snapshot(1, &dir.0));
        let call = tsgo.wait_callback("readFile");
        tsgo.signal(Signal::INT);
        tsgo.expect_answer(1, PANIC_CANCELED, &what);
        tsgo.expect_alive(&what);
        let start = Instant::now();
        if next == "reply" {
            tsgo.reply(&call);
        } else {
            tsgo.send_json(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        }
        tsgo.expect_end(start, &what, 0, "");
        if next == "ping" {
            tsgo.expect_answer(2, r#""result":"pong""#, &what);
        }
    }
}

/// Each callback kind that a request handler makes: the signal while the
/// first callback waits answers the request at once.
#[test]
fn api_async_signal_answers_each_callback_kind() {
    let kinds = [
        ("readFile", Signal::TERM, PANIC_CANCELED),
        ("fileExists", Signal::INT, PANIC_CANCELED),
        ("getAccessibleEntries", Signal::INT, PANIC_CANCELED),
        ("realpath", Signal::INT, PANIC_CANCELED),
        (
            "resolveModuleName",
            Signal::INT,
            r#""message":"resolveModuleName callback failed: context canceled""#,
        ),
    ];
    for (kind, signal, answer) in kinds {
        let what = format!("--api --async, {kind} waits, {signal:?}");
        let dir = project("cbkind");
        let (mut tsgo, id, method) = if kind == "resolveModuleName" {
            let mut tsgo = api_async(&dir, &[]);
            let resolver = tsgo.module_resolver();
            tsgo.send_json(&resolve_module_name(6, resolver, &dir.0));
            (tsgo, 6, "resolveMod")
        } else {
            let mut tsgo = api_async(&dir, &["--callbacks", kind]);
            tsgo.send_json(&create_snapshot(1, &dir.0));
            (tsgo, 1, kind)
        };
        let call = tsgo.wait_callback(method);
        tsgo.signal(signal);
        tsgo.expect_answer(id, answer, &what);
        tsgo.expect_alive(&what);
        tsgo.reply(&call);
        tsgo.expect_end(Instant::now(), &what, 0, "");
    }
}

/// A handler goes on after the signal: its later callbacks write their
/// requests and fail with `context canceled`, and it answers at once.
#[test]
fn api_async_callbacks_after_a_signal_write_their_requests() {
    // cleanBuild deletes the files one by one (removeFile).
    let what = "--api --async, cleanBuild, removeFile waits, SIGINT";
    let dir = project("cbremove");
    let mut tsgo = api_async(&dir, &["--callbacks", "removeFile"]);
    let orchestrator = tsgo.build_orchestrator(&dir.0);
    tsgo.send_json(&build(3, "build", orchestrator));
    tsgo.expect_answer(3, r#""result""#, what);
    tsgo.send_json(&build(4, "cleanBuild", orchestrator));
    let call = tsgo.wait_callback("removeFile");
    tsgo.signal(Signal::INT);
    tsgo.expect_answer(4, r#""result""#, what);
    assert!(
        tsgo.callbacks("removeFile") >= 2,
        "{what}: {:?}",
        tsgo.messages()
    );
    tsgo.expect_alive(what);
    tsgo.reply(&call);
    tsgo.expect_end(Instant::now(), what, 0, "");

    // build emits both files (writeFile). PORT: the port can write a
    // file's request twice after a write error, so only the files count.
    let what = "--api --async, build, writeFile waits, SIGINT";
    let dir = project("cbwrite");
    let mut tsgo = api_async(&dir, &["--callbacks", "writeFile"]);
    let orchestrator = tsgo.build_orchestrator(&dir.0);
    tsgo.send_json(&build(3, "build", orchestrator));
    let call = tsgo.wait_callback("writeFile");
    tsgo.signal(Signal::INT);
    let answer = tsgo.expect_answer(3, r#""result""#, what);
    for file in ["a.js", "b.js"] {
        let text = format!("{file}': context canceled.");
        assert!(answer.contains(&text), "{what}: no {text:?} in {answer}");
        let request = format!(
            r#""method":"writeFile","params":{{"path":"{}"#,
            dir.path(&format!("out/src/{file}"))
        );
        assert!(
            tsgo.messages().iter().any(|m| m.contains(&request)),
            "{what}: no writeFile of {file}: {:?}",
            tsgo.messages()
        );
    }
    tsgo.expect_alive(what);
    tsgo.reply(&call);
    tsgo.expect_end(Instant::now(), what, 0, "");
}

/// A callback that a worker goroutine makes (directoryExists, or readFile
/// of a source file) panics at the signal, and the process ends with
/// exit code 2 at once.
#[test]
fn api_async_worker_callback_ends_the_process_at_the_signal() {
    for kind in ["directoryExists", "readFile"] {
        let what = format!("--api --async, a worker's {kind} waits, SIGINT");
        let dir = project("cbworker");
        let mut tsgo = api_async(&dir, &["--callbacks", kind]);
        tsgo.send_json(&create_snapshot(1, &dir.0));
        let call = tsgo.wait_callback(kind);
        if kind == "readFile" {
            // The first readFile (tsconfig.json) is the handler's own.
            tsgo.reply(&call);
            tsgo.wait_callback_number(kind, 2);
        }
        let start = Instant::now();
        tsgo.signal(Signal::INT);
        tsgo.expect_end_with_stderr(start, &what, 2, REPANICKED_CANCELED);
    }
}

/// A signal while a callback waits, then the end of stdin: the request
/// answered at the signal, and the run ends with exit code 0.
#[test]
fn api_async_signal_then_end_of_stdin() {
    let what = "--api --async, readFile waits, SIGINT, then EOF";
    let dir = project("cbeof");
    let mut tsgo = api_async(&dir, &["--callbacks", "readFile"]);
    tsgo.send_json(&create_snapshot(1, &dir.0));
    tsgo.wait_callback("readFile");
    tsgo.signal(Signal::INT);
    tsgo.expect_answer(1, PANIC_CANCELED, what);
    tsgo.expect_alive(what);
    let start = Instant::now();
    tsgo.close_stdin();
    tsgo.expect_end(start, what, 0, "");
}

/// A signal while tsgo waits for a message, then a request that makes
/// callbacks: the request is the 1 message read after the signal, and it
/// runs in Phase B, so its callbacks write nothing and fail with
/// `ipc: connection closed` and `context canceled`.
#[test]
fn api_async_request_after_an_idle_signal_makes_no_callback() {
    let closed = r"ipc: connection closed\ncontext canceled";
    let cases: [(&str, &str); 5] = [
        ("readFile", "createSnapshot"),
        ("writeFile", "build"),
        ("removeFile", "cleanBuild"),
        ("resolveMod", "resolveModuleName"),
        ("directoryExists", "createSnapshot"),
    ];
    for (method, request) in cases {
        let what = format!("--api --async, SIGINT while idle, then {request} ({method})");
        let dir = project("idle");
        let callbacks = match method {
            "resolveMod" => &[][..],
            _ => &["--callbacks", method][..],
        };
        let mut tsgo = api_async(&dir, callbacks);
        let next = match request {
            "createSnapshot" => create_snapshot(4, &dir.0),
            "resolveModuleName" => resolve_module_name(4, tsgo.module_resolver(), &dir.0),
            _ => {
                let orchestrator = tsgo.build_orchestrator(&dir.0);
                if request == "cleanBuild" {
                    tsgo.send_json(&build(3, "build", orchestrator));
                    tsgo.expect_answer(3, r#""result""#, &what);
                }
                build(4, request, orchestrator)
            }
        };
        tsgo.send_json(r#"{"jsonrpc":"2.0","id":0,"method":"ping"}"#);
        tsgo.expect_answer(0, r#""result":"pong""#, &what);
        tsgo.signal(Signal::INT);
        tsgo.expect_alive(&what);
        let start = Instant::now();
        tsgo.send_json(&next);
        if method == "directoryExists" {
            let stderr =
                "panic: ipc: connection closed\n\tcontext canceled [recovered, repanicked]\n";
            tsgo.expect_end_with_stderr(start, &what, 2, stderr);
        } else {
            tsgo.expect_end(start, &what, 0, "");
            let answer = match method {
                "readFile" => PANIC_CLOSED.to_string(),
                "writeFile" => format!(r"a.js': {closed}."),
                "removeFile" => r#""result""#.to_string(),
                _ => format!(r#""message":"resolveModuleName callback failed: {closed}""#),
            };
            tsgo.expect_answer(4, &answer, &what);
        }
        assert_eq!(tsgo.callbacks(method), 0, "{what}: {:?}", tsgo.messages());
    }
}

/// A signal while a callback waits, then a second createSnapshot: the
/// first request answers at the signal, and the second runs in Phase B.
#[test]
fn api_async_request_read_after_the_signal_makes_no_callback() {
    let what = "--api --async, readFile waits, SIGINT, then createSnapshot";
    let dir = project("cbnext");
    let mut tsgo = api_async(&dir, &["--callbacks", "readFile"]);
    tsgo.send_json(&create_snapshot(1, &dir.0));
    tsgo.wait_callback("readFile");
    tsgo.signal(Signal::INT);
    tsgo.expect_answer(1, PANIC_CANCELED, what);
    let start = Instant::now();
    tsgo.send_json(&create_snapshot(3, &dir.0));
    tsgo.expect_end(start, what, 0, "");
    tsgo.expect_answer(3, PANIC_CLOSED, what);
    assert_eq!(
        tsgo.callbacks("readFile"),
        1,
        "{what}: {:?}",
        tsgo.messages()
    );
}

/// A signal during a request that makes no callback (it reads a FIFO):
/// the request answers, tsgo reads 1 more message and ends. A ping gets
/// its pong; a build runs in Phase B and writes no file.
#[test]
fn api_async_signal_during_a_request_reads_one_more_message() {
    for next in ["ping", "build"] {
        let what = format!("--api --async, SIGINT during a request, then {next}");
        let dir = project("fifo");
        let mut tsgo = api_async(&dir, &["--callbacks", "writeFile"]);
        let orchestrator = tsgo.build_orchestrator(&dir.0);
        dir.write("fifo/src/x.ts", "export const x = 1;\n");
        let config = dir.fifo("fifo/tsconfig.json");
        tsgo.send_json(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"createSnapshot","params":{{"openProjects":["{}"]}}}}"#,
            config.display()
        ));
        let mut writer = open_fifo_writer(&config);
        tsgo.signal(Signal::INT);
        writer
            .write_all(br#"{"compilerOptions":{"strict":true},"include":["src"]}"#)
            .unwrap();
        drop(writer);
        tsgo.expect_answer(2, r#""result""#, &what);
        tsgo.expect_alive(&what);
        let start = Instant::now();
        if next == "ping" {
            tsgo.send_json(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
            tsgo.expect_end(start, &what, 0, "");
            tsgo.expect_answer(3, r#""result":"pong""#, &what);
        } else {
            tsgo.send_json(&build(3, "build", orchestrator));
            tsgo.expect_end(start, &what, 0, "");
            let answer = r"a.js': ipc: connection closed\ncontext canceled.";
            tsgo.expect_answer(3, answer, &what);
            assert_eq!(
                tsgo.callbacks("writeFile"),
                0,
                "{what}: {:?}",
                tsgo.messages()
            );
        }
    }
}

/// After a signal, tsgo reads exactly 1 more message: of two pings in one
/// write, only the first gets its pong.
#[test]
fn api_async_reads_one_message_after_a_signal() {
    let what = "--api --async, SIGINT while idle, then two pings";
    let dir = project("twopings");
    let mut tsgo = api_async(&dir, &[]);
    tsgo.send_json(r#"{"jsonrpc":"2.0","id":0,"method":"ping"}"#);
    tsgo.expect_answer(0, r#""result":"pong""#, what);
    tsgo.signal(Signal::INT);
    tsgo.expect_alive(what);
    let start = Instant::now();
    let mut both = frame(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#);
    both.extend(frame(r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#));
    tsgo.send(&both);
    tsgo.expect_end(start, what, 0, "");
    tsgo.expect_answer(7, r#""result":"pong""#, what);
    assert!(tsgo.answer(8).is_none(), "{what}: {:?}", tsgo.messages());
}

/// Two createSnapshot requests that both wait for a callback, the second
/// sent while the first waits or with it in one write: the signal answers
/// both at once.
#[test]
fn api_async_signal_answers_every_waiting_request() {
    for pipelined in [false, true] {
        let what = format!("--api --async, two createSnapshot (pipelined {pipelined}), SIGINT");
        let dir = project("twosnap");
        let mut tsgo = api_async(&dir, &["--callbacks", "readFile"]);
        if pipelined {
            let mut both = frame(&create_snapshot(1, &dir.0));
            both.extend(frame(&create_snapshot(3, &dir.0)));
            tsgo.send(&both);
        } else {
            tsgo.send_json(&create_snapshot(1, &dir.0));
            tsgo.wait_callback("readFile");
            tsgo.send_json(&create_snapshot(3, &dir.0));
        }
        let second = tsgo.wait_callback_number("readFile", 2);
        tsgo.signal(Signal::INT);
        tsgo.expect_answer(1, PANIC_CANCELED, &what);
        tsgo.expect_answer(3, PANIC_CANCELED, &what);
        tsgo.expect_alive(&what);
        tsgo.reply(&second);
        tsgo.expect_end(Instant::now(), &what, 0, "");
    }
}

/// A second signal while tsgo waits for the next message does nothing.
#[test]
fn api_async_second_signal_does_nothing() {
    let what = "--api --async, readFile waits, SIGINT, SIGTERM, then a ping";
    let dir = project("twosig");
    let mut tsgo = api_async(&dir, &["--callbacks", "readFile"]);
    tsgo.send_json(&create_snapshot(1, &dir.0));
    tsgo.wait_callback("readFile");
    tsgo.signal(Signal::INT);
    tsgo.expect_answer(1, PANIC_CANCELED, what);
    tsgo.signal(Signal::TERM);
    tsgo.expect_alive(what);
    let start = Instant::now();
    tsgo.send_json(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    tsgo.expect_end(start, what, 0, "");
    tsgo.expect_answer(2, r#""result":"pong""#, what);
}

/// The sync API does not wake a waiting callback at a signal: the reply
/// is its result, and the next callback (readFile of a source file, on a
/// worker goroutine) fails with `context canceled` and ends the process.
#[test]
fn api_sync_signal_while_a_callback_waits() {
    let what = "--api, readFile waits, SIGINT, then the reply";
    let dir = project("synccb");
    let cwd = dir.0.to_str().unwrap();
    let mut tsgo = Tsgo::start(&["--api", "--callbacks", "readFile", "--cwd", cwd], &dir.0);
    let params = format!(r#"{{"openProjects":["{}"]}}"#, dir.path("tsconfig.json"));
    tsgo.send(&msgpack_request("createSnapshot", &params));
    tsgo.wait_stdout("readFile");
    tsgo.signal(Signal::INT);
    tsgo.expect_alive(what);
    let start = Instant::now();
    // [CallResponse, "readFile", null]
    tsgo.send(b"\x93\x02\xc4\x08readFile\xc4\x04null");
    tsgo.expect_end_with_stderr(start, what, 2, REPANICKED_CANCELED);
}

/// A two-file project (src/a.ts imports src/b.ts) that emits to out/.
fn project(prefix: &str) -> TempDir {
    let dir = TempDir::new(prefix);
    dir.write(
        "tsconfig.json",
        r#"{"compilerOptions":{"strict":true,"outDir":"out"},"include":["src"]}"#,
    );
    dir.write(
        "src/a.ts",
        "import { b } from \"./b\";\nexport const a: number = b;\n",
    );
    dir.write("src/b.ts", "export const b: number = 1;\n");
    dir
}

/// Starts `tsgo --api --async` in `dir` with `args`.
fn api_async(dir: &TempDir, args: &[&str]) -> Tsgo {
    let cwd = dir.0.to_str().unwrap();
    let mut all = vec!["--api", "--async", "--cwd", cwd];
    all.extend_from_slice(args);
    Tsgo::start(&all, &dir.0)
}

fn create_snapshot(id: u32, dir: &Path) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"createSnapshot","params":{{"openProjects":["{}/tsconfig.json"]}}}}"#,
        dir.display()
    )
}

/// A build request (`build` or `cleanBuild`) of a build orchestrator.
fn build(id: u32, method: &str, orchestrator: u32) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{"buildOrchestratorID":{orchestrator}}}}}"#
    )
}

/// resolveModuleName of `./b` from src/, with the module resolver
/// `resolver`; it calls the client back with `resolveMod`.
fn resolve_module_name(id: u32, resolver: u32, dir: &Path) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"resolveModuleName","params":{{"resolver":{resolver},"moduleName":"./b","containingDirectory":"{}/src"}}}}"#,
        dir.display()
    )
}

/// One JSON-RPC message with its `Content-Length` header.
fn frame(body: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// A tsgo child with piped stdio. Its stdout and stderr are read on
/// threads into buffers. Drop kills a tsgo that still runs.
struct Tsgo {
    child: Child,
    /// None after `close_stdin`.
    stdin: Option<ChildStdin>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    status: Option<ExitStatus>,
    /// The threads that read stdout and stderr. They end at the end of the
    /// pipes.
    readers: Vec<JoinHandle<()>>,
}

impl Tsgo {
    fn start(args: &[&str], dir: &Path) -> Tsgo {
        let bin = std::env::var_os("TSGO_STDIO_END_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_tsgo").into());
        let mut child = Command::new(bin)
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
            stdin: Some(stdin),
            stdout,
            stderr,
            status: None,
            readers,
        }
    }

    /// Checks that tsgo exits within `AT_ONCE` of `since` with `code` and
    /// `stderr`. `what` names the run in a failure.
    fn expect_end(&mut self, since: Instant, what: &str, code: i32, stderr: &str) {
        self.expect_exit(since, what, code);
        assert_eq!(self.stderr(), stderr, "{what}");
    }

    /// `expect_end` for a Go panic: stderr starts with `first` (the
    /// stacks differ).
    fn expect_end_with_stderr(&mut self, since: Instant, what: &str, code: i32, first: &str) {
        self.expect_exit(since, what, code);
        let stderr = self.stderr();
        assert!(stderr.starts_with(first), "{what}: stderr {stderr:?}");
    }

    fn expect_exit(&mut self, since: Instant, what: &str, code: i32) {
        let (status, exited) = self
            .wait_exit(LIMIT)
            .unwrap_or_else(|| panic!("{what}: tsgo did not end; stdout {:?}", self.messages()));
        assert!(
            exited - since < AT_ONCE,
            "{what}: tsgo took {:?} to end",
            exited - since
        );
        assert_eq!(
            status.code(),
            Some(code),
            "{what}: {status:?}, stderr {:?}",
            self.stderr()
        );
    }

    /// Checks that tsgo still runs `ALIVE` from now.
    fn expect_alive(&mut self, what: &str) {
        if let Some((status, _)) = self.wait_exit(ALIVE) {
            panic!(
                "{what}: tsgo ended ({status:?}) before the next message; stderr {:?}",
                self.stderr()
            );
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Sends one JSON-RPC message (`--api --async`).
    fn send_json(&mut self, body: &str) {
        self.send(&frame(body));
    }

    /// The JSON-RPC messages on stdout so far.
    fn messages(&self) -> Vec<String> {
        let out = self.stdout.lock().unwrap().clone();
        let mut messages = Vec::new();
        let mut rest = &out[..];
        while let Some(end) = rest.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&rest[..end]);
            let length: usize = header
                .trim()
                .strip_prefix("Content-Length: ")
                .and_then(|length| length.parse().ok())
                .unwrap_or_else(|| panic!("no Content-Length in {header:?}"));
            let body = &rest[end + 4..];
            if body.len() < length {
                break;
            }
            messages.push(String::from_utf8_lossy(&body[..length]).into_owned());
            rest = &body[length..];
        }
        messages
    }

    /// Waits up to `limit` for a message that `pick` accepts.
    fn wait_message(&self, limit: Duration, pick: impl Fn(&str) -> bool) -> Option<String> {
        let end = Instant::now() + limit;
        loop {
            if let Some(found) = self.messages().into_iter().find(|m| pick(m)) {
                return Some(found);
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The answer to request `id`, if it came.
    fn answer(&self, id: u32) -> Option<String> {
        let prefix = format!(r#"{{"jsonrpc":"2.0","id":{id},"#);
        self.messages().into_iter().find(|m| m.starts_with(&prefix))
    }

    /// Waits `AT_ONCE` for the answer to request `id` and checks that it
    /// holds `text`.
    fn expect_answer(&self, id: u32, text: &str, what: &str) -> String {
        let prefix = format!(r#"{{"jsonrpc":"2.0","id":{id},"#);
        let answer = self
            .wait_message(AT_ONCE, |m| m.starts_with(&prefix))
            .unwrap_or_else(|| panic!("{what}: no answer to {id}: {:?}", self.messages()));
        assert!(
            answer.contains(text),
            "{what}: answer {id} has no {text:?}: {answer}"
        );
        answer
    }

    /// The callback requests of `method` so far.
    fn callbacks(&self, method: &str) -> usize {
        let text = format!(r#"","method":"{method}""#);
        self.messages().iter().filter(|m| m.contains(&text)).count()
    }

    /// Waits for the first callback request of `method` and returns its id.
    fn wait_callback(&self, method: &str) -> String {
        self.wait_callback_number(method, 1)
    }

    /// Waits for callback request `n` (from 1) of `method` and returns its
    /// id.
    fn wait_callback_number(&self, method: &str, n: usize) -> String {
        let text = format!(r#"","method":"{method}""#);
        let end = Instant::now() + LIMIT;
        loop {
            let calls: Vec<String> = self
                .messages()
                .into_iter()
                .filter(|m| m.contains(&text))
                .collect();
            if let Some(call) = calls.get(n - 1) {
                let id = call
                    .strip_prefix(r#"{"jsonrpc":"2.0","id":""#)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or_else(|| panic!("no callback id in {call}"));
                return id.to_string();
            }
            assert!(
                Instant::now() < end,
                "no callback {n} of {method}: {:?}; stderr {:?}",
                self.messages(),
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Answers the callback request `id` with null (the real file system).
    fn reply(&mut self, id: &str) {
        self.send_json(&format!(r#"{{"jsonrpc":"2.0","id":"{id}","result":null}}"#));
    }

    /// Makes build orchestrator 1 for the project in `dir`.
    fn build_orchestrator(&mut self, dir: &Path) -> u32 {
        self.send_json(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"createBuildOrchestrator","params":{{"rootNames":["{0}/tsconfig.json"],"cwd":"{0}"}}}}"#,
            dir.display()
        ));
        self.expect_answer(
            1,
            r#""result":{"buildOrchestratorID":1}"#,
            "createBuildOrchestrator",
        );
        1
    }

    /// Makes module resolver 1, which calls the client back with
    /// `resolveMod`.
    fn module_resolver(&mut self) -> u32 {
        self.send_json(
            r#"{"jsonrpc":"2.0","id":5,"method":"createModuleResolver","params":{"compilerOptions":{},"resolveModuleNameCallback":"resolveMod"}}"#,
        );
        self.expect_answer(5, r#""result":1"#, "createModuleResolver");
        1
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

    fn path(&self, rel: &str) -> String {
        self.0.join(rel).display().to_string()
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
