//! Go: `internal/fswatch/{watcher,testutil}_test.go` (every OS) and
//! `fanotify_linux_test.go` (Linux). `tests/fswatch_linux.rs` already has
//! `TestWatchFileCreate` and `TestSubscribeSubfileUpdate` for the default
//! backend only; this file runs every Go test on every available backend,
//! as Go does.
//!
//! PORT: Go `testingT` and `retryT` are `T`. A Go `t.Fatal` is a panic; Go
//! `runWithRetry` catches it and retries the body with longer timeouts, up
//! to three attempts. Go runs the per-backend subtests in parallel; the
//! port runs them on scoped threads. Go `t.TempDir()` is `T::temp_dir`
//! (under `std::env::temp_dir()`, as Go's `os.TempDir`), removed after the
//! cleanups. Go `w == FSEvents() || w == Kqueue()` compares the backend
//! name (`is_kqueue_or_fsevents`), and Go `runtime.GOOS` is
//! `std::env::consts::OS` ("macos" for Go "darwin"). The fanotify items
//! and `fanotify_linux_test.go` build on Linux only, as in Go. The Windows
//! branches are not ported (this file uses `std::os::unix`).
//!
//! Not portable: `TestSubscribeRejectsNilCallback` (a Rust `WatchCallback`
//! cannot be nil). `TestLinuxFanotifyBackendSelection` checks only that the
//! fanotify watcher starts: Go's `impl.(*fanotifyBackend)` type assertion
//! has no Rust form for a `dyn WatcherImpl` without `Any`.

use std::cell::RefCell;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use ts_goport::fswatch::fanotify_linux::{
    fanotify_available, make_fanotify_handle_key, maybe_wrap_unsupported_filesystem,
    new_fanotify_backend,
};
use ts_goport::fswatch::pathcompare::PathComparer;
use ts_goport::fswatch::{
    self, DirWatch, DirWatchError, ERR_UNAVAILABLE, ERR_WATCH_TERMINATED, Event, EventKind,
    EventList, MAX_WAIT_TIME, Watch, WatchCallback, WatchOption, Watcher, WatcherBase, WatcherImpl,
    new_debounce, new_dir_watch, with_recursive,
};
#[cfg(target_os = "linux")]
use ts_goport::fswatch::{ERR_FILESYSTEM_UNSUPPORTED, WatcherStruct, new_watcher};
use ts_goport::gostd::{GoError, errors};

use crate::astnav_api::panic_message;

// ----- temp dirs ---------------------------------------------------------

/// A temp directory that is removed on drop (Go `t.TempDir()`).
pub(crate) struct TmpDir(PathBuf);

impl TmpDir {
    /// The directory as created (not resolved).
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn make_tmp_dir() -> TmpDir {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "goport-s2-fswatch-{}-{n}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    TmpDir(dir)
}

// Go: watcher_test.go:111 newTmpDir (for tests that take no `T`)
/// A fresh temp dir with symlinks resolved, so it matches what backends
/// report. Removed when the `TmpDir` drops.
pub(crate) fn new_tmp_dir() -> (TmpDir, PathBuf) {
    let d = make_tmp_dir();
    let resolved = crate::support::eval_symlinks(&d.0).expect("EvalSymlinks");
    (d, resolved)
}

// ----- testingT / retryT -------------------------------------------------

/// Go `testingT` (a `*retryT`): the retry attempt, the cleanups and the
/// temp dirs of one attempt.
pub(crate) struct T {
    attempt: u32,
    cleanups: RefCell<Vec<Box<dyn FnOnce()>>>,
    tmps: RefCell<Vec<TmpDir>>,
}

/// Panic payload of Go `t.Skip`.
struct Skip(String);

impl T {
    fn new(attempt: u32) -> T {
        T {
            attempt,
            cleanups: RefCell::new(Vec::new()),
            tmps: RefCell::new(Vec::new()),
        }
    }

    /// Go `t.Cleanup`.
    pub(crate) fn cleanup(&self, f: impl FnOnce() + 'static) {
        self.cleanups.borrow_mut().push(Box::new(f));
    }

    /// Go `t.TempDir()`.
    pub(crate) fn temp_dir(&self) -> String {
        let d = make_tmp_dir();
        let p = d.0.to_str().unwrap().to_string();
        self.tmps.borrow_mut().push(d);
        p
    }

    /// Runs the cleanups last-in first-out, then removes the temp dirs.
    fn finish(&self) {
        let cleanups = std::mem::take(&mut *self.cleanups.borrow_mut());
        for f in cleanups.into_iter().rev() {
            f();
        }
        self.tmps.borrow_mut().clear();
    }
}

/// Go `t.Fatal`.
pub(crate) fn fatal(message: impl Into<String>) -> ! {
    panic!("{}", message.into())
}

/// Go `t.Skip`.
fn skip(message: &str) -> ! {
    std::panic::panic_any(Skip(message.to_string()))
}

// Go: testutil_test.go retryAttempts
const RETRY_ATTEMPTS: u32 = 3;

// Go: testutil_test.go retryTimeoutScale
fn retry_timeout_scale(attempt: u32) -> u32 {
    match attempt {
        1 => 1,
        2 => 5,
        _ => 15,
    }
}

// Go: testutil_test.go runWithRetry
/// Runs `body` up to three times; `Err` has the last failure.
fn run_with_retry(name: &str, body: &(dyn Fn(&T) + Sync)) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 1..=RETRY_ATTEMPTS {
        let t = T::new(attempt);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&t)));
        t.finish();
        match result {
            Ok(()) => {
                if attempt > 1 {
                    println!("{name}: retry: succeeded on attempt {attempt}/{RETRY_ATTEMPTS}");
                }
                return Ok(());
            }
            Err(payload) => {
                if let Some(Skip(reason)) = payload.downcast_ref::<Skip>() {
                    println!("SKIP {name}: {reason}");
                    return Ok(());
                }
                last = panic_message(payload.as_ref());
            }
        }
    }
    Err(format!(
        "retry: gave up after {RETRY_ATTEMPTS} attempts; last failure: {last}"
    ))
}

// Go: fanotify_linux_test.go:21 fanotifyNoRenameWatcher (and its init)
#[cfg(target_os = "linux")]
static FANOTIFY_NO_RENAME_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("fanotify-no-rename", |w| {
        if fanotify_available() {
            w.factory = Some(|| -> Arc<dyn WatcherImpl> { new_fanotify_backend(true) });
        }
    })
});

// Go: watcher_test.go:66 availableWatchers (with additionalTestWatchers)
// PORT: only fanotify_linux_test.go adds a test watcher, so the other
// platforms have none.
fn available_watchers() -> Vec<Arc<dyn Watcher>> {
    #[allow(unused_mut)]
    let mut out: Vec<Arc<dyn Watcher>> = fswatch::all_watchers()
        .into_iter()
        .filter(|w| w.available())
        .collect();
    #[cfg(target_os = "linux")]
    if fanotify_available() {
        let w: Arc<dyn Watcher> = FANOTIFY_NO_RENAME_WATCHER.clone();
        if w.available() {
            out.push(w);
        }
    }
    out
}

