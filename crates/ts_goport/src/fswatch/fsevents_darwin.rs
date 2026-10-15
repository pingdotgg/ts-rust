//! Go: internal/fswatch/fsevents_darwin.go (the macOS FSEvents backend).
//!
//! PORT: D-W1 (no `unsafe` in ts_goport). Go calls CoreFoundation and
//! CoreServices through assembly trampolines (fsevents_darwin_ffi.go), which
//! safe Rust cannot do. The port uses the `notify` crate's `FsEventWatcher`
//! (safe API) as the stream: one `notify` watcher is one FSEventStream over
//! all its paths, on its own run loop thread. The stream set, the routing of
//! events to the logical watches and the error handling follow Go.
//!
//! PORT divergences from Go, from `notify`:
//! - `notify` creates the stream with FileEvents | NoDefer and a latency of
//!   0 (Go: UseCFTypes | FileEvents and 1 ms), on a CFRunLoop thread (Go: a
//!   GCD queue). It does not flush the stream after the start.
//! - `notify` gives no event IDs. The events go in with the event list's own
//!   sequence, as on the other backends, and the watcher has no `sequence`
//!   (Go: FSEventsGetCurrentEventId).
//! - `notify` gives no batches. Go notifies each touched watch once, after
//!   it has routed every record of a callback; the debouncer fires at once
//!   on the first notify after a quiet time, so a notify per event would
//!   deliver a record's first flag alone. The port notifies the touched
//!   watches once no event has come for 1 ms (Go's stream latency), and at
//!   the latest 20 ms after the first event.
//! - `notify` gives each FSEvents record as one event per flag. Go classifies
//!   the whole record: a remove with no create is a delete with no syscall;
//!   a rename, or a remove with a create, is an update when the path exists
//!   (lstat) and a delete when it does not. The port sees one flag at a
//!   time, so every remove and rename checks with lstat. A pure remove of a
//!   path that exists again is an update, where Go reports a delete and then
//!   the update of the next record.
//! - `notify` watches only paths that exist, and it reports no stream create
//!   or start failure. A watch root that is gone when the stream set is made
//!   again is left out, so its watch gets no events until it is made again
//!   (Go keeps the path in the stream). A stream that fails to start leaves
//!   its watches silent, with no error, and the fallback to streams of
//!   `FSEVENTS_PATHS_PER_STREAM` paths does not run in practice.
//! - `notify` aborts the process on an event path that is not UTF-8. APFS and
//!   HFS+ reject such names.
//! - The old streams stop on a thread of their own: `notify` waits for an
//!   idle run loop, which can take long while the tree changes fast (Go waits
//!   only for the callbacks already queued). Their late events reach the
//!   new watches twice, which the event lists merge, and skip the closed
//!   watches, which are marked terminated.
//! - `notify` routes events by the canonical path of each watch root.
//!   `DirWatch::physical_dir` is the root with its symlinks resolved, so the
//!   routing below matches Go's.
//! - The paths compare as bytes, as on Linux: the port has no CoreFoundation
//!   case folding (see PORTING.md, "Not ported").

use crate::fswatch::prelude::*;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use notify::event::{Flag, ModifyKind};
use notify::{EventKind, FsEventWatcher, RecursiveMode, Watcher as _};

use crate::frontend::vfs::osvfs::os_path;
use crate::fswatch::pathcompare::ComparisonPath;
use crate::fswatch::syscall;
use crate::fswatch::walkdir::path_error;
use crate::gostd::errors;

// Go: fsevents_darwin.go:215 errStreamCreateNull (and the other stream
// errors). PORT: `notify` reports no stream create or start failure, so the
// port uses only this one, for a `notify` watcher it cannot make.
pub static ERR_STREAM_CREATE_NULL: LazyLock<GoError> =
    LazyLock::new(|| errors::new("FSEventStreamCreate returned NULL"));

// Go: fsevents_darwin.go:220 errFSEventsUserDropped
pub static ERR_FSEVENTS_USER_DROPPED: LazyLock<GoError> =
    LazyLock::new(|| overflow_error("events were dropped by the FSEvents client"));
// Go: fsevents_darwin.go:221 errFSEventsKernelDropped
pub static ERR_FSEVENTS_KERNEL_DROPPED: LazyLock<GoError> =
    LazyLock::new(|| overflow_error("events were dropped by the kernel"));
