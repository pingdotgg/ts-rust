//! Go: internal/execute/tsctests/mock_watch_backend.go (the test watch
//! backend that the watch runner and `TestSys` use).
//!
//! A command child sets it as the watch backend of its thread
//! (`watcher::set_test_watch_backend`, see child.rs).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ts_goport::api::to_rooted_path;
use ts_goport::execute::watchmanager::{WatchBackend, WatchDirectoryRequest};
use ts_goport::frontend::tspath;
use ts_goport::fswatch::{self, Event, EventKind};
use ts_goport::gostd::{GoError, errors};

use crate::support::fsbaselineutil::FileChange;

// Go: mock_watch_backend.go:22 MockWatchBackend
/// MockWatchBackend implements watchmanager.WatchBackend for testing. It
/// records all WatchDirectory calls so tests can verify that
/// the correct watches are registered.  Events can be delivered through
/// SendEvents, which routes them only through watches whose paths
/// match, enforcing that tests fail if the wrong watches are set up.
///
/// PORT: Go `mu` guards `Dirs`, so `dirs` is a `Mutex`. Go `Dirs` is a map
/// with random order; the port uses a `BTreeMap`, so `send_events` visits
/// watches in path order. Go sets `DirectoryExists` after
/// `NewMockWatchBackend`; the port sets it before the backend is shared
/// (`TestSys` holds an `Rc<MockWatchBackend>`, the watch manager's backend
/// is an `Rc<dyn WatchBackend>`), so it needs no lock.
pub struct MockWatchBackend {
    pub dirs: Mutex<BTreeMap<String, Arc<MockWatch>>>,
    /// if set, WatchDirectory fails for non-existent dirs
    pub directory_exists: Option<Box<dyn Fn(&str) -> bool>>,
    pub use_case_sensitive_file_names: bool,
}

impl MockWatchBackend {
    // Go: mock_watch_backend.go:32 NewMockWatchBackend
    /// NewMockWatchBackend creates a ready-to-use mock backend.
    pub fn new() -> MockWatchBackend {
        MockWatchBackend {
            dirs: Mutex::new(BTreeMap::new()),
            directory_exists: None,
            use_case_sensitive_file_names: false,
        }
    }

    // Go: mock_watch_backend.go:37 MockWatchBackend.HasWatches
    /// HasWatches reports whether any watches have been registered.
    pub fn has_watches(&self) -> bool {
        !self.dirs.lock().unwrap().is_empty()
    }
}

// Go: mock_watch_backend.go:44 MockWatch
/// MockWatch records a single registered watch.
///
/// PORT: Go `Closed bool` is written by `Close` without the backend lock; it
/// is an `AtomicBool`, because the fswatch `Watch` must be `Send + Sync`.
pub struct MockWatch {
    pub path: String,
    pub callback: fswatch::WatchCallback,
    pub recursive: bool,
    pub ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    pub closed: AtomicBool,
}

impl MockWatch {
    // Go: mock_watch_backend.go:52 MockWatch.Close
    pub fn close(&self) -> Result<(), GoError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// PORT: Go returns the `*MockWatch` itself as the `io.Closer`. The backend
/// also keeps it in `dirs`, so the port shares it through an `Arc` and
/// returns this handle as the `Box<dyn fswatch::Watch>`.
struct MockWatchCloser(Arc<MockWatch>);

impl fswatch::Watch for MockWatchCloser {
    fn close(&self) -> Result<(), GoError> {
        self.0.close()
    }