// Go: watcher_test.go:97 runForEachWatcher
/// Runs `body` for every available watcher (in parallel, with retry) and
/// fails with every backend that failed.
fn run_for_each_watcher(test: &str, body: fn(&T, &Arc<dyn Watcher>)) {
    let watchers = available_watchers();
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for w in &watchers {
            let failures = &failures;
            s.spawn(move || {
                let name = format!("{test}/{}", w.name());
                let run = |t: &T| body(t, w);
                if let Err(err) = run_with_retry(&name, &run) {
                    failures.lock().unwrap().push(format!("{name}: {err}"));
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    if !failures.is_empty() {
        panic!("{}", failures.join("\n"));
    }
}

// ----- helpers -----------------------------------------------------------

// Go: watcher_test.go:31 defaultEventTimeout
fn default_event_timeout() -> Duration {
    Duration::from_secs(1)
}

// Go: watcher_test.go:41 kqueueFSEventsTimeout
fn kqueue_fsevents_timeout() -> Duration {
    Duration::from_secs(2)
}

/// Go `w == FSEvents() || w == Kqueue()`. PORT: Go compares the watcher
/// values; the port compares the backend names.
pub(crate) fn is_kqueue_or_fsevents(w: &Arc<dyn Watcher>) -> bool {
    matches!(w.name().as_str(), "fsevents" | "kqueue")
}

// Go: watcher_test.go:50 watcherEventTimeout (the base; `Recorder::deadline`
// applies the retry scale)
pub(crate) fn watcher_event_timeout_base(w: &Arc<dyn Watcher>) -> Duration {
    if is_kqueue_or_fsevents(w) {
        kqueue_fsevents_timeout()
    } else {
        default_event_timeout()
    }
}

// Go: watcher_test.go:111 newTmpDir
fn new_t_tmp_dir(t: &T) -> String {
    let d = t.temp_dir();
    crate::support::eval_symlinks(&d)
        .unwrap_or_else(|e| fatal(format!("EvalSymlinks: {e}")))
        .to_str()
        .unwrap()
        .to_string()
}

// Go: watcher_test.go:134 nameCounter
static NAME_COUNTER: AtomicU64 = AtomicU64::new(0);

// Go: watcher_test.go:136 uniqueName
fn unique_name(dir: &str) -> String {
    let n = NAME_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    // Go `rand.Int63()`; any unique suffix does.
    let r = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        % (1u128 << 63);
    join(dir, &format!("test{n}{r}"))
}

// Go: watcher_test.go:143 subPath
fn sub_path(dir: &str) -> String {
    unique_name(dir)
}

/// Go `filepath.Join(dir, name)`.
// PORT: the tests join clean paths, so Go's `Clean` is only the separator:
// on Windows `Join` gives backslashes, also for a `/` inside `name`.
fn join(dir: &str, name: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR_STR;
    format!("{dir}{sep}{name}").replace('/', sep)
}

// Go: watcher_test.go:150 newDirectWatcher
fn new_direct_watcher(t: &T, dir: &str) -> Arc<DirWatch> {
    let w = new_dir_watch(dir, dir, new_debounce(), true, PathComparer::default());
    let w2 = w.clone();
    t.cleanup(move || w2.destroy_debounce());
    w
}

// Go: watcher_test.go:159 subscribeFor
fn subscribe_for(t: &T, dir: &str, w: &Arc<dyn Watcher>) -> (Arc<Recorder>, Arc<dyn Watch>) {
    subscribe_for_opts(t, dir, w, vec![with_recursive()])
}

// Go: watcher_test.go:167 settleSleep
pub(crate) fn settle_sleep(w: &Arc<dyn Watcher>) -> Duration {
    if is_kqueue_or_fsevents(w) {
        return Duration::from_millis(300);
    }
    Duration::from_millis(60)
}

// Go: watcher_test.go:177 preSubscribeSleep
pub(crate) fn pre_subscribe_sleep(w: &Arc<dyn Watcher>) -> Duration {
    if is_kqueue_or_fsevents(w) {
        return Duration::from_millis(50);
    }
    Duration::ZERO
}

// Go: watcher_test.go:185 subscribeFileFor
fn subscribe_file_for(t: &T, path: &str, w: &Arc<dyn Watcher>) -> (Arc<Recorder>, Arc<dyn Watch>) {
    let d = pre_subscribe_sleep(w);
    if d > Duration::ZERO {
        std::thread::sleep(d);
    }
    let r = Recorder::for_watcher(t, w);
    let sub = w
        .watch_file(path, r.callback())
        .unwrap_or_else(|e| fatal(format!("subscribeFile: {}", e.error())));
    let sub: Arc<dyn Watch> = Arc::from(sub);
    let s2 = sub.clone();
    t.cleanup(move || {
        let _ = s2.close();
    });
    std::thread::sleep(settle_sleep(w));
    (r, sub)
}

// Go: watcher_test.go:202 subscribeForOpts
fn subscribe_for_opts(
    t: &T,
    dir: &str,
    w: &Arc<dyn Watcher>,
    opts: Vec<Box<dyn WatchOption>>,
) -> (Arc<Recorder>, Arc<dyn Watch>) {
    let d = pre_subscribe_sleep(w);
    if d > Duration::ZERO {
        std::thread::sleep(d);
    }
    let r = Recorder::for_watcher(t, w);
    let sub = w
        .watch_directory(dir, r.callback(), &opts)
        .unwrap_or_else(|e| fatal(format!("subscribe: {}", e.error())));
    let sub: Arc<dyn Watch> = Arc::from(sub);
    let s2 = sub.clone();
    t.cleanup(move || {
        let _ = s2.close();
    });
    std::thread::sleep(settle_sleep(w));
    (r, sub)
}

/// A callback that ignores everything (Go `func([]Event, error) {}`).
fn noop_callback() -> WatchCallback {
    Arc::new(|_, _| {})
}

// ----- recordingWatcher --------------------------------------------------

#[derive(Default)]
struct RecState {
    buf: Vec<Event>,
    errs: Vec<GoError>,
}

// Go: watcher_test.go:220 recordingWatcher
// PORT: Go keeps the bound `watcher` to choose the timeout; the port keeps
// that base timeout (`base`).
pub(crate) struct Recorder {
    attempt: u32,
    base: Duration,
    state: Mutex<RecState>,
    cond: Condvar,
}

impl Recorder {
    // Go: watcher_test.go:229 newRecorder
    fn new(t: &T) -> Arc<Recorder> {
        Self::with_base(t, default_event_timeout())
    }

    /// Go `newRecorder(t)` followed by `r.watcher = w`.
    fn for_watcher(t: &T, w: &Arc<dyn Watcher>) -> Arc<Recorder> {
        Self::with_base(t, watcher_event_timeout_base(w))
    }

    fn with_base(t: &T, base: Duration) -> Arc<Recorder> {
        Arc::new(Recorder {
            attempt: t.attempt,
            base,
            state: Mutex::new(RecState::default()),
            cond: Condvar::new(),
        })
    }

    // Go: watcher_test.go:239 deadline (watcherEventTimeout for a bound
    // watcher, scaledDeadline(defaultEventTimeout) for an unbound one)
    fn deadline(&self) -> Duration {
        self.base * retry_timeout_scale(self.attempt)
    }

    // Go: watcher_test.go:256 callback
    fn callback(self: &Arc<Self>) -> WatchCallback {
        let r = Arc::downgrade(self);
        Arc::new(move |events: Vec<Event>, err: Option<GoError>| {
            let Some(r) = r.upgrade() else {
                return;
            };
            let mut s = r.state.lock().unwrap();
            if let Some(err) = err {
                s.errs.push(err);
            }
            s.buf.extend(events);
            r.cond.notify_all();
        })
    }

    // Go: watcher_test.go:268 next
    fn next(&self, d: Duration) -> Vec<Event> {
        let deadline = Instant::now() + d;
        let mut s = self.state.lock().unwrap();
        while s.buf.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return Vec::new();
            }
            s = self.cond.wait_timeout(s, deadline - now).unwrap().0;
        }
        std::mem::take(&mut s.buf)
    }

    // Go: watcher_test.go:293 drainQuiet
    fn drain_quiet(&self, d: Duration) -> Vec<Event> {
        self.state.lock().unwrap().buf.clear();
        std::thread::sleep(d);
        std::mem::take(&mut self.state.lock().unwrap().buf)
    }

    // Go: watcher_test.go:308 gather
    fn gather(&self, wait: Duration, settle: Duration) -> Vec<Event> {
        let mut first = self.next(wait);
        if first.is_empty() {
            return Vec::new();
        }
        std::thread::sleep(settle);
        first.extend(std::mem::take(&mut self.state.lock().unwrap().buf));
        first
    }

    // Go: watcher_test.go:325 gatherUntilQuiet
    fn gather_until_quiet(&self, initial_wait: Duration, quiet: Duration) -> Vec<Event> {
        let mut all = self.next(initial_wait);
        if all.is_empty() {
            return all;
        }
        loop {
            let more = self.next(quiet);
            if more.is_empty() {
                return all;
            }
            all.extend(more);
        }
    }

    // Go: watcher_test.go:346 waitForEvent
    fn wait_for_event(&self, d: Duration, pred: impl Fn(&Event) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + d;
        let mut s = self.state.lock().unwrap();
        loop {
            if s.buf.iter().any(&pred) {
                return std::mem::take(&mut s.buf);
            }
            let now = Instant::now();
            if now >= deadline {
                return std::mem::take(&mut s.buf);
            }
            s = self.cond.wait_timeout(s, deadline - now).unwrap().0;
        }
    }

    // Go: watcher_test.go:385 waitForAll
    fn wait_for_all(&self, d: Duration, want: &[W]) -> Vec<Event> {
        if want.is_empty() {
            return Vec::new();
        }
        let deadline = Instant::now() + d;
        let mut collected = Vec::new();
        let mut s = self.state.lock().unwrap();
        loop {
            collected.append(&mut s.buf);
            if have_all(&collected, want) {
                return collected;
            }
            let now = Instant::now();
            if now >= deadline {
                return collected;
            }
            s = self.cond.wait_timeout(s, deadline - now).unwrap().0;
        }
    }

    fn take_errs(&self) -> Vec<GoError> {
        std::mem::take(&mut self.state.lock().unwrap().errs)
    }

    fn err_count(&self) -> usize {
        self.state.lock().unwrap().errs.len()
    }
}

// ----- assertion helpers -------------------------------------------------

// Go: watcher_test.go:498 wantEvent
type W = (EventKind, String);

fn w(kind: EventKind, path: &str) -> W {
    (kind, path.to_string())
}

const UPDATE: EventKind = EventKind::Update;
const DELETE: EventKind = EventKind::Delete;

// Go: watcher_test.go:422 haveAll
fn have_all(got: &[Event], want: &[W]) -> bool {
    want.iter()
        .all(|(k, p)| got.iter().any(|e| e.kind == *k && e.path == *p))
}

// Go: watcher_test.go:443 expectEventSet
fn expect_event_set(r: &Recorder, want: &[W]) -> Vec<Event> {
    let got = r.wait_for_all(r.deadline(), want);
    assert_event_set(&got, want);
    got
}

// Go: watcher_test.go:454 expectEventSequence
fn expect_event_sequence(r: &Recorder, want: &[W]) -> Vec<Event> {
    let got = r.wait_for_all(r.deadline(), want);
    assert_event_sequence(&got, want);
    got
}

// Go: watcher_test.go:465 expectContains
fn expect_contains(r: &Recorder, kind: EventKind, path: &str) -> Vec<Event> {
    let d = r.deadline();
    let got = r.wait_for_event(d, |e| e.kind == kind && e.path == path);
    if !contains_event(&got, kind, path) {
        fatal(format!(
            "expected event {} {path} within {d:?}, got {:?}",
            kind.string(),
            to_want_events(&got)
        ));
    }
    got
}

// Go: watcher_test.go:477 expectNoBufferedEvents
fn expect_no_buffered_events(r: &Recorder, msg: &str) {
    let got = std::mem::take(&mut r.state.lock().unwrap().buf);
    if !got.is_empty() {
        fatal(format!("{msg}, got {:?}", to_want_events(&got)));
    }
}

// Go: watcher_test.go:488 assertNoEventsForPath
fn assert_no_events_for_path(got: &[Event], path: &str, msg: &str) {
    let got = filter_events_for_paths(got, &[path]);
    if !got.is_empty() {
        fatal(format!("{msg} {path}, got {:?}", to_want_events(&got)));
    }
}

// Go: watcher_test.go:503 toWantEvents
fn to_want_events(events: &[Event]) -> Vec<W> {
    events.iter().map(|e| (e.kind, e.path.clone())).collect()
}

// Go: watcher_test.go:513 assertEventSet
fn assert_event_set(got: &[Event], want: &[W]) {
    let got = filter_to_wanted_paths(got, want);
    let mut got_w = to_want_events(&got);
    let mut want = want.to_vec();
    let key = |a: &W| (a.0 as i32, a.1.clone());
    got_w.sort_by_key(key);
    want.sort_by_key(key);
    if got_w != want {
        fatal(format!("event mismatch\nwant: {want:?}\n got: {got_w:?}"));
    }
}

// Go: watcher_test.go:532 assertEventSequence
fn assert_event_sequence(got: &[Event], want: &[W]) {
    let got = filter_to_wanted_paths(got, want);
    let got_w = to_want_events(&got);
    if got_w != want {
        fatal(format!(
            "event sequence mismatch\nwant: {want:?}\n got: {got_w:?}"
        ));
    }
}

// Go: watcher_test.go:542 filterToWantedPaths
fn filter_to_wanted_paths(got: &[Event], want: &[W]) -> Vec<Event> {
    got.iter()
        .filter(|e| want.iter().any(|(_, p)| *p == e.path))
        .cloned()
        .collect()
}

// Go: watcher_test.go:569 containsEvent
fn contains_event(got: &[Event], kind: EventKind, path: &str) -> bool {
    got.iter().any(|e| e.kind == kind && e.path == path)
}

// Go: watcher_test.go:820 filterEventsForPaths
fn filter_events_for_paths(events: &[Event], paths: &[&str]) -> Vec<Event> {
    events
        .iter()
        .filter(|e| paths.contains(&e.path.as_str()))
        .cloned()
        .collect()
}

// Go: watcher_test.go:838 replayEventList
fn replay_event_list(events: &[Event]) -> Vec<Event> {
    let el = EventList::default();
    for e in events {
        match e.kind {
            EventKind::Update => el.update(&e.path),
            EventKind::Delete => el.remove(&e.path),
        }
    }
    el.get_events()
}

// ----- small fs helpers (Go os.*; a failure is t.Fatal) ------------------

fn write_file(p: &str, data: &str) {
    std::fs::write(p, data).unwrap_or_else(|e| fatal(format!("WriteFile {p}: {e}")));
}

fn mkdir(p: &str) {
    std::fs::create_dir(p).unwrap_or_else(|e| fatal(format!("Mkdir {p}: {e}")));
}

fn mkdir_all(p: &str) {
    std::fs::create_dir_all(p).unwrap_or_else(|e| fatal(format!("MkdirAll {p}: {e}")));
}

fn rename(a: &str, b: &str) {
    std::fs::rename(a, b).unwrap_or_else(|e| fatal(format!("Rename {a} {b}: {e}")));
}

fn remove(p: &str) {
    let md = std::fs::symlink_metadata(p).unwrap_or_else(|e| fatal(format!("Remove {p}: {e}")));
    let r = if md.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    };
    r.unwrap_or_else(|e| fatal(format!("Remove {p}: {e}")));
}

fn remove_all(p: &str) {
    match std::fs::remove_dir_all(p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => fatal(format!("RemoveAll {p}: {e}")),
    }
}

#[cfg(unix)]
fn symlink(target: &str, link: &str) {
    std::os::unix::fs::symlink(target, link)
        .unwrap_or_else(|e| fatal(format!("Symlink {link}: {e}")));
}

// Go: os/file_windows.go:392 Symlink (a directory link when the target is
// a directory).
#[cfg(windows)]
fn symlink(target: &str, link: &str) {
    let result = if std::fs::metadata(target).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    result.unwrap_or_else(|e| fatal(format!("Symlink {link}: {e}")));
}

#[cfg(unix)]
fn chmod(p: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| fatal(format!("Chmod {p}: {e}")));
}