// Go: fsevents_darwin.go:222 errFSEventsTooMany
pub static ERR_FSEVENTS_TOO_MANY: LazyLock<GoError> =
    LazyLock::new(|| overflow_error("too many events"));

// PORT: Go `fmt.Errorf("<text>: %w", ErrOverflow)`.
fn overflow_error(text: &str) -> GoError {
    errors::errorf(
        format!("{text}: {}", ERR_OVERFLOW.error()),
        vec![ERR_OVERFLOW.clone()],
    )
}

// PORT: Go `fmt.Errorf("%w: watched directory removed", ErrWatchTerminated)`.
fn watched_directory_removed() -> GoError {
    errors::errorf(
        format!(
            "{}: watched directory removed",
            ERR_WATCH_TERMINATED.error()
        ),
        vec![ERR_WATCH_TERMINATED.clone()],
    )
}

// Go: fsevents_darwin.go:225 fseventsPathsPerStream
pub const FSEVENTS_PATHS_PER_STREAM: usize = 512;

// Go: fsevents_darwin.go:161 fseventsState
#[derive(Default)]
pub struct FseventsState {
    pub terminated: AtomicBool,
}

// Go: fsevents_darwin.go:227 fseventsWatchSnapshot
#[derive(Clone)]
pub struct FseventsWatchSnapshot {
    pub w: Arc<DirWatch>,
    pub state: Arc<FseventsState>,
}

// Go: fsevents_darwin.go:165 fseventsStream
/// PORT: the `notify` watcher is the stream and its callback. Dropping it
/// stops the run loop and joins its thread (Go: teardownStream).
pub struct FseventsStream {
    watcher: Mutex<Option<FsEventWatcher>>,
}

// Go: fsevents_darwin.go:174 fsEventsBackend
pub struct FsEventsBackend {
    pub base: WatcherBase,
    mu: Mutex<FsEventsBackendLocked>,
}

/// PORT: the `fsEventsBackend` fields that Go `b.mu` guards. Go's
/// `map[*dirWatch]*fseventsState` is a list in subscribe order (Go's map
/// order is random; the stream paths are sorted anyway).
#[derive(Default)]
struct FsEventsBackendLocked {
    watches: Vec<FseventsWatchSnapshot>,
    streams: Vec<Arc<FseventsStream>>,
}

// Go: fsevents_darwin.go:182 init
// PORT: Go also sets `fseventsWatcher.sequence`; the port has no event IDs
// (see the file comment).
pub fn init(fsevents_watcher: &mut WatcherStruct) {
    let factory: WatcherFactory = || -> Arc<dyn WatcherImpl> { new_fs_events_backend() };
    fsevents_watcher.factory = Some(factory);
}

// Go: fsevents_darwin.go:187 newFSEventsBackend
pub fn new_fs_events_backend() -> Arc<FsEventsBackend> {
    Arc::new_cyclic(|self_: &std::sync::Weak<FsEventsBackend>| {
        let b = FsEventsBackend {
            base: WatcherBase::default(),
            mu: Mutex::new(FsEventsBackendLocked::default()),
        };
        let self_impl: std::sync::Weak<dyn WatcherImpl> = self_.clone();
        b.base.init(self_impl);
        b
    })
}

// Go: fsevents_darwin.go:201 checkWatcher
pub fn check_watcher(w: &Arc<DirWatch>) -> Result<(), GoError> {
    let dir_watch_error = |err| DirWatchError {
        err,
        dir_watch: w.clone(),
    };
    match std::fs::metadata(os_path(&w.physical_dir)) {
        Err(err) => Err(dir_watch_error(path_error("stat", &w.physical_dir, &err)).to_go_error()),
        Ok(info) if !info.is_dir() => {
            Err(dir_watch_error(errors::from_value(syscall::ENOTDIR)).to_go_error())
        }
        Ok(_) => Ok(()),
    }
}

// Go: fsevents_darwin.go:232 fsEventsBackend.activeWatchesLocked
fn active_watches_locked(l: &FsEventsBackendLocked) -> Vec<FseventsWatchSnapshot> {
    l.watches
        .iter()
        .filter(|watch| !watch.state.terminated.load(Ordering::SeqCst))
        .cloned()
        .collect()
}

