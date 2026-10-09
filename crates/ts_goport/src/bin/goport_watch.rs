//! `goport_watch`: runs Go `tsc --watch` through several builds, to check
//! that each build makes a new program version.
//!
//! ```text
//! goport_watch <out-file> <changed-file> <edit>... -- <tsc args>...
//! ```
//!
//! It runs `execute_tsc::command_line` (Go `execute.CommandLine`) with the
//! tsc arguments in the current directory. The watch backend records the
//! watches (`watcher::set_test_watch_backend`) instead of watching the OS
//! file system. After the first build, for each edit: it copies the edit
//! file over its target, sends an update event for the target to the
//! watches that cover it, and waits for the build that the event starts.
//! An edit is `<edit-file>`, whose target is `changed-file`, or
//! `<target>=<edit-file>`. Then it stops the watch, writes the tsc output to
//! `out-file`, and gives each target its original text back. Last, after
//! the released programs are freed, it prints
//! `file_versions made=<n> dead=<m>` (`ast::file_versions_made`,
//! `ast::dead_file_versions`).
//!
//! Exit 0 when every build ran, 1 when a build did not end in time, and 70
//! (`EXIT_UNPORTED`) when unported code ran or the run panicked.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ts_goport::execute::execute_tsc::{GoTsc, command_line};
use ts_goport::execute::tsc::{EXIT_UNPORTED, System, Writer, new_os_system};
use ts_goport::execute::watcher::set_test_watch_backend;
use ts_goport::execute::watchmanager::{WatchBackend, WatchDirectoryRequest};
use ts_goport::fswatch::{Event, EventKind, Watch, WatchCallback};
use ts_goport::gostd::context;
use ts_goport::gostd::errors::GoError;
use ts_goport::prelude::*;

/// The text that ends each watch build.
const BUILD_END: &str = "Watching for file changes";

/// How long one build may take.
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);

/// The wait between the end of a build and the next edit.
const EDIT_DELAY: Duration = Duration::from_millis(300);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(split) = args.iter().position(|arg| arg == "--") else {
        usage();
    };
    let (ours, tsc_args) = (&args[..split], args[split + 1..].to_vec());
    if ours.len() < 3 {
        usage();
    }
    let out_file = ours[0].clone();
    let changed = absolute(&ours[1]);
    let edits: Vec<Edit> = ours[2..]
        .iter()
        .map(|edit| match edit.split_once('=') {
            Some((target, file)) => Edit {
                target: absolute(target),
                text: read(file),
            },
            None => Edit {
                target: changed.clone(),
                text: read(edit),
            },
        })
        .collect();

    let mut originals: Vec<(String, Vec<u8>)> = Vec::new();
    for target in std::iter::once(&changed).chain(edits.iter().map(|edit| &edit.target)) {
        if !originals.iter().any(|(path, _)| path == target) {
            originals.push((target.clone(), read(target)));
        }
    }
    let output = Arc::new(Mutex::new(Vec::new()));
    let watches = Arc::new(Watches::default());
    let work = {
        let (output, watches) = (output.clone(), watches);
        std::thread::Builder::new()
            .stack_size(ts_goport::gostd::stack::max_stack_size())
            .spawn(move || run(&tsc_args, output, watches, edits))
            .expect("start the work thread")
    };
    let result = work.join();
    for (path, original) in &originals {
        std::fs::write(path, original).unwrap_or_else(|e| panic!("restore {path}: {e}"));
    }
    std::fs::write(&out_file, &*output.lock().expect("output"))
        .unwrap_or_else(|e| panic!("write {out_file}: {e}"));
    // The checkers of the released programs end in the background; the
    // tables and file versions that only they hold go with them.
    ts_goport::program::wait_for_background_releases();
    println!(
        "file_versions made={} dead={}",
        ts_goport::ast::file_versions_made(),
        ts_goport::ast::dead_file_versions()
    );
    let code = match result {
        Ok(true) if unported_report().is_empty() => 0,
        Ok(false) if unported_report().is_empty() => 1,
        _ => {
            for (name, count) in unported_report() {
                eprintln!("unported: {name} {count}");
            }
            EXIT_UNPORTED
        }
    };
    std::process::exit(code);
}

fn usage() -> ! {
    eprintln!(
        "usage: goport_watch <out-file> <changed-file> <[target=]edit-file>... -- <tsc args>..."
    );
    std::process::exit(2);
}

/// One edit: the new text of `target` (an absolute path).
struct Edit {
    target: String,
    text: Vec<u8>,
}

