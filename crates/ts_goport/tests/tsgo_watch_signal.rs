//! How `tsgo -b -w` ends on SIGINT and SIGTERM, and how `-w` and `-b -w`
//! end on a signal during a rebuild cycle, as Go's tsgo ends them. Go's
//! watch loop (execute/watchmanager/watchmanager.go:354 `RunLoop`) selects
//! on `ctx.Done()` and the cycle channel:
//!
//! - A signal while the loop waits ends the run at once with exit code 0.
//!   `tsgo_stdio_end.rs` has the `-w` case.
//! - A signal during a cycle ends the run after that cycle. When a change
//!   came during the cycle, both cases are ready, and Go picks one at
//!   random: it can run one more cycle first. The tests accept both orders
//!   and use only loose time limits.
//!
//! A cycle that the test holds open reads `src/a.ts` from a FIFO: the read
//! waits until the test writes the text. `TS_WATCH_DEBUG=1` makes tsgo
//! print each watch event, so the test knows when a change has reached the
//! watch loop.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process};

/// The longest wait for a step that ends in milliseconds when it works.
const LIMIT: Duration = Duration::from_secs(20);

/// The longest time from a signal to the exit of a watch loop that waits.
/// tsgo ends in milliseconds; the rest is room for a loaded host.
const AT_ONCE: Duration = Duration::from_secs(2);

/// The text that the test writes to the FIFO of a held cycle.
const NEW_TEXT: &[u8] = b"export const a: number = 2;\n";

/// The line that each build of a watch run ends with.
const WATCHING: &str = "Watching for file changes";

/// The line that each rebuild cycle starts with.
const CYCLE: &str = "File change detected";

#[test]
fn build_watch_ends_soon_after_sigint_or_sigterm() {
    for signal in [Signal::INT, Signal::TERM] {
        let dir = Project::new("bw");
        let mut tsgo = Tsgo::start(&["-b", "-w"], &dir.proj);
        tsgo.wait_stdout(WATCHING);
        let start = Instant::now();
        tsgo.signal(signal);
        let (status, exited) = tsgo
            .wait_exit(LIMIT)
            .unwrap_or_else(|| panic!("{signal:?}: tsgo -b -w did not end"));
        assert!(
            exited - start < AT_ONCE,
            "{signal:?}: tsgo -b -w took {:?} to end",
            exited - start
        );
        assert_eq!(status.code(), Some(0), "{signal:?}: {status:?}");
        assert_eq!(tsgo.stderr(), "", "{signal:?}");
    }
}

/// A signal comes while a cycle reads `src/a.ts`, and a change of
/// `src/b.ts` reaches the watch loop before the cycle ends. The run ends
/// with exit code 0 after that cycle, or after one more (Go's random
/// select), and each cycle ends with its watch line.
#[test]
fn watch_ends_after_the_cycle_that_a_signal_interrupts() {
    let modes: [(&[&str], Signal); 2] = [
        (&["-w", "-p", "tsconfig.json"], Signal::INT),
        (&["-b", "-w"], Signal::TERM),
    ];
    for (args, signal) in modes {
        let what = format!("{args:?} {signal:?}");
        let dir = Project::new("cycle");
        let mut tsgo = Tsgo::start(args, &dir.proj);
        tsgo.wait_stdout(WATCHING);
        tsgo.wait_until_src_is_watched(&dir);

        let fifo = dir.hold_a_ts();
        let reader = fifo.wait_reader(&what);
        let at_read = tsgo.stdout_len();
        tsgo.signal(signal);
        dir.write("src/b.ts", "export const b: number = 2;\n");
        tsgo.wait_stdout_after(at_read, "b.ts");
        fifo.feed(reader, &what);

        // Go can run one more cycle: it reads `src/a.ts` again if the
        // cycle needs it.
        let end = Instant::now() + LIMIT;
        let status = loop {
            if let Some((status, _)) = tsgo.wait_exit(Duration::from_millis(2)) {
                break status;
            }
            assert!(Instant::now() < end, "{what}: tsgo did not end");
            if let Some(reader) = fifo.reader() {
                fifo.feed(reader, &what);
            }
        };
        assert_eq!(status.code(), Some(0), "{what}: {status:?}");
        assert_eq!(tsgo.stderr(), "", "{what}");
        // The held cycle and each later one end with the watch line.
        let after = tsgo.stdout_from(at_read);
        let last = after.rfind(CYCLE).map_or(0, |at| at + CYCLE.len());
        assert!(
            after[last..].contains(WATCHING),
            "{what}: a cycle did not end: {after:?}"
        );
    }
}

/// A watch project in a directory deep enough to watch (Go
/// `CanWatchDirectory` skips the first 3 components): `src/a.ts` and
/// `src/b.ts` are the files of `tsconfig.json`.
struct Project {
    /// The temporary directory that holds the project.
    root: PathBuf,
    /// The project directory.
    proj: PathBuf,
}

static NEXT: AtomicU32 = AtomicU32::new(0);