// Go: fsevents_darwin.go:247 startFSEventsStreams
/// One stream for all the physical watch roots, sorted; streams of
/// `FSEVENTS_PATHS_PER_STREAM` roots when that one cannot start. `start` is a
/// parameter for the tests, as in Go.
pub fn start_fs_events_streams<S>(
    watches: &[FseventsWatchSnapshot],
    start: impl Fn(&[String], &[FseventsWatchSnapshot]) -> Result<S, GoError>,
) -> Result<Vec<S>, GoError> {
    if watches.is_empty() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<String> = watches.iter().map(|w| w.w.physical_dir.clone()).collect();
    paths.sort();
    paths.dedup();

    if let Ok(stream) = start(&paths, watches) {
        return Ok(vec![stream]);
    }

    let mut streams = Vec::with_capacity(paths.len().div_ceil(FSEVENTS_PATHS_PER_STREAM));
    for chunk in paths.chunks(FSEVENTS_PATHS_PER_STREAM) {
        // PORT: Go stops the streams it started on an error; dropping
        // `streams` stops them.
        streams.push(start(chunk, &watches_for_fs_events_paths(watches, chunk))?);
    }
    Ok(streams)
}

// Go: fsevents_darwin.go:284 watchesForFSEventsPaths
/// The watches whose physical root is in `paths` (sorted).
pub fn watches_for_fs_events_paths(
    watches: &[FseventsWatchSnapshot],
    paths: &[String],
) -> Vec<FseventsWatchSnapshot> {
    watches
        .iter()
        .filter(|watch| paths.binary_search(&watch.w.physical_dir).is_ok())
        .cloned()
        .collect()
}

// Go: fsevents_darwin.go:298 fsEventsBackend.startStream
/// One stream over `paths`, routing to `watches`.
///
/// PORT: Go builds the CFArray of paths and starts the stream on a GCD
/// queue. `paths_mut` adds every path and then starts the stream once. A
/// path that `notify` cannot watch (it is gone) is left out (see the file
/// comment).
fn start_stream(
    paths: &[String],
    watches: &[FseventsWatchSnapshot],
) -> Result<Arc<FseventsStream>, GoError> {
    let routes = watches.to_vec();
    let (touched_tx, touched_rx) = std::sync::mpsc::channel::<Arc<DirWatch>>();
    let handler = move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            for w in fs_events_callback(&routes, &event) {
                let _ = touched_tx.send(w);
            }
        }
    };
    // Notifies the touched watches once no event has come for 1 ms, and at
    // the latest 20 ms after the first. It ends when the stream drops the
    // handler.
    std::thread::spawn(move || {
        use std::time::{Duration, Instant};
        while let Ok(first) = touched_rx.recv() {
            let last = Instant::now() + Duration::from_millis(20);
            let mut touched = vec![first];
            while let Some(wait) = last
                .checked_duration_since(Instant::now())
                .map(|left| left.min(Duration::from_millis(1)))
            {
                let Ok(w) = touched_rx.recv_timeout(wait) else {
                    break;
                };
                if !touched.iter().any(|t| Arc::ptr_eq(t, &w)) {
                    touched.push(w);
                }
            }
            for w in &touched {
                w.notify();
            }
        }
    });
    let mut watcher = FsEventWatcher::new(handler, notify::Config::default())
        .map_err(|_| ERR_STREAM_CREATE_NULL.clone())?;
    let mut batch = watcher.paths_mut();
    for path in paths {
        let _ = batch.add(Path::new(&*os_path(path)), RecursiveMode::Recursive);
    }
    batch.commit().map_err(|_| ERR_STREAM_CREATE_NULL.clone())?;
    Ok(Arc::new(FseventsStream {
        watcher: Mutex::new(Some(watcher)),
    }))
}

// Go: fsevents_darwin.go:383 stopFSEventsStreams
/// PORT: Go's atomic swap makes a second stop a no-op; `take` does that. The
/// streams stop on a thread of their own (see the file comment).
fn stop_fs_events_streams(streams: Vec<Arc<FseventsStream>>) {
    let watchers: Vec<FsEventWatcher> = streams
        .iter()
        .filter_map(|stream| stream.watcher.lock().unwrap().take())
        .collect();
    if !watchers.is_empty() {
        std::thread::spawn(move || drop(watchers));
    }
}

