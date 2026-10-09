//! Go: `internal/lsp/lspwatcher/lspwatcher_test.go`.
//!
//! PORT: the Rust watcher lives on the LSP dispatch thread: its flush timer
//! is `gostd::local::after_func`, so `wait_for` runs `local::run_pending()`
//! on each poll (the libtest thread is the dispatch thread). The Go
//! `fakeBackend` is `FakeBackend`; its callbacks are `LocalWatchCallback`s
//! (`Rc`), and a `fswatch::Watch` must be `Send`, so a closed fake watch
//! queues its directory in `closes` and the backend applies the queue before
//! each use (same order as Go: every call is on the test thread). The Go
//! logger (`logging.NewLogger(os.Stderr)`) is `None`; the tests do not read
//! the log.
//!
//! The two tests on the real backend (`New`) run in a child process of
//! this test binary, so that a fault in the `DefaultWatcherBackend`
//! delivery (bug S2-001, fixed) cannot stop the shared fswatch debounce
//! thread that the fswatch tests of this process use.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use ts_goport::frontend::bundled;
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::{Fs, osvfs_fs};
use ts_goport::fswatch::{self, ERR_OVERFLOW, ERR_WATCH_TERMINATED, Event, EventKind};
use ts_goport::gostd::{GoError, errors, local};
use ts_goport::lsp::lsproto::{
    FileChangeType, FileEvent, FileSystemWatcher, PatternOrRelativePattern, WatchKind,
};
use ts_goport::lsp::lspwatcher::{
    self, LocalWatchCallback, Watcher, WatcherBackend, new_with_backend, root_from_glob, watch_root,
};

use super::fswatch_watcher::{TmpDir, new_tmp_dir};

// Go: lspwatcher_test.go:21 waitFor
fn wait_for(mut cond: impl FnMut() -> bool, msg: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        local::run_pending();
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {msg}");
}

/// Go `t.TempDir()`: the dir guard and its path (not resolved, as Go).
fn temp_dir() -> (TmpDir, String) {
    let (guard, _) = new_tmp_dir();
    let p = guard.path().to_str().unwrap().to_string();
    (guard, p)
}

fn wrapped_os_fs() -> Rc<dyn Fs> {
    bundled::wrap_fs(osvfs_fs())
}

fn all_kinds() -> WatchKind {
    WatchKind(WatchKind::CREATE.0 | WatchKind::CHANGE.0 | WatchKind::DELETE.0)
}

fn fsw(pattern: &str, kind: Option<WatchKind>) -> FileSystemWatcher {
    FileSystemWatcher {
        glob_pattern: PatternOrRelativePattern {
            pattern: Some(pattern.to_string()),
            relative_pattern: None,
        },
        kind,
    }
}

/// Collects every batch that `on_changes` receives.
type Got = Rc<RefCell<Vec<FileEvent>>>;

fn collector() -> (Got, Box<dyn Fn(Vec<FileEvent>)>) {
    let got: Got = Rc::new(RefCell::new(Vec::new()));
    let g = got.clone();
    (got, Box::new(move |changes| g.borrow_mut().extend(changes)))
}

/// Go `filepath.Join`.
fn join(parts: &[&str]) -> String {
    let mut p = std::path::PathBuf::new();
    for part in parts {
        p.push(part);
    }
    p.to_str().unwrap().to_string()
}

fn ev(kind: EventKind, path: &str) -> Event {
    Event {
        kind,
        path: path.to_string(),
    }
}

// Go: lspwatcher_test.go:167 fakeBackend
#[derive(Default)]
struct FakeShared {
    by_dir: RefCell<IndexMap<String, LocalWatchCallback>>,
    closed: RefCell<HashMap<String, i32>>,
    opt_count: RefCell<HashMap<String, usize>>,
    fail_dirs: RefCell<HashMap<String, GoError>>,
    /// Directories whose fake watch was closed and not yet applied.
    closes: Arc<Mutex<Vec<String>>>,
}

impl FakeShared {
    /// Applies the queued closes (Go `closeFn`: `delete(f.byDir, dir)` and
    /// `f.closed[dir]++`).
    fn apply_closes(&self) {
        let closes = std::mem::take(&mut *self.closes.lock().unwrap());
        for dir in closes {
            self.by_dir.borrow_mut().shift_remove(&dir);
            *self.closed.borrow_mut().entry(dir).or_default() += 1;
        }
    }