/// Runs tsc on this thread and the edits on another. Returns false when a
/// build did not end in time.
fn run(
    tsc_args: &[String],
    output: Arc<Mutex<Vec<u8>>>,
    watches: Arc<Watches>,
    edits: Vec<Edit>,
) -> bool {
    set_test_watch_backend(Rc::new(TestBackend {
        watches: watches.clone(),
    }));
    let writer: Writer = Rc::new(RefCell::new(SharedOutput(output.clone())));
    let sys = new_os_system()
        .unwrap_or_else(|status| panic!("no system: {status:?}"))
        .with_writer(writer);
    let (ctx, cancel) = context::with_cancel(&context::background());
    let driver = std::thread::spawn(move || {
        let ok = drive(&output, &watches, &edits);
        cancel();
        ok
    });
    let sys: Rc<dyn System> = Rc::new(sys);
    let _ = command_line(&ctx, sys, tsc_args, &GoTsc);
    driver.join().expect("the edit thread")
}

/// Waits for the first build, then makes each edit and waits for its build.
fn drive(output: &Mutex<Vec<u8>>, watches: &Watches, edits: &[Edit]) -> bool {
    if !wait_for_builds(output, 1) {
        return false;
    }
    for (i, Edit { target, text }) in edits.iter().enumerate() {
        // The file system clock is coarse: an edit right after a build can
        // get the mtime of that build's outputs, and the `-b` up-to-date
        // check then sees no change.
        std::thread::sleep(EDIT_DELAY);
        std::fs::write(target, text).unwrap_or_else(|e| panic!("write {target}: {e}"));
        watches.send_update(target);
        if !wait_for_builds(output, i + 2) {
            return false;
        }
    }
    true
}

/// Waits until the output shows `count` finished builds.
fn wait_for_builds(output: &Mutex<Vec<u8>>, count: usize) -> bool {
    let start = Instant::now();
    loop {
        let text = String::from_utf8_lossy(&output.lock().expect("output")).into_owned();
        if text.matches(BUILD_END).count() >= count {
            return true;
        }
        if start.elapsed() > BUILD_TIMEOUT {
            eprintln!("goport_watch: build {count} did not end");
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A tsc writer that another thread can read.
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("output").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The directory watches that the watcher made: id, directory, recursive
/// and callback.
#[derive(Default)]
struct Watches {
    next: AtomicU64,
    live: Mutex<Vec<(u64, String, bool, WatchCallback)>>,
}

impl Watches {
    /// Sends an update event for `path` to each watch that covers it.
    fn send_update(&self, path: &str) {
        let callbacks: Vec<WatchCallback> = self
            .live
            .lock()
            .expect("watches")
            .iter()
            .filter(|(_, dir, recursive, _)| covers(dir, *recursive, path))
            .map(|(_, _, _, callback)| callback.clone())
            .collect();
        for callback in callbacks {
            callback(
                vec![Event {
                    kind: EventKind::Update,
                    path: path.to_string(),
                }],
                None,
            );
        }
    }
}

/// True when a watch of `dir` reports changes of `path`.
fn covers(dir: &str, recursive: bool, path: &str) -> bool {
    let Some(rest) = path
        .strip_prefix(dir)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    recursive || !rest.contains('/')
}

/// Go test watch backend: records each watch instead of watching the OS.
struct TestBackend {
    watches: Arc<Watches>,
}

impl WatchBackend for TestBackend {
    fn watch_directory(
        &self,
        dir: &str,
        fn_: WatchCallback,
        recursive: bool,
        _ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> Result<Box<dyn Watch>, GoError> {
        let id = self.watches.next.fetch_add(1, Ordering::Relaxed);
        self.watches
            .live
            .lock()
            .expect("watches")
            .push((id, dir.to_string(), recursive, fn_));
        Ok(Box::new(TestWatch {
            id,
            watches: self.watches.clone(),
        }))
    }

    /// One `watch_directory` call per request, in request order. A test
    /// watch never fails, so the batch never fails either.
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Box<dyn Watch>>, GoError> {
        requests
            .into_iter()
            .map(|request| {
                self.watch_directory(
                    &request.dir,
                    request.callback,
                    request.recursive,
                    request.ignore,
                )
            })
            .collect()
    }
}

struct TestWatch {
    id: u64,
    watches: Arc<Watches>,
}

impl Watch for TestWatch {
    fn close(&self) -> Result<(), GoError> {
        self.watches
            .live
            .lock()
            .expect("watches")
            .retain(|(id, _, _, _)| *id != self.id);
        Ok(())
    }

    fn unexported(&self) {}
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// `path` as an absolute path with `/` separators.
fn absolute(path: &str) -> String {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("no current directory")
            .join(path)
    };
    path.to_string_lossy().replace('\\', "/")
}