impl FsEventsBackend {
    // Go: fsevents_darwin.go:408 fsEventsBackend.subscribeMany
    fn subscribe_watches(&self, watches_to_add: &[Arc<DirWatch>]) -> Result<(), GoError> {
        if watches_to_add.is_empty() {
            return Ok(());
        }
        let mut added = Vec::with_capacity(watches_to_add.len());
        for w in watches_to_add {
            check_watcher(w)?;
            added.push(FseventsWatchSnapshot {
                w: w.clone(),
                state: Arc::new(FseventsState::default()),
            });
        }

        let watches = {
            let mut l = self.mu.lock().unwrap();
            for watch in &added {
                *watch.w.state.lock().unwrap() = Some(Box::new(watch.state.clone()));
                l.watches.push(watch.clone());
            }
            active_watches_locked(&l)
        };

        let streams = match start_fs_events_streams(&watches, start_stream) {
            Ok(streams) => streams,
            Err(err) => {
                let mut l = self.mu.lock().unwrap();
                for watch in &added {
                    l.watches
                        .retain(|other| !Arc::ptr_eq(&other.state, &watch.state));
                    *watch.w.state.lock().unwrap() = None;
                }
                return Err(DirWatchError {
                    err,
                    dir_watch: watches_to_add[0].clone(),
                }
                .to_go_error());
            }
        };
        self.replace_streams(streams);
        Ok(())
    }

    /// The number of streams.
    // PORT: for the tests, which read Go `b.streams`.
    pub fn stream_count(&self) -> usize {
        self.mu.lock().unwrap().streams.len()
    }

    /// The number of logical watches.
    // PORT: for the tests, which read Go `b.watches`.
    pub fn watch_count(&self) -> usize {
        self.mu.lock().unwrap().watches.len()
    }

    /// Go: `b.streams = streams` under the lock, then the old streams stop
    /// outside it, so a callback cannot deadlock against the teardown.
    fn replace_streams(&self, streams: Vec<Arc<FseventsStream>>) {
        let old = std::mem::replace(&mut self.mu.lock().unwrap().streams, streams);
        stop_fs_events_streams(old);
    }
}

impl WatcherImpl for FsEventsBackend {
    // Go: fsevents_darwin.go:195 fsEventsBackend.start
    fn start(&self) -> Result<(), GoError> {
        self.base.notify_started();
        Ok(())
    }

    // Go: fsevents_darwin.go:404 fsEventsBackend.subscribe
    fn subscribe(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        self.subscribe_watches(std::slice::from_ref(w))
    }

    // Go: fsevents_darwin.go:408 fsEventsBackend.subscribeMany
    fn subscribe_many(&self, watches: &[Arc<DirWatch>]) -> Option<Result<(), GoError>> {
        Some(self.subscribe_watches(watches))
    }

    // Go: fsevents_darwin.go:450 fsEventsBackend.closeWatch
    fn close_watch(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        let state = w.state.lock().unwrap().take();
        let Some(state) = state.and_then(|s| s.downcast::<Arc<FseventsState>>().ok()) else {
            return Ok(());
        };
        state.terminated.store(true, Ordering::SeqCst);

        let watches = {
            let mut l = self.mu.lock().unwrap();
            l.watches.retain(|other| !Arc::ptr_eq(&other.state, &state));
            active_watches_locked(&l)
        };
        let streams = start_fs_events_streams(&watches, start_stream)?;
        self.replace_streams(streams);
        Ok(())
    }

    fn base(&self) -> &WatcherBase {
        &self.base
    }
}

/// How the port classifies one `notify` event (see the file comment).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// ItemRemoved or ItemRenamed: an update when the path exists, else a delete.
    RemovedOrRenamed,
    /// Every other change flag: an update.
    Updated,
}