    // Go: lspwatcher_test.go:203 watchedDirs
    fn watched_dirs(&self) -> Vec<String> {
        self.apply_closes();
        self.by_dir.borrow().keys().cloned().collect()
    }

    // Go: lspwatcher_test.go:213 isWatching
    fn is_watching(&self, dir: &str) -> bool {
        self.apply_closes();
        self.by_dir.borrow().contains_key(dir)
    }

    // Go: lspwatcher_test.go:220 emit
    fn emit(&self, dir: &str, events: Vec<Event>, err: Option<GoError>) {
        self.apply_closes();
        let cb = self.by_dir.borrow().get(dir).cloned();
        if let Some(cb) = cb {
            cb(events, err);
        }
    }

    // Go: lspwatcher_test.go:229 emitAll
    fn emit_all(&self, events: Vec<Event>, err: Option<GoError>) {
        self.apply_closes();
        let cbs: Vec<LocalWatchCallback> = self.by_dir.borrow().values().cloned().collect();
        for cb in cbs {
            cb(events.clone(), err.clone());
        }
    }
}

struct FakeBackend(Rc<FakeShared>);

// Go: lspwatcher_test.go:175 newFakeBackend
fn new_fake_backend() -> (Rc<FakeShared>, Box<dyn WatcherBackend>) {
    let shared = Rc::new(FakeShared::default());
    (shared.clone(), Box::new(FakeBackend(shared)))
}

impl WatcherBackend for FakeBackend {
    // Go: lspwatcher_test.go:184 WatchDirectory
    fn watch_directory(
        &self,
        dir: &str,
        fn_: LocalWatchCallback,
        opts: &[Box<dyn fswatch::WatchOption>],
    ) -> Result<Box<dyn fswatch::Watch>, GoError> {
        let f = &self.0;
        f.apply_closes();
        if let Some(err) = f.fail_dirs.borrow().get(dir) {
            return Err(err.clone());
        }
        f.by_dir.borrow_mut().insert(dir.to_string(), fn_);
        f.opt_count.borrow_mut().insert(dir.to_string(), opts.len());
        Ok(Box::new(FakeWatch {
            dir: dir.to_string(),
            closes: f.closes.clone(),
        }))
    }
}

// Go: lspwatcher_test.go:241 fakeWatch
struct FakeWatch {
    dir: String,
    closes: Arc<Mutex<Vec<String>>>,
}

impl fswatch::Watch for FakeWatch {
    fn close(&self) -> Result<(), GoError> {
        self.closes.lock().unwrap().push(self.dir.clone());
        Ok(())
    }

    fn unexported(&self) {}
}