    fn unexported(&self) {}
}

impl WatchBackend for MockWatchBackend {
    // Go: mock_watch_backend.go:57 MockWatchBackend.WatchDirectory
    fn watch_directory(
        &self,
        dir: &str,
        fn_: fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> Result<Box<dyn fswatch::Watch>, GoError> {
        let closers = self.watch_directories(vec![WatchDirectoryRequest {
            dir: dir.to_string(),
            callback: fn_,
            recursive,
            ignore,
        }])?;
        Ok(closers
            .into_iter()
            .next()
            .expect("WatchDirectories returns one closer per request"))
    }

    // Go: mock_watch_backend.go:70 MockWatchBackend.WatchDirectories
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Box<dyn fswatch::Watch>>, GoError> {
        let mut dirs = self.dirs.lock().unwrap();
        for request in &requests {
            if let Some(directory_exists) = &self.directory_exists {
                if !directory_exists(&request.dir) {
                    return Err(errors::errorf(
                        format!("directory does not exist: {}", request.dir),
                        Vec::new(),
                    ));
                }
            }
        }
        let mut closers: Vec<Box<dyn fswatch::Watch>> = Vec::with_capacity(requests.len());
        for request in requests {
            let w = Arc::new(MockWatch {
                path: request.dir.clone(),
                callback: request.callback,
                recursive: request.recursive,
                ignore: request.ignore,
                closed: AtomicBool::new(false),
            });
            dirs.insert(request.dir, w.clone());
            closers.push(Box::new(MockWatchCloser(w)));
        }
        Ok(closers)
    }
}

impl MockWatchBackend {
    // Go: mock_watch_backend.go:95 MockWatchBackend.SendEvents
    /// SendEvents routes events through the registered watch callbacks
    /// that match each event's path. Directory watches match if the event
    /// path is a child (or recursive descendant) of the watched directory.
    /// Events that match no watch are silently dropped: this is by design
    /// so that tests fail when the production code doesn't register the
    /// needed watches.
    ///
    /// PORT: Go keys the targets by `*MockWatch` in a map (random order).
    /// The port keeps them in first-match order and compares with
    /// `Arc::ptr_eq`.
    pub fn send_events(&self, events: Vec<Event>) {
        // Snapshot callbacks under the lock, then invoke outside the lock
        // to avoid deadlock if the callback re-enters the mock.
        let targets: Vec<(Arc<MockWatch>, Vec<Event>)> = {
            let dirs = self.dirs.lock().unwrap();
            let mut targets: Vec<(Arc<MockWatch>, Vec<Event>)> = Vec::new();
            for e in &events {
                // Check directory watches.
                for w in dirs.values() {
                    if w.is_closed() {
                        continue;
                    }
                    // ts#64159 (mock_watch_backend.go:98): the event path is
                    // rooted against the watched directory.
                    let event = Event {
                        path: to_rooted_path(&e.path, &w.path),
                        ..e.clone()
                    };
                    if let Some(ignore) = &w.ignore {
                        if ignore(&event.path) {
                            continue;
                        }
                    }
                    if !path_is_under(
                        &event.path,
                        &w.path,
                        w.recursive,
                        self.use_case_sensitive_file_names,
                    ) {
                        continue;
                    }
                    match targets.iter_mut().find(|(t, _)| Arc::ptr_eq(t, w)) {
                        Some((_, t_events)) => t_events.push(event),
                        None => targets.push((w.clone(), vec![event])),
                    }
                }
            }
            targets
        };

        for (w, events) in targets {
            (w.callback)(events, None);
        }
    }

    // Go: mock_watch_backend.go:134 MockWatchBackend.SendOverflow
    /// SendOverflow simulates a kernel event-queue overflow by invoking every
    /// active watch callback with fswatch.ErrOverflow. The watch manager treats
    /// this as a signal that events were dropped and a full rebuild is required.
    ///
    /// PORT: Go ranges over the `Dirs` map (random order); the port visits
    /// the watches in path order.
    pub fn send_overflow(&self) {
        let cbs: Vec<fswatch::WatchCallback> = {
            let dirs = self.dirs.lock().unwrap();
            dirs.values()
                .filter(|w| !w.is_closed())
                .map(|w| w.callback.clone())
                .collect()
        };
        for cb in cbs {
            cb(Vec::new(), Some(fswatch::ERR_OVERFLOW.clone()));
        }
    }