// Go: os/file_posix.go:60 syscallMode and syscall_windows.go Chmod: on
// Windows only the owner write bit counts; it clears or sets read-only.
#[cfg(windows)]
fn chmod(p: &str, mode: u32) {
    let result = std::fs::metadata(p).and_then(|m| {
        let mut permissions = m.permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        std::fs::set_permissions(p, permissions)
    });
    result.unwrap_or_else(|e| fatal(format!("Chmod {p}: {e}")));
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn sleep(d: Duration) {
    std::thread::sleep(d);
}

// ----- files -------------------------------------------------------------

// Go: watcher_test.go:853 TestWatchFileCreate
#[test]
fn test_watch_file_create() {
    run_for_each_watcher("TestWatchFileCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "hello");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:867 TestWatchFileUpdate
#[test]
fn test_watch_file_update() {
    run_for_each_watcher("TestWatchFileUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true); // consume the create event
        write_file(&f, "v2-longer");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:887 TestWatchFileRename
#[test]
fn test_watch_file_rename() {
    run_for_each_watcher("TestWatchFileRename", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = sub_path(&dir);
        let f2 = sub_path(&dir);
        write_file(&f1, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        rename(&f1, &f2);
        expect_event_set(&r, &[w(DELETE, &f1), w(UPDATE, &f2)]);
    });
}

// Go: watcher_test.go:907 TestWatchFileRenameExisting
#[test]
fn test_watch_file_rename_existing() {
    run_for_each_watcher("TestWatchFileRenameExisting", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = sub_path(&dir);
        write_file(&f1, "hi");
        let (r, _) = subscribe_for(t, &dir, wi);
        let f2 = sub_path(&dir);
        rename(&f1, &f2);
        expect_event_set(&r, &[w(DELETE, &f1), w(UPDATE, &f2)]);
    });
}

// Go: watcher_test.go:928 TestWatchFileDelete
#[test]
fn test_watch_file_delete() {
    run_for_each_watcher("TestWatchFileDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        write_file(&f, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove(&f);
        expect_event_sequence(&r, &[w(DELETE, &f)]);
    });
}

// ----- directories -------------------------------------------------------

// Go: watcher_test.go:946 TestSubscribeDirCreate
#[test]
fn test_subscribe_dir_create() {
    run_for_each_watcher("TestSubscribeDirCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        mkdir(&f);
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:965 TestSubscribeNonASCIIPath
#[test]
fn test_subscribe_non_ascii_path() {
    run_for_each_watcher("TestSubscribeNonASCIIPath", |t, wi| {
        let parent = new_t_tmp_dir(t);
        // "café" + "résumé"; both precomposed NFC.
        let dir = join(&parent, "caf\u{00e9}-dir");
        mkdir(&dir);
        let (r, _) = subscribe_for(t, &dir, wi);
        let child = join(&dir, "r\u{00e9}sum\u{00e9}.txt");
        write_file(&child, "hi");
        expect_event_sequence(&r, &[w(UPDATE, &child)]);
    });
}

// Go: watcher_test.go:984 TestSubscribeDirRename
#[test]
fn test_subscribe_dir_rename() {
    run_for_each_watcher("TestSubscribeDirRename", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = sub_path(&dir);
        mkdir(&f1);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f2 = sub_path(&dir);
        rename(&f1, &f2);
        expect_event_set(&r, &[w(DELETE, &f1), w(UPDATE, &f2)]);
    });
}

// Go: watcher_test.go:1004 TestSubscribeDirDelete
#[test]
fn test_subscribe_dir_delete() {
    run_for_each_watcher("TestSubscribeDirDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        mkdir(&f);
        let (r, _) = subscribe_for(t, &dir, wi);
        remove_all(&f);
        expect_event_sequence(&r, &[w(DELETE, &f)]);
    });
}

// Go: watcher_test.go:1020 TestSubscribeWatchedDirDeleted
#[test]
fn test_subscribe_watched_dir_deleted() {
    run_for_each_watcher("TestSubscribeWatchedDirDeleted", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        remove_all(&dir);
        expect_event_sequence(&r, &[w(DELETE, &dir)]);

        let deadline = Instant::now() + r.deadline();
        while Instant::now() < deadline {
            if r.err_count() > 0 {
                break;
            }
            sleep(ms(20));
        }
        let errs = r.take_errs();
        let saw_terminated = errs.iter().any(|e| errors::is(e, &ERR_WATCH_TERMINATED));
        if !saw_terminated {
            let texts: Vec<String> = errs.iter().map(GoError::error).collect();
            fatal(format!(
                "expected ErrWatchTerminated after watched dir delete, got errs={texts:?}"
            ));
        }

        // Re-create; should not emit events for a now-stale watch.
        mkdir_all(&dir);
        let extra = r.drain_quiet(ms(200));
        if !extra.is_empty() {
            fatal(format!("expected no follow-up events, got {extra:?}"));
        }
    });
}

// ----- sub-files ---------------------------------------------------------

// Go: watcher_test.go:1071 TestSubscribeSubfileCreate
#[test]
fn test_subscribe_subfile_create() {
    run_for_each_watcher("TestSubscribeSubfileCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let sub = sub_path(&dir);
        mkdir(&sub);
        expect_contains(&r, UPDATE, &sub);
        sleep(ms(100));
        let f = sub_path(&sub);
        write_file(&f, "hi");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:1094 TestSubscribeSubfileUpdate
#[test]
fn test_subscribe_subfile_update() {
    run_for_each_watcher("TestSubscribeSubfileUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = sub_path(&dir);
        mkdir(&sub);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&sub);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true);
        write_file(&f, "v2-longer");
        expect_contains(&r, UPDATE, &f);
    });
}

// Go: watcher_test.go:1117 TestSubscribeSubfileRename
#[test]
fn test_subscribe_subfile_rename() {
    run_for_each_watcher("TestSubscribeSubfileRename", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = sub_path(&dir);
        mkdir(&sub);
        let f1 = sub_path(&sub);
        write_file(&f1, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        let f2 = sub_path(&sub);
        rename(&f1, &f2);
        let want = [w(DELETE, &f1), w(UPDATE, &f2)];
        let got = r.wait_for_all(r.deadline(), &want);
        let filtered = filter_events_for_paths(&got, &[&f1, &f2]);
        assert_event_set(&filtered, &want);
    });
}

// Go: watcher_test.go:1142 TestSubscribeSubfileDelete
#[test]
fn test_subscribe_subfile_delete() {
    run_for_each_watcher("TestSubscribeSubfileDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = sub_path(&dir);
        mkdir(&sub);
        let f = sub_path(&sub);
        write_file(&f, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove(&f);
        let want = [w(DELETE, &f)];
        let got = r.wait_for_all(r.deadline(), &want);
        let filtered = filter_events_for_paths(&got, &[&f]);
        assert_event_sequence(&filtered, &want);
    });
}

// ----- sub-directories ---------------------------------------------------

// Go: watcher_test.go:1167 TestSubscribeSubdirCreate
#[test]
fn test_subscribe_subdir_create() {
    run_for_each_watcher("TestSubscribeSubdirCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = sub_path(&dir);
        mkdir(&sub);
        let (r, _) = subscribe_for(t, &dir, wi);
        let nested = sub_path(&sub);
        mkdir(&nested);
        let want = [w(UPDATE, &nested)];
        let got = r.wait_for_all(r.deadline(), &want);
        let filtered = filter_events_for_paths(&got, &[&nested]);
        assert_event_sequence(&filtered, &want);
    });
}