/// Closes the watcher when dropped (Go `t.Cleanup(w.Close)`).
struct CloseOnDrop(Rc<Watcher>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn has_type(got: &Got, t: FileChangeType) -> bool {
    got.borrow().iter().any(|e| e.type_ == t)
}

// ----- real backend (child process) --------------------------------------

const CHILD_TEST: &str = "units_platform::lspwatcher::real_backend_child";
const CHILD_ENV: &str = "S2_LSPWATCHER_CHILD";

/// Child entry of the real-backend tests. Does nothing unless
/// `S2_LSPWATCHER_CHILD` names a test.
#[test]
fn real_backend_child() {
    let Some(which) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    match which.to_str().unwrap() {
        "CreateChangeDelete" => create_change_delete_body(),
        "MissingThenCreate" => missing_then_create_body(),
        other => panic!("unknown child {other}"),
    }
}

fn run_child(which: &str) {
    let exe = super::self_exe();
    let output = std::process::Command::new(exe)
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads", "1"])
        .env(CHILD_ENV, which)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run lspwatcher child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "lspwatcher child {which} failed ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

// Go: lspwatcher_test.go:33 TestWatcher_CreateChangeDelete (body)
fn create_change_delete_body() {
    let (_guard, dir) = temp_dir();
    let (got, on_changes) = collector();
    let w = lspwatcher::new(wrapped_os_fs(), on_changes, None);
    let _close = CloseOnDrop(w.clone());

    let pattern = format!("{}/**/*", tspath::normalize_slashes(&dir));
    w.watch_files("test", &[fsw(&pattern, Some(all_kinds()))])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    std::thread::sleep(Duration::from_millis(200));

    let file = join(&[&dir, "a.ts"]);
    std::fs::write(&file, "export {}").unwrap();
    wait_for(|| has_type(&got, FileChangeType::CHANGED), "update event");

    std::fs::remove_file(&file).unwrap();
    wait_for(|| has_type(&got, FileChangeType::DELETED), "delete event");

    w.unwatch_files("test")
        .unwrap_or_else(|e| panic!("{}", e.error()));
}

// Go: lspwatcher_test.go:33 TestWatcher_CreateChangeDelete
#[test]
fn test_watcher_create_change_delete() {
    run_child("CreateChangeDelete");
}

// Go: lspwatcher_test.go:100 TestWatcher_KindFilter
#[test]
fn test_watcher_kind_filter() {
    let (_guard, dir) = temp_dir();
    let dir_norm = tspath::normalize_slashes(&dir);
    let (got, on_changes) = collector();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(wrapped_os_fs(), b, on_changes, None);
    let _close = CloseOnDrop(w.clone());

    let pattern = format!("{dir_norm}/**/*");
    w.watch_files("test", &[fsw(&pattern, Some(WatchKind::DELETE))])
        .unwrap_or_else(|e| panic!("{}", e.error()));
    let x = join(&[&dir_norm, "x.ts"]);
    backend.emit_all(
        vec![ev(EventKind::Update, &x), ev(EventKind::Delete, &x)],
        None,
    );

    wait_for(|| has_type(&got, FileChangeType::DELETED), "delete event");
    for e in got.borrow().iter() {
        assert_eq!(
            e.type_,
            FileChangeType::DELETED,
            "unexpected non-delete event: {e:?}"
        );
    }
}

// Go: lspwatcher_test.go:147 TestRootFromGlob
#[test]
fn test_root_from_glob() {
    let cases = [
        ("/abs/path/**/*", "/abs/path"),
        ("/abs/path/", "/abs/path"),
        ("/abs/path/?.ts", "/abs/path"),
        ("/abs/path/{a,b}/*", "/abs/path"),
        // ts#64159
        ("/abs/path/../shared/*", "/abs/shared"),
    ];
    for (pattern, want) in cases {
        assert_eq!(root_from_glob(pattern), want, "rootFromGlob({pattern:?})");
    }
}

// Go: lspwatcher_test.go:170 TestWatchRootFromRelativePattern (ts#64159)
#[test]
fn test_watch_root_from_relative_pattern() {
    use ts_goport::lsp::lsproto::{RelativePattern, URI, WorkspaceFolderOrURI};
    let base_uri = URI("file:///workspace/project".to_string());
    let cases = [
        ("**/*", "/workspace/project"),
        ("src/**/*", "/workspace/project/src"),
        ("../shared/*", "/workspace/shared"),
    ];
    for (pattern, want) in cases {
        let watcher = FileSystemWatcher {
            glob_pattern: PatternOrRelativePattern {
                relative_pattern: Some(RelativePattern {
                    base_uri: WorkspaceFolderOrURI {
                        uri: Some(base_uri.clone()),
                        ..Default::default()
                    },
                    pattern: pattern.to_string(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let (got, ok) = watch_root(&watcher);
        assert!(
            ok && got == want,
            "watchRoot({pattern:?}) = {got:?}, {ok}, want {want:?}, true"
        );
    }
}

// Go: lspwatcher_test.go:245 TestWatcher_BookkeepingAndOverflow
#[test]
fn test_watcher_bookkeeping_and_overflow() {
    let (_guard, dir) = temp_dir();
    let dir_norm = tspath::normalize_slashes(&dir);
    let pattern = format!("{dir_norm}/**/*");
    let (got, on_changes) = collector();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(wrapped_os_fs(), b, on_changes, None);

    w.watch_files("id", &[fsw(&pattern, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));
    assert!(
        w.watch_files("id", &[fsw(&pattern, None)]).is_err(),
        "expected duplicate-id error"
    );

    backend.emit_all(
        vec![ev(EventKind::Update, &join(&[&dir_norm, "a.ts"]))],
        Some(ERR_OVERFLOW.clone()),
    );
    wait_for(|| !got.borrow().is_empty(), "events after overflow");

    assert!(
        w.unwatch_files("missing").is_err(),
        "expected unknown-id error"
    );
    w.unwatch_files("id")
        .unwrap_or_else(|e| panic!("{}", e.error()));
    w.watch_files("id2", &[])
        .unwrap_or_else(|e| panic!("{}", e.error()));
    w.close();
    assert!(w.watch_files("id3", &[]).is_err(), "expected closed error");
}

// Go: lspwatcher_test.go:298 TestWatcher_NonRecursiveGlobIsNotRecursive
#[test]
fn test_watcher_non_recursive_glob_is_not_recursive() {
    let (_guard, dir) = temp_dir();
    let dir_norm = tspath::normalize_slashes(&dir);
    std::fs::create_dir_all(join(&[&dir, "sub"])).unwrap();
    let sub_norm = tspath::normalize_slashes(&join(&[&dir, "sub"]));

    let fs = wrapped_os_fs();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, Box::new(|_| {}), None);
    let _close = CloseOnDrop(w.clone());

    let recursive = format!("{dir_norm}/**/*");
    let non_recursive = format!("{sub_norm}/*");
    w.watch_files("id", &[fsw(&recursive, None), fsw(&non_recursive, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    let opt_count = backend.opt_count.borrow();
    assert_eq!(
        opt_count.get(&dir_norm).copied().unwrap_or(0),
        1,
        "recursive glob {recursive:?}: expected 1 watch option (WithRecursive)"
    );
    assert_eq!(
        opt_count.get(&sub_norm).copied().unwrap_or(0),
        0,
        "non-recursive glob {non_recursive:?}: expected 0 watch options"
    );
}

// Go: lspwatcher_test.go:336 TestWatcher_RealBackend_MissingThenCreate (body)
fn missing_then_create_body() {
    let (_guard, base) = temp_dir();
    let (got, on_changes) = collector();
    let w = lspwatcher::new(wrapped_os_fs(), on_changes, None);
    let _close = CloseOnDrop(w.clone());

    let target = tspath::normalize_slashes(&join(&[&base, "pkg"]));
    let pattern = format!("{target}/*");
    w.watch_files("test", &[fsw(&pattern, Some(all_kinds()))])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    std::thread::sleep(Duration::from_millis(200));

    std::fs::create_dir_all(join(&[&base, "pkg"])).unwrap();
    std::fs::write(join(&[&base, "pkg", "index.ts"]), "export {}").unwrap();

    wait_for(
        || {
            got.borrow()
                .iter()
                .any(|e| e.uri.0.ends_with("/pkg/index.ts"))
        },
        "event for file created in a previously-missing directory",
    );
}

// Go: lspwatcher_test.go:336 TestWatcher_RealBackend_MissingThenCreate
#[test]
fn test_watcher_real_backend_missing_then_create() {
    run_child("MissingThenCreate");
}

// Go: lspwatcher_test.go:393 TestWatcher_MissingDirectoryTracksAncestor
#[test]
fn test_watcher_missing_directory_tracks_ancestor() {
    let fs = wrapped_os_fs();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, Box::new(|_| {}), None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, base) = temp_dir();
    let base_norm = tspath::normalize_slashes(&base);
    let target = tspath::normalize_slashes(&join(&[&base, "pkg"]));
    let pattern = format!("{target}/*");

    w.watch_files("id", &[fsw(&pattern, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    assert!(
        backend.is_watching(&base_norm),
        "expected ancestor watch on ancestor {base_norm:?}, watched: {:?}",
        backend.watched_dirs()
    );
    let dirs = backend.watched_dirs();
    assert_eq!(
        dirs.len(),
        1,
        "expected exactly one (ancestor) watch, got {dirs:?}"
    );

    w.unwatch_files("id")
        .unwrap_or_else(|e| panic!("{}", e.error()));
    let dirs = backend.watched_dirs();
    assert!(
        dirs.is_empty(),
        "expected all watches closed after unwatch, got {dirs:?}"
    );
}

// Go: lspwatcher_test.go:428 TestWatcher_MissingDirectoryPromotesOnCreate
#[test]
fn test_watcher_missing_directory_promotes_on_create() {
    let fs = wrapped_os_fs();
    let (got, on_changes) = collector();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, on_changes, None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, base) = temp_dir();
    let base_norm = tspath::normalize_slashes(&base);
    let target = tspath::normalize_slashes(&join(&[&base, "pkg"]));
    let pattern = format!("{target}/*");

    w.watch_files("id", &[fsw(&pattern, Some(all_kinds()))])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    std::fs::create_dir_all(join(&[&base, "pkg"])).unwrap();
    std::fs::write(join(&[&base, "pkg", "index.ts"]), "export {}").unwrap();
    backend.emit(
        &base_norm,
        vec![ev(EventKind::Update, &join(&[&base, "pkg"]))],
        None,
    );

    wait_for(|| backend.is_watching(&target), "promotion to target watch");

    wait_for(
        || {
            let got = got.borrow();
            let created = got.iter().filter(|e| e.type_ == FileChangeType::CREATED);
            let mut saw_dir = false;
            let mut saw_child = false;
            for e in created {
                saw_dir |= e.uri.0.ends_with("/pkg");
                saw_child |= e.uri.0.ends_with("/pkg/index.ts");
            }
            saw_dir && saw_child
        },
        "synthetic create events for target and child",
    );
}

// Go: lspwatcher_test.go:487 TestWatcher_MultiLevelDescend
#[test]
fn test_watcher_multi_level_descend() {
    let fs = wrapped_os_fs();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, Box::new(|_| {}), None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, base) = temp_dir();
    let base_norm = tspath::normalize_slashes(&base);
    let target = tspath::normalize_slashes(&join(&[&base, "a", "b", "c"]));
    let pattern = format!("{target}/*");

    w.watch_files("id", &[fsw(&pattern, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));
    assert!(
        backend.is_watching(&base_norm),
        "expected initial ancestor watch on {base_norm:?}, got {:?}",
        backend.watched_dirs()
    );

    let mkdir_and_path = |rel: &[&str]| -> String {
        let mut parts = vec![base.as_str()];
        parts.extend_from_slice(rel);
        let p = join(&parts);
        std::fs::create_dir_all(&p).unwrap();
        tspath::normalize_slashes(&p)
    };

    let a_dir = mkdir_and_path(&["a"]);
    backend.emit(
        &base_norm,
        vec![ev(EventKind::Update, &join(&[&base, "a"]))],
        None,
    );
    wait_for(|| backend.is_watching(&a_dir), "descend to a");

    let ab_dir = mkdir_and_path(&["a", "b"]);
    backend.emit(
        &a_dir,
        vec![ev(EventKind::Update, &join(&[&base, "a", "b"]))],
        None,
    );
    wait_for(|| backend.is_watching(&ab_dir), "descend to a/b");

    let abc_dir = mkdir_and_path(&["a", "b", "c"]);
    backend.emit(
        &ab_dir,
        vec![ev(EventKind::Update, &join(&[&base, "a", "b", "c"]))],
        None,
    );
    wait_for(|| backend.is_watching(&abc_dir), "promote to target a/b/c");
}

// Go: lspwatcher_test.go:535 TestWatcher_AtomicTreeCreateRace
#[test]
fn test_watcher_atomic_tree_create_race() {
    let fs = wrapped_os_fs();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, Box::new(|_| {}), None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, base) = temp_dir();
    let base_norm = tspath::normalize_slashes(&base);
    let target = tspath::normalize_slashes(&join(&[&base, "a", "b", "c"]));
    let pattern = format!("{target}/*");

    w.watch_files("id", &[fsw(&pattern, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    std::fs::create_dir_all(join(&[&base, "a", "b", "c"])).unwrap();
    backend.emit(
        &base_norm,
        vec![ev(EventKind::Update, &join(&[&base, "a"]))],
        None,
    );

    wait_for(
        || backend.is_watching(&target),
        "promote to target in one pass",
    );
}

// Go: lspwatcher_test.go:566 TestWatcher_SyntheticCreateDepth
#[test]
fn test_watcher_synthetic_create_depth() {
    for recursive in [false, true] {
        let name = if recursive {
            "recursive"
        } else {
            "non-recursive"
        };
        let fs = wrapped_os_fs();
        let (got, on_changes) = collector();
        let (backend, b) = new_fake_backend();
        let w = new_with_backend(fs, b, on_changes, None);
        let _close = CloseOnDrop(w.clone());

        let (_guard, base) = temp_dir();
        let base_norm = tspath::normalize_slashes(&base);
        let target = tspath::normalize_slashes(&join(&[&base, "pkg"]));
        let pattern = if recursive {
            format!("{target}/**/*")
        } else {
            format!("{target}/*")
        };
        w.watch_files("id", &[fsw(&pattern, Some(all_kinds()))])
            .unwrap_or_else(|e| panic!("{name}: {}", e.error()));

        std::fs::create_dir_all(join(&[&base, "pkg", "sub"])).unwrap();
        std::fs::write(join(&[&base, "pkg", "top.ts"]), "export {}").unwrap();
        std::fs::write(join(&[&base, "pkg", "sub", "deep.ts"]), "export {}").unwrap();
        backend.emit(
            &base_norm,
            vec![ev(EventKind::Update, &join(&[&base, "pkg"]))],
            None,
        );

        wait_for(|| backend.is_watching(&target), "promotion");

        let created = |suffix: &str| {
            got.borrow()
                .iter()
                .any(|e| e.type_ == FileChangeType::CREATED && e.uri.0.ends_with(suffix))
        };

        // Both modes must synthesize the immediate child.
        wait_for(
            || created("/pkg/top.ts"),
            "synthetic create for immediate child",
        );

        // Only the recursive watch should synthesize the deep descendant.
        if recursive {
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline && !created("/pkg/sub/deep.ts") {
                local::run_pending();
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                created("/pkg/sub/deep.ts"),
                "{name}: recursive watch should synthesize deep descendant; got {:?}",
                got.borrow()
            );
        } else {
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                local::run_pending();
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                !created("/pkg/sub/deep.ts"),
                "{name}: non-recursive watch must not synthesize deep descendant; got {:?}",
                got.borrow()
            );
        }
    }
}

// Go: lspwatcher_test.go:667 TestWatcher_TerminatedFallsBackAndRecovers
#[test]
fn test_watcher_terminated_falls_back_and_recovers() {
    let fs = wrapped_os_fs();
    let (got, on_changes) = collector();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, on_changes, None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, base) = temp_dir();
    let base_norm = tspath::normalize_slashes(&base);
    let target = tspath::normalize_slashes(&join(&[&base, "pkg"]));
    std::fs::create_dir_all(join(&[&base, "pkg"])).unwrap();
    let pattern = format!("{target}/*");

    w.watch_files("id", &[fsw(&pattern, Some(all_kinds()))])
        .unwrap_or_else(|e| panic!("{}", e.error()));
    assert!(
        backend.is_watching(&target),
        "expected target watch on {target:?}, got {:?}",
        backend.watched_dirs()
    );

    std::fs::remove_dir_all(join(&[&base, "pkg"])).unwrap();
    backend.emit(
        &target,
        vec![ev(EventKind::Delete, &join(&[&base, "pkg"]))],
        errors::join(vec![ERR_WATCH_TERMINATED.clone(), errors::new("removed")]),
    );

    wait_for(
        || {
            got.borrow()
                .iter()
                .any(|e| e.type_ == FileChangeType::DELETED && e.uri.0.ends_with("/pkg"))
        },
        "forwarded delete of terminated dir",
    );
    wait_for(
        || backend.is_watching(&base_norm) && !backend.is_watching(&target),
        "fallback to ancestor watch",
    );

    std::fs::create_dir_all(join(&[&base, "pkg"])).unwrap();
    backend.emit(
        &base_norm,
        vec![ev(EventKind::Update, &join(&[&base, "pkg"]))],
        None,
    );
    wait_for(
        || backend.is_watching(&target),
        "recovery to target watch after recreation",
    );
}

// Go: lspwatcher_test.go:735 TestWatcher_GenuineFailureRollsBackForRetry
#[test]
fn test_watcher_genuine_failure_rolls_back_for_retry() {
    let fs = wrapped_os_fs();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(fs, b, Box::new(|_| {}), None);
    let _close = CloseOnDrop(w.clone());

    let (_guard, dir) = temp_dir();
    let dir_norm = tspath::normalize_slashes(&dir);
    let pattern = format!("{dir_norm}/*");

    backend
        .fail_dirs
        .borrow_mut()
        .insert(dir_norm.clone(), errors::new("too many open files"));

    assert!(
        w.watch_files("id", &[fsw(&pattern, None)]).is_err(),
        "expected error from genuine backend failure"
    );

    backend.fail_dirs.borrow_mut().remove(&dir_norm);

    if let Err(err) = w.watch_files("id", &[fsw(&pattern, None)]) {
        panic!("retry after rollback should succeed, got {}", err.error());
    }
    assert!(
        backend.is_watching(&dir_norm),
        "expected watch on {dir_norm:?} after successful retry, got {:?}",
        backend.watched_dirs()
    );
}

// Go: lspwatcher_test.go:776 TestWatcher_WatchTerminatedDoesNotDropEvents
#[test]
fn test_watcher_watch_terminated_does_not_drop_events() {
    let (_guard, dir) = temp_dir();
    let dir_norm = tspath::normalize_slashes(&dir);
    let (got, on_changes) = collector();
    let (backend, b) = new_fake_backend();
    let w = new_with_backend(wrapped_os_fs(), b, on_changes, None);

    let pattern = format!("{dir_norm}/**/*");
    w.watch_files("id", &[fsw(&pattern, None)])
        .unwrap_or_else(|e| panic!("{}", e.error()));

    backend.emit_all(
        vec![ev(EventKind::Update, &join(&[&dir_norm, "b.ts"]))],
        errors::join(vec![ERR_WATCH_TERMINATED.clone(), errors::new("simulated")]),
    );

    wait_for(
        || !got.borrow().is_empty(),
        "events with watch-terminated error",
    );
}

// Go: lspwatcher_test.go:807 blockingBackend
/// PORT: the Rust watcher lives on one thread, so Go's two goroutines
/// (`WatchFiles` waits in `WatchDirectory` while the test calls `Close`)
/// are one call chain here: `watch_directory` closes the watcher at the
/// point where Go's backend waits for `release`.
struct BlockingBackend {
    watcher: Rc<RefCell<Option<Rc<Watcher>>>>,
}

impl WatcherBackend for BlockingBackend {
    // Go: lspwatcher_test.go:812 blockingBackend.WatchDirectory
    fn watch_directory(
        &self,
        _dir: &str,
        _fn: LocalWatchCallback,
        _opts: &[Box<dyn fswatch::WatchOption>],
    ) -> Result<Box<dyn fswatch::Watch>, GoError> {
        // Go: close(b.entered); <-b.release, while the test calls w.Close().
        let watcher = self.watcher.borrow().clone();
        if let Some(w) = watcher {
            w.close();
        }
        // Go: fakeWatch{closeFn: func() error { return nil }}
        Ok(Box::new(FakeWatch {
            dir: String::new(),
            closes: Arc::new(Mutex::new(Vec::new())),
        }))
    }
}

// Go: lspwatcher_test.go:822 TestWatcher_CloseWhileWatchFilesReconciles
#[test]
fn test_watcher_close_while_watch_files_reconciles() {
    let (_guard, dir) = temp_dir();
    let pattern = format!("{}/**/*", tspath::normalize_slashes(&dir));
    let watcher: Rc<RefCell<Option<Rc<Watcher>>>> = Rc::new(RefCell::new(None));
    let backend = BlockingBackend {
        watcher: watcher.clone(),
    };
    let w = new_with_backend(wrapped_os_fs(), Box::new(backend), Box::new(|_| {}), None);
    *watcher.borrow_mut() = Some(w.clone());

    let done = w.watch_files("id", &[fsw(&pattern, None)]);
    // PORT: break the watcher/backend cycle.
    *watcher.borrow_mut() = None;

    assert!(
        done.is_err(),
        "expected WatchFiles to report that the watcher was closed"
    );
}