    // Go: mock_watch_backend.go:134 MockWatchBackend.SendChangedPaths
    /// SendChangedPaths converts a list of file changes into fswatch
    /// events with appropriate event kinds and routes them through
    /// registered watches via SendEvents. For new/modified files, it also
    /// emits update events for their parent directories, simulating how
    /// real filesystem watchers report directory events.
    pub fn send_changed_paths(&self, changes: &[FileChange]) {
        let mut events: Vec<Event> = Vec::with_capacity(changes.len() * 2);
        let mut seen_dirs: HashSet<String> = HashSet::new();
        for c in changes {
            let mut kind = EventKind::Update;
            if c.deleted {
                kind = EventKind::Delete;
            }
            events.push(Event {
                kind,
                path: c.path.clone(),
            });
            // Emit update events for parent directories of changed files.
            // Real filesystem watchers deliver events to non-recursive watches
            // when a child directory is created, which the mock must replicate.
            let mut dir = go_path_dir(&c.path);
            while !dir.is_empty() && dir != "/" && dir != "." {
                if !seen_dirs.insert(dir.clone()) {
                    break;
                }
                events.push(Event {
                    kind: EventKind::Update,
                    path: dir.clone(),
                });
                let parent = go_path_dir(&dir);
                if parent == dir {
                    break;
                }
                dir = parent;
            }
        }
        self.send_events(events);
    }
}

// Go: mock_watch_backend.go:172 pathIsUnder
/// pathIsUnder reports whether eventPath is inside dir. If recursive is
/// false, only direct children match.
///
/// ts#64159: the paths compare as path keys (`PathKey.ContainsPath`), so a
/// watch of a root ("/", "c:/") sees its children. N compared text and
/// wanted a "/" after the directory.
fn path_is_under(
    event_path: &str,
    dir: &str,
    recursive: bool,
    use_case_sensitive_file_names: bool,
) -> bool {
    let dir_key = tspath::to_path(dir, "", use_case_sensitive_file_names);
    let event_key = tspath::to_path(event_path, "", use_case_sensitive_file_names);
    if dir_key == event_key || !dir_key.contains_path(&event_key) {
        return false;
    }
    if recursive {
        return true;
    }
    tspath::to_path(
        &tspath::get_directory_path(event_path),
        "",
        use_case_sensitive_file_names,
    ) == dir_key
}

impl MockWatchBackend {
    // Go: mock_watch_backend.go:186 MockWatchBackend.WatchState
    /// WatchState returns a deterministic, human-readable summary of all
    /// active watches. This is intended to be included in test baselines
    /// so that watch registration correctness is verified via snapshot diffs.
    pub fn watch_state(&self) -> String {
        let dirs = self.dirs.lock().unwrap();

        let mut b = String::new();
        b.push_str("Watch Registrations::\n");

        // Directory watches, sorted by path.
        // PORT: `dirs` is a BTreeMap, so it is already in Go `sort.Strings`
        // order (byte order).
        let active: Vec<&Arc<MockWatch>> = dirs.values().filter(|w| !w.is_closed()).collect();

        b.push_str("Directory watches::\n");
        if active.is_empty() {
            b.push_str("  (none)\n");
        }
        for w in active {
            if w.recursive {
                b.push_str(&format!("  {} (recursive)\n", w.path));
            } else {
                b.push_str(&format!("  {}\n", w.path));
            }
        }

        b
    }
}

// Go: path/path.go:223 Dir
/// Go `path.Dir`: all but the last element of `path`, cleaned.
fn go_path_dir(path: &str) -> String {
    // Go: path/path.go:145 Split
    let dir = match path.rfind('/') {
        Some(i) => &path[..i + 1],
        None => "",
    };
    go_path_clean(dir)
}

// Go: path/path.go:72 Clean
/// Go `path.Clean`: the shortest path name equal to `path` by lexical
/// processing.
///
/// PORT: Go `lazybuf` only avoids a copy; the port always writes into
/// `buf` and keeps Go's write index `w`.
fn go_path_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();

    // Go: path/path.go:33 lazybuf.append
    fn append(buf: &mut Vec<u8>, w: &mut usize, c: u8) {
        if *w < buf.len() {
            buf[*w] = c;
        } else {
            buf.push(c);
        }
        *w += 1;
    }

    let mut buf: Vec<u8> = Vec::with_capacity(n);
    let mut w: usize = 0;

    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        append(&mut buf, &mut w, b'/');
        r = 1;
        dotdot = 1;
    }

    while r < n {
        if p[r] == b'/' {
            // empty path element
            r += 1;
        } else if p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/') {
            // . element
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            // .. element: remove to last /
            r += 2;
            if w > dotdot {
                // can backtrack
                w -= 1;
                while w > dotdot && buf[w] != b'/' {
                    w -= 1;
                }
            } else if !rooted {
                // cannot backtrack, but not rooted, so append .. element.
                if w > 0 {
                    append(&mut buf, &mut w, b'/');
                }
                append(&mut buf, &mut w, b'.');
                append(&mut buf, &mut w, b'.');
                dotdot = w;
            }
        } else {
            // real path element.
            // add slash if needed
            if (rooted && w != 1) || (!rooted && w != 0) {
                append(&mut buf, &mut w, b'/');
            }
            // copy element
            while r < n && p[r] != b'/' {
                append(&mut buf, &mut w, p[r]);
                r += 1;
            }
        }
    }

    // Turn empty string into "."
    if w == 0 {
        return ".".to_string();
    }
    String::from_utf8_lossy(&buf[..w]).into_owned()
}