// Go: fsevents_darwin.go:482 fsEventsCallback
/// Routes one `notify` event to the watches of its stream.
///
/// PORT: called on the stream's run loop thread with one flag of one
/// FSEvents record (Go: one batch of records on the event loop goroutine).
/// It returns the touched watches; `start_stream` notifies them (see the
/// file comment).
pub fn fs_events_callback(
    watches: &[FseventsWatchSnapshot],
    event: &notify::Event,
) -> Vec<Arc<DirWatch>> {
    let change = match event.kind {
        // `notify` reports an unmount as a remove (Go: an update).
        EventKind::Remove(_) if event.info() != Some("mount") => Change::RemovedOrRenamed,
        EventKind::Modify(ModifyKind::Name(_)) => Change::RemovedOrRenamed,
        _ => Change::Updated,
    };
    let mut touched: Vec<Arc<DirWatch>> = Vec::new();
    let mut touch = |w: &Arc<DirWatch>| {
        if !touched.iter().any(|t| Arc::ptr_eq(t, w)) {
            touched.push(w.clone());
        }
    };

    for path in &event.paths {
        // Go: cfStringToNFC. `notify` gives UTF-8 paths.
        let Some(path) = path.to_str() else {
            continue;
        };
        let path = normalize_nfc(path);
        if path.is_empty() {
            continue;
        }

        // Go also routes a MustScanSubDirs record as an update below.
        let change = if event.flag() == Some(Flag::Rescan) {
            let overflow = match event.info() {
                Some("rescan: user dropped") => &ERR_FSEVENTS_USER_DROPPED,
                Some("rescan: kernel dropped") => &ERR_FSEVENTS_KERNEL_DROPPED,
                _ => &ERR_FSEVENTS_TOO_MANY,
            };
            for watch in watches {
                if !watch.state.terminated.load(Ordering::SeqCst)
                    && fs_events_overflow_matches(&watch.w, &path)
                {
                    watch.w.events.set_error((*overflow).clone());
                    touch(&watch.w);
                }
            }
            Change::Updated
        } else {
            change
        };

        let mut path_exists: Option<bool> = None;
        for watch in watches {
            if watch.state.terminated.load(Ordering::SeqCst) {
                continue;
            }
            let w = &watch.w;
            let Some(display_path) = fs_events_display_path(w, &path) else {
                continue;
            };
            let is_root = display_path == w.dir;

            // Skip events for the watched directory itself unless it's been
            // removed. fseventsd reports a change on the watched dir when a
            // child is added or removed; subscribers observe changes *within*
            // the directory, not the dir's own metadata churn.
            if is_root && change == Change::Updated {
                continue;
            }

            let exists = change == Change::Updated
                || *path_exists
                    .get_or_insert_with(|| std::fs::symlink_metadata(&*os_path(&path)).is_ok());
            if exists {
                if is_root {
                    w.events.update_watch_root(&display_path);
                } else {
                    w.events.update(&display_path);
                }
            } else {
                let seq = if is_root {
                    w.events.remove_watch_root_and_get_sequence(&display_path)
                } else {
                    w.events.remove_and_get_sequence(&display_path)
                };
                w.terminate_callbacks_for_deleted_root(
                    &display_path,
                    seq,
                    watched_directory_removed(),
                );
                if is_root {
                    watch.state.terminated.store(true, Ordering::SeqCst);
                    w.events.set_error(watched_directory_removed());
                }
            }
            touch(w);
        }
    }

    touched
}

// Go: fsevents_darwin.go:626 fseventsDisplayPath
/// `raw_path` under the watch's physical root (or its logical root), as a
/// path under the logical root `w.dir`.
pub fn fs_events_display_path(w: &DirWatch, raw_path: &str) -> Option<String> {
    let mut raw = ComparisonPath {
        path: raw_path.to_string(),
        ..Default::default()
    };
    let physical = ComparisonPath {
        path: w.physical_dir.clone(),
        folded: w.physical_dir_fold.clone(),
        ready: !w.physical_dir_fold.is_empty(),
        cache: None,
    };
    if let (path, true) = w.comparer.rebase_prepared(&mut raw, &physical, &w.dir) {
        return Some(path);
    }
    if w.physical_dir != w.dir {
        let logical = ComparisonPath {
            path: w.dir.clone(),
            folded: w.dir_fold.clone(),
            ready: !w.dir_fold.is_empty(),
            cache: None,
        };
        if let (path, true) = w.comparer.rebase_prepared(&mut raw, &logical, &w.dir) {
            return Some(path);
        }
    }
    None
}

// Go: fsevents_darwin.go:643 fseventsOverflowMatches
/// True when the overflow at `raw_path` is in the watched tree or above it.
pub fn fs_events_overflow_matches(w: &DirWatch, raw_path: &str) -> bool {
    let mut raw = ComparisonPath {
        path: raw_path.to_string(),
        ..Default::default()
    };
    let roots = [
        (w.physical_dir.clone(), w.physical_dir_fold.clone()),
        (w.dir.clone(), w.dir_fold.clone()),
    ];
    let count = if w.physical_dir != w.dir { 2 } else { 1 };
    roots[..count].iter().any(|(path, folded)| {
        let mut root = ComparisonPath {
            path: path.clone(),
            folded: folded.clone(),
            ready: !folded.is_empty(),
            cache: None,
        };
        w.comparer.suffix_prepared(&root, &mut raw).1
            || w.comparer.suffix_prepared(&raw, &mut root).1
    })
}