// Go: watcher_test.go:1187 TestSubscribeSubdirDeleteWithFiles
#[test]
fn test_subscribe_subdir_delete_with_files() {
    run_for_each_watcher("TestSubscribeSubdirDeleteWithFiles", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub_dir = sub_path(&dir);
        mkdir(&sub_dir);
        let child = sub_path(&sub_dir);
        write_file(&child, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove_all(&sub_dir);
        expect_event_set(&r, &[w(DELETE, &sub_dir), w(DELETE, &child)]);
    });
}

// ----- symlinks ----------------------------------------------------------

// Go: watcher_test.go:1212 TestSubscribeSymlinkCreate
#[test]
fn test_subscribe_symlink_create() {
    if std::env::consts::OS == "dragonfly" {
        println!("SKIP: DragonFlyBSD kqueue doesn't fire NOTE_WRITE on symlink creation");
        return;
    }
    run_for_each_watcher("TestSubscribeSymlinkCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = sub_path(&dir);
        write_file(&f1, "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        let f2 = sub_path(&dir);
        symlink(&f1, &f2);
        expect_event_sequence(&r, &[w(UPDATE, &f2)]);
    });
}

// Go: watcher_test.go:1232 TestSubscribeSymlinkDelete
#[test]
fn test_subscribe_symlink_delete() {
    run_for_each_watcher("TestSubscribeSymlinkDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = sub_path(&dir);
        let f2 = sub_path(&dir);
        write_file(&f1, "x");
        symlink(&f1, &f2);
        let (r, _) = subscribe_for(t, &dir, wi);
        remove(&f2);
        expect_event_sequence(&r, &[w(DELETE, &f2)]);
    });
}

// ----- event coalescing --------------------------------------------------