impl Project {
    fn new(prefix: &str) -> Project {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "tsgo_watch_signal-{}-{prefix}{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("x").join("proj");
        let project = Project { root, proj };
        project.write(
            "tsconfig.json",
            r#"{"compilerOptions":{"strict":true,"outDir":"out","rootDir":"src"},"files":["src/a.ts","src/b.ts"]}"#,
        );
        project.write("src/a.ts", "export const a: number = 1;\n");
        project.write("src/b.ts", "export const b: number = 1;\n");
        project
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.proj.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Puts a FIFO in the place of `src/a.ts` with one rename, so the
    /// watch sees one change, and the next read of the file waits.
    fn hold_a_ts(&self) -> Fifo {
        let staged = self.root.join("x").join("a.fifo");
        rustix::fs::mkfifoat(rustix::fs::CWD, &staged, Mode::RUSR | Mode::WUSR).unwrap();
        let path = self.proj.join("src/a.ts");
        std::fs::rename(&staged, &path).unwrap();
        Fifo(path)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The FIFO at `src/a.ts`.
struct Fifo(PathBuf);

impl Fifo {
    /// A writer end when a reader has the FIFO open. Without one, the
    /// non-blocking open fails with ENXIO.
    fn reader(&self) -> Option<std::fs::File> {
        match rustix::fs::open(&self.0, OFlags::WRONLY | OFlags::NONBLOCK, Mode::empty()) {
            Ok(fd) => {
                rustix::fs::fcntl_setfl(&fd, OFlags::empty()).unwrap();
                Some(std::fs::File::from(fd))
            }
            Err(Errno::NXIO) => None,
            Err(err) => panic!("open {}: {err}", self.0.display()),
        }
    }

    /// Waits until tsgo opens the FIFO to read it.
    fn wait_reader(&self, what: &str) -> std::fs::File {
        let end = Instant::now() + LIMIT;
        loop {
            if let Some(writer) = self.reader() {
                return writer;
            }
            assert!(Instant::now() < end, "{what}: tsgo did not read src/a.ts");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Writes the new text to the reader and closes, then waits until the
    /// reader has closed its end too, so a later open is a new read. A
    /// reader that went away first gives EPIPE.
    fn feed(&self, mut writer: std::fs::File, what: &str) {
        match writer.write_all(NEW_TEXT) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(err) => panic!("{what}: write {}: {err}", self.0.display()),
        }
        drop(writer);
        let end = Instant::now() + LIMIT;
        while self.reader().is_some() {
            assert!(Instant::now() < end, "{what}: tsgo did not close src/a.ts");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A tsgo child with `TS_WATCH_DEBUG=1` and piped stdio. Its stdout and
/// stderr are read on threads into buffers. Drop kills a tsgo that still
/// runs.
struct Tsgo {
    child: Child,
    /// Held open: the watch runs end on a signal, not at the end of stdin.
    _stdin: ChildStdin,
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
            .env("TS_WATCH_DEBUG", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let readers = vec![
            read_into(child.stdout.take().unwrap(), stdout.clone()),
            read_into(child.stderr.take().unwrap(), stderr.clone()),
        ];
        Tsgo {
            child,
            _stdin: stdin,
            stdout,
            stderr,
            status: None,
            readers,
        }
    }

    fn signal(&self, signal: Signal) {
        let pid = Pid::from_raw(self.child.id().cast_signed()).unwrap();
        kill_process(pid, signal).unwrap();
    }

    fn wait_stdout(&self, text: &str) {
        self.wait_stdout_after(0, text);
    }

    /// Waits until `text` is on stdout after its first `start` bytes.
    fn wait_stdout_after(&self, start: usize, text: &str) {
        let end = Instant::now() + LIMIT;
        while !self.stdout_from(start).contains(text) {
            assert!(
                Instant::now() < end,
                "no {text:?} on stdout: {:?}; stderr: {:?}",
                self.stdout(),
                self.stderr()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Writes `src/ready-<n>.txt` until tsgo prints its watch event: the
    /// watch of `src` works. `-b -w` starts its watches after the first
    /// watch line.
    fn wait_until_src_is_watched(&self, dir: &Project) {
        let end = Instant::now() + LIMIT;
        for n in 0.. {
            let name = format!("ready-{n}.txt");
            dir.write(&format!("src/{name}"), "");
            let wait = Instant::now() + Duration::from_millis(500);
            while Instant::now() < wait {
                if self.stdout().contains(&name) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(
                Instant::now() < end,
                "no watch event for src: {:?}",
                self.stdout()
            );
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

    fn stdout(&self) -> String {
        self.stdout_from(0)
    }

    /// Stdout after its first `start` bytes.
    fn stdout_from(&self, start: usize) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap()[start..]).into_owned()
    }

    fn stdout_len(&self) -> usize {
        self.stdout.lock().unwrap().len()
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

/// Reads `pipe` into `buf` on a new thread until the end of the pipe.
fn read_into(mut pipe: impl Read + Send + 'static, buf: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = vec![0; 65536];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            buf.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    })
}