// Go: watcher_test.go:1314 TestSubscribeCoalesceCreateUpdate
#[test]
fn test_subscribe_coalesce_create_update() {
    run_for_each_watcher("TestSubscribeCoalesceCreateUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        write_file(&f, "v2");
        let got = r.gather_until_quiet(r.deadline(), MAX_WAIT_TIME * 3);
        let net = replay_event_list(&filter_events_for_paths(&got, &[&f]));
        assert_event_set(&net, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:1336 TestSubscribeCoalesceDeleteCreateAsUpdate
#[test]
fn test_subscribe_coalesce_delete_create_as_update() {
    run_for_each_watcher("TestSubscribeCoalesceDeleteCreateAsUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true);
        remove(&f);
        write_file(&f, "v2");
        let got = r.gather_until_quiet(r.deadline(), MAX_WAIT_TIME * 3);
        let net = replay_event_list(&filter_events_for_paths(&got, &[&f]));
        assert_event_set(&net, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:1359 TestSubscribeCoalesceCreateThenDelete
#[test]
fn test_subscribe_coalesce_create_then_delete() {
    run_for_each_watcher("TestSubscribeCoalesceCreateThenDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f1 = sub_path(&dir);
        let f2 = sub_path(&dir);
        write_file(&f1, "x");
        write_file(&f2, "x");
        remove(&f2);
        let got = r.gather_until_quiet(r.deadline(), MAX_WAIT_TIME * 3);
        let net = replay_event_list(&got);
        assert_event_set(&net, &[w(UPDATE, &f1)]);
    });
}

// Go: watcher_test.go:1389 TestSubscribeCoalesceMultipleUpdates
#[test]
fn test_subscribe_coalesce_multiple_updates() {
    run_for_each_watcher("TestSubscribeCoalesceMultipleUpdates", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true); // consume initial update
        for v in ["v2", "v3", "v4"] {
            write_file(&f, v);
        }
        let got = r.gather_until_quiet(r.deadline(), MAX_WAIT_TIME * 3);
        let net = replay_event_list(&filter_events_for_paths(&got, &[&f]));
        assert_event_set(&net, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:1410 TestSubscribeCoalesceUpdateDelete
#[test]
fn test_subscribe_coalesce_update_delete() {
    run_for_each_watcher("TestSubscribeCoalesceUpdateDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true);
        write_file(&f, "v2");
        remove(&f);
        let got = r.gather_until_quiet(r.deadline(), MAX_WAIT_TIME * 3);
        let net = replay_event_list(&filter_events_for_paths(&got, &[&f]));
        assert_event_set(&net, &[w(DELETE, &f)]);
    });
}

// ----- multiple subscriptions --------------------------------------------

// Go: watcher_test.go:1438 TestSubscribeMultipleSameDir
#[test]
fn test_subscribe_multiple_same_dir() {
    run_for_each_watcher("TestSubscribeMultipleSameDir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        sleep(ms(50));

        let r1 = Recorder::new(t);
        let s1: Arc<dyn Watch> = Arc::from(
            wi.watch_directory(&dir, r1.callback(), &[])
                .unwrap_or_else(|e| fatal(e.error())),
        );
        let c1 = s1.clone();
        t.cleanup(move || {
            let _ = c1.close();
        });

        let r2 = Recorder::new(t);
        let s2: Arc<dyn Watch> = Arc::from(
            wi.watch_directory(&dir, r2.callback(), &[])
                .unwrap_or_else(|e| fatal(e.error())),
        );
        let c2 = s2.clone();
        t.cleanup(move || {
            let _ = c2.close();
        });

        sleep(ms(100));
        let f = sub_path(&dir);
        write_file(&f, "hi");
        assert_event_sequence(&r1.next(r1.deadline()), &[w(UPDATE, &f)]);
        assert_event_sequence(&r2.next(r2.deadline()), &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:1471 TestSubscribeMultipleDifferentDirs
#[test]
fn test_subscribe_multiple_different_dirs() {
    run_for_each_watcher("TestSubscribeMultipleDifferentDirs", |t, wi| {
        let dir1 = new_t_tmp_dir(t);
        let dir2 = new_t_tmp_dir(t);
        let (r1, _) = subscribe_for(t, &dir1, wi);
        let (r2, _) = subscribe_for(t, &dir2, wi);
        let f1 = sub_path(&dir1);
        let f2 = sub_path(&dir2);
        write_file(&f1, "a");
        write_file(&f2, "b");
        assert_event_sequence(&r1.next(r1.deadline()), &[w(UPDATE, &f1)]);
        assert_event_sequence(&r2.next(r2.deadline()), &[w(UPDATE, &f2)]);
    });
}

// Go: watcher_test.go:1493 TestWatchDirectoriesBatch
#[test]
fn test_watch_directories_batch() {
    run_for_each_watcher("TestWatchDirectoriesBatch", |t, wi| {
        let dir1 = new_t_tmp_dir(t);
        let dir2 = new_t_tmp_dir(t);
        let r1 = Recorder::for_watcher(t, wi);
        let r2 = Recorder::for_watcher(t, wi);

        let opts1: Vec<Box<dyn WatchOption>> = vec![with_recursive()];
        let opts2: Vec<Box<dyn WatchOption>> = vec![with_recursive()];
        let watches = wi
            .watch_directories(&[
                fswatch::WatchDirectoryRequest {
                    dir: dir1.clone(),
                    callback: r1.callback(),
                    options: &opts1,
                },
                fswatch::WatchDirectoryRequest {
                    dir: dir2.clone(),
                    callback: r2.callback(),
                    options: &opts2,
                },
            ])
            .unwrap_or_else(|e| fatal(e.error()));
        t.cleanup(move || {
            for watch in &watches {
                let _ = watch.close();
            }
        });
        std::thread::sleep(settle_sleep(wi));

        let f1 = sub_path(&dir1);
        let f2 = sub_path(&dir2);
        write_file(&f1, "a");
        write_file(&f2, "b");
        assert_event_sequence(&r1.next(r1.deadline()), &[w(UPDATE, &f1)]);
        assert_event_sequence(&r2.next(r2.deadline()), &[w(UPDATE, &f2)]);
    });
}

// ----- errors ------------------------------------------------------------

// Go: watcher_test.go:1810 TestSubscribeMissingDirError
#[test]
fn test_subscribe_missing_dir_error() {
    run_for_each_watcher("TestSubscribeMissingDirError", |t, wi| {
        let bogus = join(&new_t_tmp_dir(t), "definitely-not-here");
        if wi.watch_directory(&bogus, noop_callback(), &[]).is_ok() {
            fatal("expected error subscribing to non-existent dir");
        }
    });
}

// Go: watcher_test.go:1821 TestSubscribeNotADirError
#[test]
fn test_subscribe_not_a_dir_error() {
    run_for_each_watcher("TestSubscribeNotADirError", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        write_file(&f, "x");
        if wi.watch_directory(&f, noop_callback(), &[]).is_ok() {
            fatal("expected error subscribing to a file");
        }
    });
}

// Go: watcher_test.go:1845 TestSubscribeRejectsRelativePath
#[test]
fn test_subscribe_rejects_relative_path() {
    run_for_each_watcher("TestSubscribeRejectsRelativePath", |_, wi| {
        if wi
            .watch_directory("relative/path", noop_callback(), &[])
            .is_ok()
        {
            fatal("WatchDirectory with relative path should return an error");
        }
        if wi
            .watch_file("relative/path/file.txt", noop_callback())
            .is_ok()
        {
            fatal("WatchFile with relative path should return an error");
        }
    });
}

// ----- watch lifecycle --------------------------------------------

// Go: watcher_test.go:1861 TestSubscribeUnsubscribeIdempotent
#[test]
fn test_subscribe_unsubscribe_idempotent() {
    run_for_each_watcher("TestSubscribeUnsubscribeIdempotent", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let r = Recorder::new(t);
        let sub = wi
            .watch_directory(&dir, r.callback(), &[])
            .unwrap_or_else(|e| fatal(e.error()));
        if let Err(e) = sub.close() {
            fatal(e.error());
        }
        if let Err(e) = sub.close() {
            fatal(format!("second Close should be a no-op, got {}", e.error()));
        }
    });
}

// Go: watcher_test.go:1888 TestSubscribeCloseThenReSubscribe
#[test]
fn test_subscribe_close_then_re_subscribe() {
    run_for_each_watcher("TestSubscribeCloseThenReSubscribe", |t, wi| {
        let dir = new_t_tmp_dir(t);

        let r1 = Recorder::new(t);
        let s1 = wi
            .watch_directory(&dir, r1.callback(), &[])
            .unwrap_or_else(|e| fatal(e.error()));
        if let Err(e) = s1.close() {
            fatal(e.error());
        }

        let r2 = Recorder::new(t);
        let s2: Arc<dyn Watch> = Arc::from(
            wi.watch_directory(&dir, r2.callback(), &[])
                .unwrap_or_else(|e| fatal(format!("re-WatchDirectory after Close: {}", e.error()))),
        );
        let c2 = s2.clone();
        t.cleanup(move || {
            let _ = c2.close();
        });

        // Give the second watcher a moment to settle (fsevents/kqueue
        // need it; inotify/fanotify/Windows don't but the wait is cheap).
        if is_kqueue_or_fsevents(wi) {
            sleep(ms(300));
        } else {
            sleep(ms(60));
        }

        let f = sub_path(&dir);
        write_file(&f, "hi");
        expect_event_sequence(&r2, &[w(UPDATE, &f)]);

        let stale = r1.drain_quiet(ms(50));
        if !stale.is_empty() {
            fatal(format!(
                "closed watch saw events: {:?}",
                to_want_events(&stale)
            ));
        }
    });
}

const THREAD_CHILD_TEST: &str = "units_platform::fswatch_watcher::no_thread_leak_child";
const THREAD_CHILD_ENV: &str = "S2_FSWATCH_THREAD_CHILD";

/// The number of threads of this process (Go `runtime.NumGoroutine()`).
#[cfg(target_os = "linux")]
fn num_threads() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, |d| d.count())
}

/// The number of threads of this process (Go `runtime.NumGoroutine()`).
/// PORT: there is no `/proc` outside Linux; `ps -M` prints one header row
/// and one row per thread (macOS and the BSDs).
#[cfg(not(target_os = "linux"))]
fn num_threads() -> usize {
    std::process::Command::new("ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .map_or(0, |out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .count()
                .saturating_sub(1)
        })
}

/// Child entry of `TestSubscribeNoGoroutineLeak`. Does nothing unless
/// `S2_FSWATCH_THREAD_CHILD` is set.
#[test]
fn no_thread_leak_child() {
    if std::env::var_os(THREAD_CHILD_ENV).is_none() {
        return;
    }
    for b in available_watchers() {
        let (_tmp, dir) = new_tmp_dir();
        let dir = dir.to_str().unwrap().to_string();
        let t = T::new(1);
        // Warm up: trigger any lazy singleton init (backend, debouncer).
        let warmup = b
            .watch_directory(&dir, noop_callback(), &[])
            .unwrap_or_else(|e| fatal(e.error()));
        warmup.close().unwrap_or_else(|e| fatal(e.error()));
        sleep(ms(100));

        let baseline = num_threads();
        for _ in 0..8 {
            let r = Recorder::new(&t);
            let sub = b
                .watch_directory(&dir, r.callback(), &[])
                .unwrap_or_else(|e| fatal(e.error()));
            sub.close().unwrap_or_else(|e| fatal(e.error()));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut ok = false;
        while Instant::now() < deadline {
            if num_threads() <= baseline + 2 {
                ok = true;
                break;
            }
            sleep(ms(50));
        }
        assert!(
            ok,
            "{}: thread leak: baseline={baseline} now={}",
            b.name(),
            num_threads()
        );
    }
}

// Go: watcher_test.go:1934 TestSubscribeNoGoroutineLeak
// PORT: Go counts goroutines with the test run alone (no `t.Parallel`).
// libtest runs tests in parallel, so the count runs in a child process of
// this test binary with one test thread; a thread stands for a goroutine.
#[test]
fn test_subscribe_no_goroutine_leak() {
    let exe = super::self_exe();
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            THREAD_CHILD_TEST,
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(THREAD_CHILD_ENV, "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run thread leak child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "thread leak child failed ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

// ----- additional coverage -----------------------------------------------

// Go: watcher_test.go:1978 TestSubscribeDeepNestedCreate
#[test]
fn test_subscribe_deep_nested_create() {
    run_for_each_watcher("TestSubscribeDeepNestedCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        let a = join(&dir, "a");
        let b = join(&a, "b");
        let c = join(&b, "c");
        for d in [&a, &b, &c] {
            mkdir(d);
            sleep(ms(150));
        }
        let f = join(&c, "deep.txt");
        write_file(&f, "deep");
        let want = [w(UPDATE, &a), w(UPDATE, &f)];
        let got = r.wait_for_all(r.deadline(), &want);
        for (k, p) in &want {
            if !contains_event(&got, *k, p) {
                fatal(format!(
                    "expected {} for {p}, got {:?}",
                    k.string(),
                    to_want_events(&got)
                ));
            }
        }
    });
}

// Go: watcher_test.go:2008 TestSubscribeManyFilesAtOnce
#[test]
fn test_subscribe_many_files_at_once() {
    run_for_each_watcher("TestSubscribeManyFilesAtOnce", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for(t, &dir, wi);
        const COUNT: usize = 50;
        let paths: Vec<String> = (0..COUNT)
            .map(|_| {
                let p = sub_path(&dir);
                write_file(&p, "x");
                p
            })
            .collect();
        let want: Vec<W> = paths.iter().map(|p| w(UPDATE, p)).collect();
        let mut got = r.wait_for_all(r.deadline(), &want);
        let mut attempt = 0;
        while attempt < 3 && !have_all(&got, &want) {
            for p in &paths {
                if !contains_event(&got, UPDATE, p) {
                    let _ = std::fs::write(p, "x");
                }
            }
            got.extend(r.wait_for_all(r.deadline(), &want));
            attempt += 1;
        }
        for p in &paths {
            if !contains_event(&got, UPDATE, p) {
                fatal(format!(
                    "missing create for {p} (got {} events total)",
                    got.len()
                ));
            }
        }
    });
}

// Go: watcher_test.go:2049 TestSubscribeTruncateFile
#[test]
fn test_subscribe_truncate_file() {
    run_for_each_watcher("TestSubscribeTruncateFile", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        write_file(&f, "hello world");
        let (r, _) = subscribe_for(t, &dir, wi);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&f)
            .and_then(|fh| fh.set_len(0))
            .unwrap_or_else(|e| fatal(format!("Truncate: {e}")));
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2066 TestSubscribeConcurrentSubscribeUnsubscribe
#[test]
fn test_subscribe_concurrent_subscribe_unsubscribe() {
    run_for_each_watcher("TestSubscribeConcurrentSubscribeUnsubscribe", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let attempt = t.attempt;
        std::thread::scope(|s| {
            for _ in 0..8 {
                let dir = &dir;
                s.spawn(move || {
                    let t = T::new(attempt);
                    let rec = Recorder::new(&t);
                    if let Ok(sub) = wi.watch_directory(dir, rec.callback(), &[]) {
                        let _ = sub.close();
                    }
                });
            }
        });
    });
}

// Go: watcher_test.go:2088 TestSubscribeRenameDir
#[test]
fn test_subscribe_rename_dir() {
    run_for_each_watcher("TestSubscribeRenameDir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = join(&dir, "before");
        mkdir(&sub);
        write_file(&join(&sub, "file.txt"), "x");
        let (r, _) = subscribe_for(t, &dir, wi);
        let after = join(&dir, "after");
        rename(&sub, &after);
        let want = [w(UPDATE, &after), w(DELETE, &sub)];
        let got = r.wait_for_all(r.deadline(), &want);
        for (k, p) in &want {
            if !contains_event(&got, *k, p) {
                fatal(format!(
                    "expected {} for {p}, got {:?}",
                    k.string(),
                    to_want_events(&got)
                ));
            }
        }
    });
}

// Go: watcher_test.go:2116 TestSubscribeReplaceFileWithDir
#[test]
fn test_subscribe_replace_file_with_dir() {
    run_for_each_watcher("TestSubscribeReplaceFileWithDir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let target = sub_path(&dir);
        write_file(&target, "file");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove(&target);
        mkdir(&target);
        let got = r.wait_for_event(r.deadline(), |e| e.path == target);
        if !contains_event(&got, DELETE, &target) && !contains_event(&got, UPDATE, &target) {
            fatal(format!(
                "expected events for file-to-dir replacement, got {:?}",
                to_want_events(&got)
            ));
        }
    });
}

// Go: watcher_test.go:2142 TestSubscribeAppendToFile
#[test]
fn test_subscribe_append_to_file() {
    run_for_each_watcher("TestSubscribeAppendToFile", |t, wi| {
        use std::io::Write as _;
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        write_file(&f, "initial");
        let (r, _) = subscribe_for(t, &dir, wi);
        let mut fh = std::fs::OpenOptions::new()
            .append(true)
            .open(&f)
            .unwrap_or_else(|e| fatal(e.to_string()));
        let _ = fh.write_all(b" appended");
        drop(fh);
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2163 TestSubscribeNoEventsAfterUnsubscribe
#[test]
fn test_subscribe_no_events_after_unsubscribe() {
    run_for_each_watcher("TestSubscribeNoEventsAfterUnsubscribe", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, sub) = subscribe_for(t, &dir, wi);
        if let Err(e) = sub.close() {
            fatal(e.error());
        }
        let f = sub_path(&dir);
        write_file(&f, "x");
        let got = r.drain_quiet(ms(500));
        if !got.is_empty() {
            fatal(format!(
                "expected no events after closeWatch, got {:?}",
                to_want_events(&got)
            ));
        }
    });
}

// ----- watcherBase / dirWatchError internals -----------------------------

// Go: watcher_test.go:2185 failingBackend
struct FailingBackend {
    base: WatcherBase,
    err: GoError,
}

// Go: watcher_test.go:2190 newFailingBackend
fn new_failing_backend(err: GoError) -> Arc<FailingBackend> {
    Arc::new_cyclic(|self_: &Weak<FailingBackend>| {
        let b = FailingBackend {
            base: WatcherBase::default(),
            err,
        };
        let self_impl: Weak<dyn WatcherImpl> = self_.clone();
        b.base.init(self_impl);
        b
    })
}

impl WatcherImpl for FailingBackend {
    fn start(&self) -> Result<(), GoError> {
        Err(self.err.clone())
    }

    fn subscribe(&self, _: &Arc<DirWatch>) -> Result<(), GoError> {
        Ok(())
    }

    fn close_watch(&self, _: &Arc<DirWatch>) -> Result<(), GoError> {
        Ok(())
    }

    fn base(&self) -> &WatcherBase {
        &self.base
    }
}

// Go: watcher_test.go:2206 TestBackendRunReturnsStartError
#[test]
fn test_backend_run_returns_start_error() {
    let want = errors::new("startup failed");
    let b = new_failing_backend(want.clone());
    match b.run() {
        Err(err) => assert!(
            errors::is(&err, &want),
            "run() error = {}, want {}",
            err.error(),
            want.error()
        ),
        Ok(()) => panic!("run() error = nil, want {}", want.error()),
    }
}

// Go: watcher_test.go:2215 TestDirWatchErrorImplementsError
// PORT: Go leaves `dirWatch` nil; the Rust field is an `Arc`, so it holds a
// fresh dirWatch.
#[test]
fn test_dir_watch_error_implements_error() {
    let dw = new_dir_watch(
        "/unused",
        "/unused",
        new_debounce(),
        false,
        PathComparer::default(),
    );
    let err = DirWatchError {
        err: errors::new("boom"),
        dir_watch: dw.clone(),
    }
    .to_go_error();
    assert_eq!(err.error(), "boom");
    dw.destroy_debounce();
}

// Go: watcher_test.go:2223 TestFileCallbackForwardsErrAlongsideEvents
// ts#64210: the file filter is a dirWatch callback option (Go `addCallback`
// with a file), not the removed `fileCallback` wrapper. `cb` feeds the
// events through the dirWatch event list.
#[test]
fn test_file_callback_forwards_err_alongside_events() {
    let target = "/abs/dir/target.txt";
    let other = "/abs/dir/sibling.txt";
    let overflow = errors::new("overflow");

    type Call = (Vec<Event>, Option<GoError>);
    let got: Arc<Mutex<Vec<Call>>> = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    let t = T::new(1);
    let dw = new_direct_watcher(&t, "/abs/dir");
    dw.add_callback(
        "/abs/dir",
        "/abs/dir",
        false,
        Arc::new(move |events, err| g.lock().unwrap().push((events, err))),
        None,
        target,
    );
    let cb = |events: Vec<Event>, err: Option<GoError>| {
        for e in &events {
            if e.kind == DELETE {
                dw.events.remove(&e.path);
            } else {
                dw.events.update(&e.path);
            }
        }
        if let Some(err) = err {
            dw.events.set_error(err);
        }
        dw.trigger_callbacks();
    };
    let ev = |kind, path: &str| Event {
        kind,
        path: path.to_string(),
    };

    // Plain events: only target events pass through, sibling dropped.
    cb(vec![ev(UPDATE, target), ev(UPDATE, other)], None);
    {
        let g = got.lock().unwrap();
        assert!(
            g.len() == 1 && g[0].0.len() == 1 && g[0].0[0].path == target && g[0].1.is_none(),
            "plain delivery: got {:?}",
            g.iter().map(|c| &c.0).collect::<Vec<_>>()
        );
    }

    // Err only, no matching events: still forwarded with empty slice.
    got.lock().unwrap().clear();
    cb(vec![ev(UPDATE, other)], Some(overflow.clone()));
    {
        let g = got.lock().unwrap();
        assert!(
            g.len() == 1
                && g[0].0.is_empty()
                && g[0].1.as_ref().is_some_and(|e| errors::is(e, &overflow)),
            "err-only delivery"
        );
    }

    // Err with matching events: deliver both the filtered events and err.
    got.lock().unwrap().clear();
    cb(
        vec![ev(DELETE, target), ev(UPDATE, other)],
        Some(overflow.clone()),
    );
    {
        let g = got.lock().unwrap();
        assert!(
            g.len() == 1
                && g[0].0.len() == 1
                && g[0].0[0].path == target
                && g[0].0[0].kind == DELETE
                && g[0].1.as_ref().is_some_and(|e| errors::is(e, &overflow)),
            "combined delivery"
        );
    }

    // No events, no err: callback not invoked at all.
    got.lock().unwrap().clear();
    cb(Vec::new(), None);
    assert!(got.lock().unwrap().is_empty(), "no-op delivery");
    t.finish();
}

// Go: watcher_test.go:2291 TestRenameDirOutOfTreeNoStaleEvents
#[test]
fn test_rename_dir_out_of_tree_no_stale_events() {
    run_for_each_watcher("TestRenameDirOutOfTreeNoStaleEvents", |t, wi| {
        let watched = new_t_tmp_dir(t);
        let outside = new_t_tmp_dir(t);
        let sub = join(&watched, "sub");
        let inner = join(&sub, "inner");
        mkdir_all(&inner);
        write_file(&join(&inner, "leaf.txt"), "v1");

        let (r, _) = subscribe_for_opts(t, &watched, wi, vec![with_recursive()]);

        let dest = join(&outside, "moved");
        rename(&sub, &dest);
        let _ = r.drain_quiet(ms(500));

        let moved_nested = join(&join(&dest, "inner"), "leaf.txt");
        write_file(&moved_nested, "v2-longer");

        let extra = r.drain_quiet(ms(800));
        let old_prefix = format!("{sub}{}", std::path::MAIN_SEPARATOR);
        for e in &extra {
            if e.path == sub || e.path.starts_with(&old_prefix) {
                fatal(format!(
                    "stale event for moved-out path {}: {e:?}\nall extras: {:?}",
                    e.path,
                    to_want_events(&extra)
                ));
            }
        }
    });
}

// ----- platform-specific -------------------------------------------------

// Go: watcher_test.go:2344 TestDefaultBackendMatchesPlatform
// PORT: Go wants "fsevents" on darwin. `fswatch::default` picks FSEvents
// when that backend is available and kqueue (Go's fallback) when it is
// not, so the port wants the backend that is available.
#[test]
fn test_default_backend_matches_platform() {
    let d = fswatch::default();
    let os = std::env::consts::OS;
    let want_name = match os {
        "linux" => {
            if fswatch::fanotify().available() {
                "fanotify"
            } else {
                "inotify"
            }
        }
        "android" => "inotify",
        "macos" => "fsevents",
        "windows" => "windows",
        "freebsd" | "openbsd" | "netbsd" | "dragonfly" => "kqueue",
        _ => {
            println!("SKIP: no expected default watcher for {os}");
            return;
        }
    };
    assert!(d.available(), "Default() should be available on {os}");
    assert_eq!(d.name(), want_name, "Default().Name()");
}

// Go: watcher_test.go:2374 TestUnavailableBackendReturnsError
#[test]
fn test_unavailable_backend_returns_error() {
    let Some(unavailable) = fswatch::all_watchers().into_iter().find(|w| !w.available()) else {
        println!("SKIP: all watchers are available on this platform");
        return;
    };
    let (_tmp, dir) = new_tmp_dir();
    match unavailable.watch_directory(dir.to_str().unwrap(), noop_callback(), &[]) {
        Err(err) => assert!(
            errors::is(&err, &ERR_UNAVAILABLE),
            "expected ErrUnavailable from {}, got {}",
            unavailable.name(),
            err.error()
        ),
        Ok(_) => panic!(
            "expected ErrUnavailable from {}, got nil",
            unavailable.name()
        ),
    }
}

// Go: watcher_test.go:2394 TestSubscribeNestedDirDeletionCleansDescendants
#[test]
fn test_subscribe_nested_dir_deletion_cleans_descendants() {
    run_for_each_watcher(
        "TestSubscribeNestedDirDeletionCleansDescendants",
        |t, wi| {
            let dir = new_t_tmp_dir(t);
            let sub = join(&dir, "parent");
            let nested = join(&sub, "child");
            mkdir_all(&nested);
            write_file(&join(&nested, "file.txt"), "x");
            let (r, _) = subscribe_for(t, &dir, wi);
            remove_all(&sub);
            expect_contains(&r, DELETE, &sub);
        },
    );
}

// ----- non-recursive tests -----------------------------------------------

// Go: watcher_test.go:2420 TestNonRecursiveFileCreate
#[test]
fn test_non_recursive_file_create() {
    run_for_each_watcher("TestNonRecursiveFileCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let f = sub_path(&dir);
        write_file(&f, "hello");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2434 TestNonRecursiveFileUpdate
#[test]
fn test_non_recursive_file_update() {
    run_for_each_watcher("TestNonRecursiveFileUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let f = sub_path(&dir);
        write_file(&f, "v1");
        let _ = r.wait_for_event(r.deadline(), |_| true); // consume create
        write_file(&f, "v2-longer");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2452 TestNonRecursiveFileDelete
#[test]
fn test_non_recursive_file_delete() {
    run_for_each_watcher("TestNonRecursiveFileDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = sub_path(&dir);
        write_file(&f, "x");
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        remove(&f);
        expect_event_sequence(&r, &[w(DELETE, &f)]);
    });
}

// Go: watcher_test.go:2469 TestNonRecursiveDirCreate
#[test]
fn test_non_recursive_dir_create() {
    run_for_each_watcher("TestNonRecursiveDirCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let sub = sub_path(&dir);
        mkdir(&sub);
        expect_contains(&r, UPDATE, &sub);
    });
}

// Go: watcher_test.go:2483 TestNonRecursiveGrandchildIgnored
#[test]
fn test_non_recursive_grandchild_ignored() {
    run_for_each_watcher("TestNonRecursiveGrandchildIgnored", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = join(&dir, "child");
        mkdir(&sub);
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let grandchild = sub_path(&sub);
        write_file(&grandchild, "deep");
        let marker = sub_path(&dir);
        write_file(&marker, "flush");
        let mut got = expect_contains(&r, UPDATE, &marker);
        got.extend(r.drain_quiet(MAX_WAIT_TIME * 2));
        assert_no_events_for_path(&got, &grandchild, "expected no events for grandchild");
    });
}

// Go: watcher_test.go:2513 TestNonRecursiveNewSubdirContentIgnored
#[test]
fn test_non_recursive_new_subdir_content_ignored() {
    run_for_each_watcher("TestNonRecursiveNewSubdirContentIgnored", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let sub = sub_path(&dir);
        mkdir(&sub);
        expect_contains(&r, UPDATE, &sub);
        let grandchild = sub_path(&sub);
        write_file(&grandchild, "nested");
        let marker = sub_path(&dir);
        write_file(&marker, "flush");
        let mut got = expect_contains(&r, UPDATE, &marker);
        got.extend(r.drain_quiet(MAX_WAIT_TIME * 2));
        assert_no_events_for_path(&got, &grandchild, "expected no events for nested file");
    });
}

// Go: watcher_test.go:2545 TestNonRecursiveAndRecursiveSameDir
#[test]
fn test_non_recursive_and_recursive_same_dir() {
    run_for_each_watcher("TestNonRecursiveAndRecursiveSameDir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = join(&dir, "child");
        mkdir(&sub);
        let (r_non_rec, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let (r_rec, _) = subscribe_for_opts(t, &dir, wi, vec![with_recursive()]);
        let grandchild = sub_path(&sub);
        write_file(&grandchild, "deep");
        let marker = sub_path(&dir);
        write_file(&marker, "flush");
        expect_contains(&r_rec, UPDATE, &grandchild);
        let mut got = expect_contains(&r_non_rec, UPDATE, &marker);
        got.extend(r_non_rec.drain_quiet(MAX_WAIT_TIME * 2));
        assert_no_events_for_path(&got, &grandchild, "non-recursive: expected no events for");
    });
}

// Go: watcher_test.go:2577 TestNonRecursiveWithDeniedSubdir
#[test]
fn test_non_recursive_with_denied_subdir() {
    run_for_each_watcher("TestNonRecursiveWithDeniedSubdir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let denied = join(&dir, "denied");
        mkdir(&denied);
        chmod(&denied, 0);
        let d2 = denied.clone();
        t.cleanup(move || chmod(&d2, 0o700));
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        let f = sub_path(&dir);
        write_file(&f, "hello");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// ----- file watch tests --------------------------------------------------

// Go: watcher_test.go:2608 TestFileWatchCreate
#[test]
fn test_file_watch_create() {
    run_for_each_watcher("TestFileWatchCreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = join(&dir, "target.txt");
        let (r, _) = subscribe_file_for(t, &f, wi);
        write_file(&f, "hello");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2623 TestFileWatchUpdate
#[test]
fn test_file_watch_update() {
    run_for_each_watcher("TestFileWatchUpdate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = join(&dir, "target.txt");
        write_file(&f, "v1");
        let (r, _) = subscribe_file_for(t, &f, wi);
        write_file(&f, "v2-longer");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

// Go: watcher_test.go:2641 TestFileWatchDelete
#[test]
fn test_file_watch_delete() {
    run_for_each_watcher("TestFileWatchDelete", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = join(&dir, "target.txt");
        write_file(&f, "x");
        let (r, _) = subscribe_file_for(t, &f, wi);
        remove(&f);
        expect_event_sequence(&r, &[w(DELETE, &f)]);
    });
}

// Go: watcher_test.go:2659 TestFileWatchIgnoresSiblings
#[test]
fn test_file_watch_ignores_siblings() {
    run_for_each_watcher("TestFileWatchIgnoresSiblings", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let target = join(&dir, "target.txt");
        let sibling = join(&dir, "sibling.txt");
        let (r, _) = subscribe_file_for(t, &target, wi);
        let (witness, _) = subscribe_for_opts(t, &dir, wi, vec![]);
        write_file(&sibling, "noise");
        expect_contains(&witness, UPDATE, &sibling);
        expect_no_buffered_events(&r, "expected no events for sibling");
    });
}

// Go: watcher_test.go:2682 TestFileWatchMultipleSameDir
#[test]
fn test_file_watch_multiple_same_dir() {
    run_for_each_watcher("TestFileWatchMultipleSameDir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f1 = join(&dir, "a.txt");
        let f2 = join(&dir, "b.txt");
        let (r1, _) = subscribe_file_for(t, &f1, wi);
        let (r2, _) = subscribe_file_for(t, &f2, wi);

        write_file(&f1, "hello");
        let got1 = r1.next(r1.deadline());
        assert_event_sequence(&got1, &[w(UPDATE, &f1)]);
        expect_no_buffered_events(&r2, "r2 should not see f1 events");

        write_file(&f2, "world");
        let got2 = r2.next(r2.deadline());
        assert_event_sequence(&got2, &[w(UPDATE, &f2)]);
        expect_no_buffered_events(&r1, "r1 should not see f2 events");
    });
}

// Go: watcher_test.go:2714 TestFileWatchDeleteAndRecreate
#[test]
fn test_file_watch_delete_and_recreate() {
    run_for_each_watcher("TestFileWatchDeleteAndRecreate", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = join(&dir, "config.json");
        write_file(&f, r#"{"v":1}"#);
        let (r, _) = subscribe_file_for(t, &f, wi);
        remove(&f);
        expect_event_sequence(&r, &[w(DELETE, &f)]);
        write_file(&f, r#"{"v":2}"#);
        expect_contains(&r, UPDATE, &f);
    });
}

// Go: watcher_test.go:2738 TestFileWatchNonExistentTarget
#[test]
fn test_file_watch_non_existent_target() {
    run_for_each_watcher("TestFileWatchNonExistentTarget", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let f = join(&dir, "doesnotexist.txt");
        let (r, _) = subscribe_file_for(t, &f, wi);
        write_file(&f, "appeared");
        expect_event_sequence(&r, &[w(UPDATE, &f)]);
    });
}

/// The retry loop of the Go tests that write fresh files under `dir` until
/// one update surfaces (or `total` passes).
fn nudge_until_update(
    r: &Recorder,
    dir: &str,
    prefix: &str,
    total: Duration,
) -> Result<(), Vec<W>> {
    let deadline = Instant::now() + total;
    let mut all_seen = Vec::new();
    let under = format!("{dir}{}", std::path::MAIN_SEPARATOR);
    let mut attempt = 0;
    while Instant::now() < deadline {
        let f = join(dir, &format!("{prefix}-{attempt}.txt"));
        write_file(&f, "hello");
        let more = r.wait_for_event(ms(750), |e| e.kind == UPDATE && e.path.starts_with(&under));
        let found = more
            .iter()
            .any(|e| e.kind == UPDATE && e.path.starts_with(&under));
        all_seen.extend(more);
        if found {
            return Ok(());
        }
        attempt += 1;
    }
    Err(to_want_events(&all_seen))
}

// Go: watcher_test.go:2758 TestRecursiveMoveInPrePopulated
#[test]
fn test_recursive_move_in_pre_populated() {
    run_for_each_watcher("TestRecursiveMoveInPrePopulated", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let outside = new_t_tmp_dir(t);
        mkdir_all(&join(&join(&join(&outside, "a"), "b"), "c"));
        let (r, _) = subscribe_for_opts(t, &dir, wi, vec![with_recursive()]);
        let dest = join(&dir, "tree");
        rename(&outside, &dest);
        let _ = r.drain_quiet(ms(500));
        let nested_dir = join(&join(&join(&dest, "a"), "b"), "c");
        if let Err(seen) = nudge_until_update(&r, &nested_dir, "deep", r.deadline()) {
            fatal(format!(
                "expected update for a file inside moved-in tree (gave up after {:?}), got {seen:?}",
                r.deadline()
            ));
        }
    });
}

// Go: watcher_test.go:2809 TestAtomicSave
#[test]
fn test_atomic_save() {
    run_for_each_watcher("TestAtomicSave", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let target = join(&dir, "config.json");
        write_file(&target, r#"{"v":1}"#);
        let (r, _) = subscribe_for(t, &dir, wi);
        let tmp = format!("{target}.tmp");
        write_file(&tmp, r#"{"v":2}"#);
        rename(&tmp, &target);
        let got = r.wait_for_event(r.deadline(), |e| e.path == target);
        if filter_events_for_paths(&got, &[&target]).is_empty() {
            fatal(format!(
                "expected events for {target} after atomic save, got none"
            ));
        }
    });
}

// Go: watcher_test.go:2839 TestAtomicSaveFileWatch
#[test]
fn test_atomic_save_file_watch() {
    run_for_each_watcher("TestAtomicSaveFileWatch", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let target = join(&dir, "target.txt");
        write_file(&target, "v1");
        let (r, _) = subscribe_file_for(t, &target, wi);
        let tmp = format!("{target}.tmp");
        write_file(&tmp, "v2");
        rename(&tmp, &target);
        let got = r.wait_for_event(r.deadline(), |e| e.path == target);
        if filter_events_for_paths(&got, &[&target]).is_empty() {
            fatal(format!(
                "expected events for {target} after atomic save, got none"
            ));
        }
    });
}

// Go: watcher_test.go:2868 TestReplaceDirWithFile
#[test]
fn test_replace_dir_with_file() {
    run_for_each_watcher("TestReplaceDirWithFile", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let child = join(&dir, "child");
        mkdir(&child);
        let (r, _) = subscribe_for(t, &dir, wi);
        remove(&child);
        write_file(&child, "now a file");
        let got = r.wait_for_event(r.deadline(), |e| e.path == child);
        if filter_events_for_paths(&got, &[&child]).is_empty() {
            fatal(format!(
                "expected events for dir->file replacement at {child}, got none"
            ));
        }
    });
}

// Go: watcher_test.go:2897 TestRecreateSubdirAndModify
#[test]
fn test_recreate_subdir_and_modify() {
    run_for_each_watcher("TestRecreateSubdirAndModify", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = join(&dir, "sub");
        mkdir(&sub);
        write_file(&join(&sub, "file.txt"), "v1");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove_all(&sub);
        let _ = r.drain_quiet(ms(500));
        mkdir(&sub);
        sleep(ms(150));
        if let Err(seen) = nudge_until_update(&r, &sub, "attempt", r.deadline() * 2) {
            fatal(format!(
                "expected update for a file inside recreated sub (gave up after {:?}), got {seen:?}",
                r.deadline() * 2
            ));
        }
    });
}

// Go: watcher_test.go:2961 TestReplaceParentDirWithDifferent
#[test]
fn test_replace_parent_dir_with_different() {
    run_for_each_watcher("TestReplaceParentDirWithDifferent", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let sub = join(&dir, "pkg");
        mkdir_all(&join(&sub, "old"));
        write_file(&join(&join(&sub, "old"), "a.txt"), "a");
        let (r, _) = subscribe_for(t, &dir, wi);
        remove_all(&sub);
        mkdir_all(&join(&sub, "new"));
        write_file(&join(&join(&sub, "new"), "b.txt"), "b");
        let _ = r.drain_quiet(ms(500));
        let new_dir = join(&sub, "new");
        if let Err(seen) = nudge_until_update(&r, &new_dir, "attempt", r.deadline()) {
            fatal(format!(
                "expected update for a file inside replaced tree (gave up after {:?}), got {seen:?}",
                r.deadline()
            ));
        }
    });
}

// Go: watcher_test.go:3016 TestRoundTripRename
#[test]
fn test_round_trip_rename() {
    run_for_each_watcher("TestRoundTripRename", |t, wi| {
        if wi.name() == "kqueue" {
            skip(
                "kqueue fd-based tracking delivers stale delete before parent NOTE_WRITE reconciles",
            );
        }
        let dir = new_t_tmp_dir(t);
        let orig = join(&dir, "data.txt");
        write_file(&orig, "content");
        let (r, _) = subscribe_for(t, &dir, wi);
        let tmp = join(&dir, "data.txt.bak");
        rename(&orig, &tmp);
        rename(&tmp, &orig);
        let got = r.gather_until_quiet(r.deadline(), ms(500));
        let got = filter_events_for_paths(&got, &[&orig]);
        let has_delete = contains_event(&got, DELETE, &orig);
        let has_update = contains_event(&got, UPDATE, &orig);
        if has_delete && !has_update {
            fatal(format!(
                "round-trip rename left a stale delete without recovery for {orig}; events: {:?}",
                to_want_events(&got)
            ));
        }
    });
}

// Go: watcher_test.go:3055 TestRecursiveWithDeniedSubdir
#[test]
fn test_recursive_with_denied_subdir() {
    run_for_each_watcher("TestRecursiveWithDeniedSubdir", |t, wi| {
        let dir = new_t_tmp_dir(t);
        let accessible = join(&dir, "ok");
        mkdir(&accessible);
        let denied = join(&dir, "denied");
        mkdir(&denied);
        chmod(&denied, 0);
        let d2 = denied.clone();
        t.cleanup(move || chmod(&d2, 0o700));
        let (r, _) = subscribe_for(t, &dir, wi);
        let f = join(&accessible, "test.txt");
        write_file(&f, "hello");
        expect_contains(&r, UPDATE, &f);
    });
}

// ----- fanotify_linux_test.go (Linux only, as in Go) ----------------------

// Go: fanotify_linux_test.go:30 TestLinuxFanotifyShutdownBeforeStart
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_shutdown_before_start() {
    new_fanotify_backend(false).shutdown();
}

// Go: fanotify_linux_test.go:35 TestLinuxFanotifyBackendSelection
// PORT: the Go `impl.(*fanotifyBackend)` type assertion is not portable
// (see the module comment); this checks that the backend starts.
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_backend_selection() {
    if !fanotify_available() {
        println!("SKIP: fanotify not available");
        return;
    }
    if let Err(err) = fswatch::FANOTIFY_WATCHER.get_impl() {
        panic!("{}", err.error());
    }
}

// Go: fanotify_linux_test.go:49 TestLinuxFanotifySubscribeCleansUpAfterMarkFailure
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_subscribe_cleans_up_after_mark_failure() {
    let t = T::new(1);
    let dir = new_t_tmp_dir(&t);
    let w = new_direct_watcher(&t, &dir);
    let b = new_fanotify_backend(false);

    let err = match b.subscribe(&w) {
        Err(err) => err,
        Ok(()) => panic!("subscribe error = nil, want *dirWatchError"),
    };
    let werr = errors::as_type::<DirWatchError>(&err)
        .unwrap_or_else(|| panic!("subscribe error = {}, want *dirWatchError", err.error()));
    assert!(
        Arc::ptr_eq(&werr.dir_watch, &w),
        "dirWatchError dirWatch is not the subscribed dirWatch"
    );
    let remaining = b.locked.lock().unwrap().subscriptions.len();
    assert_eq!(remaining, 0, "subscriptions not cleaned up");
    t.finish();
}

// Go: fanotify_linux_test.go:68 TestLinuxFanotifyParseDfidNameRoundTrip
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_parse_dfid_name_round_trip() {
    use ts_goport::fswatch::unix;
    let (_tmp, dir) = new_tmp_dir();
    let dir = dir.to_str().unwrap();
    let handle = match unix::name_to_handle_at(unix::AT_FDCWD, dir, 0) {
        Ok((handle, _)) => handle,
        Err(err) => {
            println!("SKIP: NameToHandleAt not supported: {}", err.error());
            return;
        }
    };
    let mut st = unix::Statfs_t::default();
    unix::statfs(dir, &mut st).unwrap_or_else(|e| panic!("{}", e.error()));
    let key = make_fanotify_handle_key(st.fsid.val, handle.type_(), handle.bytes());
    assert!(!key.handle.is_empty(), "empty handle bytes");
    let (handle2, _) =
        unix::name_to_handle_at(unix::AT_FDCWD, dir, 0).unwrap_or_else(|e| panic!("{}", e.error()));
    let key2 = make_fanotify_handle_key(st.fsid.val, handle2.type_(), handle2.bytes());
    assert_eq!(key, key2, "handle keys differ for same path");
}

// Go: fanotify_linux_test.go:93 TestFanotifyCrossWatcherSameFs
#[cfg(target_os = "linux")]
#[test]
fn test_fanotify_cross_watcher_same_fs() {
    if !fanotify_available() {
        println!("SKIP: fanotify not available");
        return;
    }
    // Modify
    let result = run_with_retry("TestFanotifyCrossWatcherSameFs/Modify", &|t: &T| {
        let fan = fswatch::fanotify();
        let dir_a = new_t_tmp_dir(t);
        let dir_b = new_t_tmp_dir(t);
        let path_a = join(&dir_a, "child");
        let path_b = join(&dir_b, "child");
        for p in [&path_a, &path_b] {
            write_file(p, "initial");
        }
        let (r_a, _) = subscribe_for(t, &dir_a, &fan);
        let (r_b, _) = subscribe_for(t, &dir_b, &fan);

        write_file(&path_a, "changed");
        let got_a = r_a.gather(r_a.deadline(), ms(200));
        assert_event_set(&got_a, &[w(UPDATE, &path_a)]);

        let got_b = r_b.drain_quiet(ms(200));
        if !got_b.is_empty() {
            fatal(format!(
                "watcher B got phantom events: {:?}",
                to_want_events(&got_b)
            ));
        }
    });
    if let Err(err) = result {
        panic!("{err}");
    }
}

// Go: fanotify_linux_test.go:124 TestLinuxFanotifyMaybeWrapUnsupportedFilesystem
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_maybe_wrap_unsupported_filesystem() {
    use ts_goport::fswatch::unix;

    // Errnos that indicate the filesystem cannot support fanotify FID-based
    // watching are tagged with ErrFilesystemUnsupported so higher layers can
    // fall back to inotify (issue #63646).
    for errno in [unix::EOPNOTSUPP, unix::ENODEV] {
        let errno_err = errors::from_value(errno);
        let wrapped = maybe_wrap_unsupported_filesystem(errors::errorf(
            format!("name_to_handle_at: {}", errno_err.error()),
            vec![errno_err.clone()],
        ));
        assert!(
            errors::is(&wrapped, &ERR_FILESYSTEM_UNSUPPORTED),
            "expected {} to be tagged ErrFilesystemUnsupported",
            errno_err.error()
        );
        assert!(
            errors::is(&wrapped, &errno_err),
            "expected wrapped error to still unwrap to {}",
            errno_err.error()
        );
    }

    // Unrelated errnos are returned unchanged.
    let other = maybe_wrap_unsupported_filesystem(errors::from_value(unix::EACCES));
    assert!(
        !errors::is(&other, &ERR_FILESYSTEM_UNSUPPORTED),
        "EACCES should not be tagged ErrFilesystemUnsupported"
    );
}

// Go: fanotify_linux_test.go:147 TestLinuxFanotifyMarkENODEVTagged
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_mark_enodev_tagged() {
    use ts_goport::fswatch::unix;

    // On NTFS mounted via fuseblk, fanotify_mark itself fails with ENODEV
    // ("no such device") — the bare errno, with no name_to_handle_at wrapping
    // (issue #63678). markDir passes that straight to
    // maybeWrapUnsupportedFilesystem, so it must be tagged and drive the inotify
    // fallback just like the EOPNOTSUPP case.
    let err = maybe_wrap_unsupported_filesystem(errors::from_value(unix::ENODEV));
    assert!(
        errors::is(&err, &ERR_FILESYSTEM_UNSUPPORTED),
        "bare ENODEV from fanotify_mark should be tagged ErrFilesystemUnsupported"
    );
    assert!(
        errors::is(&err, &errors::from_value(unix::ENODEV)),
        "tagged error should still unwrap to ENODEV"
    );
}

// Go: fanotify_linux_test.go:164 TestLinuxFanotifyUnsupportedTagSurvivesDirWatchError
/// PORT: Go's `dirWatchError` literal has a nil `dirWatch`; the Rust field
/// is not optional, so the error gets a direct watcher that is never used.
#[cfg(target_os = "linux")]
#[test]
fn test_linux_fanotify_unsupported_tag_survives_dir_watch_error() {
    use ts_goport::fswatch::unix;

    // Closes the loop between maybeWrapUnsupportedFilesystem and the higher
    // layers: the tag must survive the exact wrapping that markDir + subscribe
    // apply (fmt.Errorf with %w, then dirWatchError) so that errors.Is still
    // finds ErrFilesystemUnsupported at the WatchDirectories boundary. Catches a
    // regression such as a %w->%v change or a dropped dirWatchError.Unwrap.
    let eopnotsupp = errors::from_value(unix::EOPNOTSUPP);
    let inner = maybe_wrap_unsupported_filesystem(errors::errorf(
        format!("name_to_handle_at: {}", eopnotsupp.error()),
        vec![eopnotsupp.clone()],
    ));
    let t = T::new(1);
    let sub_err = DirWatchError {
        err: errors::errorf(
            format!("fanotify_mark on '{}' failed: {}", "/x", inner.error()),
            vec![inner],
        ),
        dir_watch: new_direct_watcher(&t, "/x"),
    }
    .to_go_error();

    assert!(
        errors::is(&sub_err, &ERR_FILESYSTEM_UNSUPPORTED),
        "ErrFilesystemUnsupported did not survive dirWatchError wrapping"
    );
    assert!(
        errors::is(&sub_err, &eopnotsupp),
        "underlying errno did not survive dirWatchError wrapping"
    );
    t.finish();
}

// Go: fsevents_darwin_{shared,nfd}_test.go share this Go package.
#[cfg(target_os = "macos")]
#[path = "fswatch_fsevents_darwin.rs"]
mod fsevents_darwin;
