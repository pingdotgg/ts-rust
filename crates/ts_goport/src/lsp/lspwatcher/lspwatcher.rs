//! Go `internal/lsp/lspwatcher/lspwatcher.go`.
//!
//! Package lspwatcher implements an in-process file watcher used as a
//! drop-in replacement for LSP-based file watching when the client does not
//! support dynamic registration of file watchers.
//!
//! PORT: the watcher holds the session file system (`Rc<dyn vfs::Fs>`), so
//! it lives on the LSP dispatch thread (PORTING "Threads"). Go mutexes are
//! dropped; state is in `Cell` / `RefCell`. Go `*Watcher` and `*watch` are
//! `Rc`. `time.AfterFunc` is `gostd::local::after_func`, so `flush` runs on
//! the dispatch thread. fswatch callbacks reach the dispatch thread through
//! `DeliveryBridge`. The server only makes this watcher when
//! `fswatch::default().has_fast_recursive_backend()` is true: on macOS
//! (FSEvents) and Windows, not on Linux.

use crate::lsp::lspwatcher::prelude::*;

use crate::frontend::{tspath, vfs};
use crate::fswatch;
use crate::gostd::{errors, local, strconv};
use crate::ls::lsconv;
use crate::lsp::lsproto;
use crate::project::logging;
use crate::project::logging::Logger as _;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

// Go: lsp/lspwatcher/lspwatcher.go:24 throttleWindow
// throttleWindow mirrors VS Code's parcel watcher integration: give the
// first batch a short grace window so adjacent filesystem bursts coalesce.
pub const THROTTLE_WINDOW: Duration = Duration::from_millis(75);

/// The callback a `WatcherBackend` subscription calls.
// PORT: Go passes an `fswatch.WatchCallback`. The lspwatcher callbacks touch
// the file system, logger and subscriptions of the watcher, which are
// dispatch-thread state (`Rc`), so they are not `Send`.
pub type LocalWatchCallback = Rc<dyn Fn(Vec<fswatch::Event>, Option<GoError>)>;

// Go: lsp/lspwatcher/lspwatcher.go:26 watcherBackend
// PORT: Go `io.Closer` is `Box<dyn fswatch::Watch>`; the variadic options
// are a slice. The callback is a `LocalWatchCallback` (see above).
pub trait WatcherBackend {
    fn watch_directory(
        &self,
        dir: &str,
        fn_: LocalWatchCallback,
        opts: &[Box<dyn fswatch::WatchOption>],
    ) -> Result<Box<dyn fswatch::Watch>, GoError>;
}

// Go: lsp/lspwatcher/lspwatcher.go:30 defaultWatcherBackend
// PORT: `bridge` is port-only. It moves fswatch callbacks to the dispatch
// thread (see `DeliveryBridge`).
pub struct DefaultWatcherBackend {
    pub watcher: Arc<dyn fswatch::Watcher>,
    pub bridge: Rc<DeliveryBridge>,
}

impl WatcherBackend for DefaultWatcherBackend {
    // Go: lsp/lspwatcher/lspwatcher.go:34 defaultWatcherBackend.WatchDirectory
    // PORT: fswatch calls its callback on its own thread, and `fn_` must run
    // on the dispatch thread. The fswatch callback only queues the batch in
    // the bridge; the bridge calls `fn_` on the dispatch thread.
    fn watch_directory(
        &self,
        dir: &str,
        fn_: LocalWatchCallback,
        opts: &[Box<dyn fswatch::WatchOption>],
    ) -> Result<Box<dyn fswatch::Watch>, GoError> {
        let bridge = &self.bridge;
        let id = bridge.next_id.get();
        bridge.next_id.set(id + 1);
        let closed = Arc::new(AtomicBool::new(false));
        let callback: fswatch::WatchCallback = {
            let inbox = bridge.inbox.clone();
            let closed = closed.clone();
            Arc::new(move |events: Vec<fswatch::Event>, err: Option<GoError>| {
                if closed.load(Ordering::SeqCst) {
                    return;
                }
                inbox
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(Delivery { id, events, err });
            })
        };
        let inner = self.watcher.watch_directory(dir, callback, opts)?;
        bridge
            .subscriptions
            .borrow_mut()
            .insert(id, (fn_, closed.clone()));
        bridge.start_polling();
        Ok(Box::new(BridgedWatch { inner, closed }))
    }
}

/// How often the dispatch thread takes the queued fswatch batches.
// PORT: no Go source. See `DeliveryBridge`.
const DELIVERY_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// One fswatch callback call, queued for the dispatch thread.
struct Delivery {
    id: u64,
    events: Vec<fswatch::Event>,
    err: Option<GoError>,
}

/// Moves fswatch callbacks from the fswatch threads to the dispatch thread.
///
/// PORT: no Go source. Go calls the lspwatcher callbacks on the fswatch
/// goroutine. Here they touch dispatch-thread state (`Rc`), so the fswatch
/// callback queues each call in `inbox`, in arrival order. `gostd::local`
/// has no queue that another thread can post to, so a `local::after_func`
/// timer takes the queue every `DELIVERY_POLL_INTERVAL` while a
/// subscription is open, and calls each `LocalWatchCallback` in order. It
/// stops when all subscriptions are closed. A batch of a closed
/// subscription is dropped, as if Go's `Close` ran before the fswatch
/// callback.
#[derive(Default)]
pub struct DeliveryBridge {
    /// Filled by the fswatch threads.
    inbox: Arc<Mutex<VecDeque<Delivery>>>,
    /// Open subscriptions by id: the callback and the closed flag of its
    /// `BridgedWatch`.
    subscriptions: RefCell<FxHashMap<u64, (LocalWatchCallback, Arc<AtomicBool>)>>,
    next_id: Cell<u64>,
    poll_timer: RefCell<Option<local::LocalTimer>>,
}

impl DeliveryBridge {
    /// Arms the poll timer if it is not armed.
    fn start_polling(self: &Rc<Self>) {
        if self.poll_timer.borrow().is_some() {
            return;
        }
        // Weak: the timer must not keep the bridge alive.
        let bridge = Rc::downgrade(self);
        let timer = local::after_func(
            DELIVERY_POLL_INTERVAL,
            Box::new(move || {
                if let Some(bridge) = bridge.upgrade() {
                    bridge.poll();
                }
            }),
        );
        *self.poll_timer.borrow_mut() = Some(timer);
    }

    /// Runs on the dispatch thread when the poll timer fires.
    fn poll(self: &Rc<Self>) {
        *self.poll_timer.borrow_mut() = None;
        loop {
            let delivery = self
                .inbox
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            let Some(Delivery { id, events, err }) = delivery else {
                break;
            };
            // Clone the callback out: it can open or close subscriptions.
            let callback = match self.subscriptions.borrow().get(&id) {
                Some((callback, closed)) if !closed.load(Ordering::SeqCst) => callback.clone(),
                _ => continue,
            };
            callback(events, err);
        }
        self.subscriptions
            .borrow_mut()
            .retain(|_, (_, closed)| !closed.load(Ordering::SeqCst));
        if !self.subscriptions.borrow().is_empty() {
            self.start_polling();
        }
    }
}

/// The `fswatch::Watch` that `DefaultWatcherBackend` returns: closing it
/// also stops the delivery of its queued batches.
struct BridgedWatch {
    inner: Box<dyn fswatch::Watch>,
    closed: Arc<AtomicBool>,
}

impl fswatch::Watch for BridgedWatch {
    fn close(&self) -> Result<(), GoError> {
        self.closed.store(true, Ordering::SeqCst);
        self.inner.close()
    }

    fn unexported(&self) {}
}

// Go: lsp/lspwatcher/lspwatcher.go:42 Watcher
// Watcher manages a set of file system subscriptions identified by
// WatcherID strings (matching the LSP server's project.WatcherID type).
// Events are delivered to onChanges in batches as `*lsproto.FileEvent`,
// shaped exactly like a `workspace/didChangeWatchedFiles` notification.
// PORT: Go map fields are `IndexMap`s (insertion order; Go map order is
// random). A nil map after `Close` is an empty one.
pub struct Watcher {
    pub fs: Rc<dyn vfs::Fs>,
    pub backend: Box<dyn WatcherBackend>,
    pub on_changes: Box<dyn Fn(Vec<lsproto::FileEvent>)>,
    pub logger: Option<Rc<dyn logging::Logger>>,

    // watches holds the watches associated with each LSP WatcherID. A single id
    // may map to more than one watch because each FileSystemWatcher in the
    // registration becomes its own watch (different roots and kinds).
    pub watches: RefCell<IndexMap<String, Vec<Rc<Watch>>>>,
    pub closed: Cell<bool>,

    // Pending batch state, protected by mu.
    pub pending: RefCell<Option<IndexMap<String, lsproto::FileEvent>>>,
    pub flush_timer: RefCell<Option<local::LocalTimer>>,
}

// Go: lsp/lspwatcher/lspwatcher.go:80 watch
// watch represents one FileSystemWatcher from the LSP registration.
//
// The directory the session asks to watch may not exist yet (common in
// granular mode, where each probed-but-missing package directory becomes a
// watch) or may be deleted while watched. To honor the watch across those
// transitions, a watch maintains either:
//
//   - a "target" subscription rooted directly at the requested directory, once
//     it exists, or
//   - an "ancestor" subscription on the nearest existing ancestor
//     (non-recursive), used to detect the requested directory — or an
//     intermediate path component — being created, after which the watch
//     descends toward and eventually promotes to the target.
//
// When the target materializes, synthetic create events are emitted for it
// (and, depending on whether the watch is recursive, its immediate children or
// its whole subtree) so the session re-resolves files that appeared in the gap
// before the real subscription was installed.
//
// All path fields are tspath-style (forward-slash) absolute paths.
pub struct Watch {
    pub watcher: Rc<Watcher>,
    pub requested_directory: String, // directory requested by the LSP layer (possibly a symlink)
    pub kind: lsproto::WatchKind,
    pub recursive: bool, // whether the target subscription should be recursive

    pub subscription: RefCell<Option<Box<dyn fswatch::Watch>>>, // current subscription (target or ancestor); nil if none
    pub watched_directory: RefCell<String>, // canonicalized directory 'subscription' is rooted at
    pub watching_target: Cell<bool>, // whether 'subscription' is rooted at the target directory
    pub closed: Cell<bool>,
}

// Go: lsp/lspwatcher/lspwatcher.go:95 New
// New constructs a Watcher backed by internal/fswatch's platform-default
// watcher implementation.
pub fn new(
    fs: Rc<dyn vfs::Fs>,
    on_changes: Box<dyn Fn(Vec<lsproto::FileEvent>)>,
    logger: Option<Rc<dyn logging::Logger>>,
) -> Rc<Watcher> {
    new_with_fs_watcher(fs, fswatch::default(), on_changes, logger)
}

// Go: lsp/lspwatcher/lspwatcher.go:102 NewWithFSWatcher
// NewWithFSWatcher constructs a Watcher backed by the provided fswatch.Watcher.
// Use this to select a specific backend (e.g. fswatch.Kqueue()) instead of the
// platform default.
pub fn new_with_fs_watcher(
    fs: Rc<dyn vfs::Fs>,
    watcher: Arc<dyn fswatch::Watcher>,
    on_changes: Box<dyn Fn(Vec<lsproto::FileEvent>)>,
    logger: Option<Rc<dyn logging::Logger>>,
) -> Rc<Watcher> {
    new_with_backend(
        fs,
        Box::new(DefaultWatcherBackend {
            watcher,
            bridge: Rc::default(),
        }),
        on_changes,
        logger,
    )
}

// Go: lsp/lspwatcher/lspwatcher.go:106 newWithBackend
pub fn new_with_backend(
    fs: Rc<dyn vfs::Fs>,
    backend: Box<dyn WatcherBackend>,
    on_changes: Box<dyn Fn(Vec<lsproto::FileEvent>)>,
    logger: Option<Rc<dyn logging::Logger>>,
) -> Rc<Watcher> {
    Rc::new(Watcher {
        fs,
        backend,
        on_changes,
        logger,
        watches: RefCell::new(IndexMap::new()),
        closed: Cell::new(false),
        pending: RefCell::new(None),
        flush_timer: RefCell::new(None),
    })
}

impl Watcher {
    // Go: lsp/lspwatcher/lspwatcher.go:125 Watcher.WatchFiles
    // WatchFiles subscribes to each FileSystemWatcher under the given id.
    //
    // A watcher whose directory does not exist yet is not an error: an ancestor
    // watch is installed on the nearest existing ancestor and the subscription is
    // reported as successful, so the session's notion of "this watcher is alive"
    // stays true for the subscription's whole lifetime. Only a genuine backend
    // failure (e.g. resource exhaustion while watching an existing directory)
    // causes WatchFiles to roll back the whole id and return an error, so the
    // session's pending/retry path re-registers it on the next reevaluation.
    pub fn watch_files(
        self: &Rc<Self>,
        id: &str,
        file_system_watchers: &[lsproto::FileSystemWatcher],
    ) -> Result<(), GoError> {
        if self.closed.get() {
            return Err(errors::new("lspwatcher: closed"));
        }
        if self.watches.borrow().contains_key(id) {
            return Err(errors::new(format!(
                "lspwatcher: watcher {} already exists",
                strconv::quote(id)
            )));
        }
        // Mark the id as existing before installing any watches so a concurrent
        // WatchFiles for the same id is rejected above.
        self.watches.borrow_mut().insert(id.to_string(), Vec::new());

        let mut failed = false;
        for file_system_watcher in file_system_watchers {
            let (directory, ok) = watch_root(file_system_watcher);
            if !ok || directory.is_empty() {
                self.logger.logf(&format!(
                    "lspwatcher: skipping watcher {}: unrecognized pattern {}",
                    strconv::quote(id),
                    strconv::quote(&watch_pattern_string(file_system_watcher))
                ));
                continue;
            }
            let new_watch = Rc::new(Watch {
                watcher: self.clone(),
                requested_directory: directory.clone(),
                kind: effective_kind(file_system_watcher),
                recursive: is_recursive_glob(file_system_watcher),
                subscription: RefCell::new(None),
                watched_directory: RefCell::new(String::new()),
                watching_target: Cell::new(false),
                closed: Cell::new(false),
            });
            if let Err(err) = new_watch.reconcile(false /*emitSynthetic*/) {
                self.logger.logf(&format!(
                    "lspwatcher: failed to register watcher {} for {}: {}",
                    strconv::quote(id),
                    strconv::quote(&directory),
                    err.error()
                ));
                new_watch.close();
                failed = true;
                break;
            }
            if self.closed.get() {
                new_watch.close();
                return Err(errors::new("lspwatcher: closed"));
            }
            self.watches
                .borrow_mut()
                .entry(id.to_string())
                .or_default()
                .push(new_watch);
        }

        if failed {
            // Roll back the whole id so the session's retry (MarkPending) can
            // cleanly re-register it. The session treats an id as a single unit.
            let _ = self.unwatch_files(id);
            return Err(errors::new(format!(
                "lspwatcher: failed to register one or more watchers for {}",
                strconv::quote(id)
            )));
        }
        Ok(())
    }

    // Go: lsp/lspwatcher/lspwatcher.go:179 Watcher.UnwatchFiles
    // UnwatchFiles tears down all subscriptions associated with id.
    pub fn unwatch_files(&self, id: &str) -> Result<(), GoError> {
        let watches = self.watches.borrow_mut().shift_remove(id);
        let Some(watches) = watches else {
            return Err(errors::new(format!(
                "lspwatcher: no watcher with id {}",
                strconv::quote(id)
            )));
        };
        for watch in &watches {
            watch.close();
        }
        Ok(())
    }

    // Go: lsp/lspwatcher/lspwatcher.go:195 Watcher.Close
    // Close removes every subscription. Safe to call multiple times.
    pub fn close(&self) {
        if self.closed.get() {
            return;
        }
        self.closed.set(true);
        let watches_by_id = std::mem::take(&mut *self.watches.borrow_mut());
        if let Some(flush_timer) = self.flush_timer.borrow_mut().take() {
            flush_timer.stop();
        }
        *self.pending.borrow_mut() = None;
        for watches in watches_by_id.values() {
            for watch in watches {
                watch.close();
            }
        }
    }

    // Go: lsp/lspwatcher/lspwatcher.go:396 Watcher.forwardEvents
    // forwardEvents translates fswatch events into LSP file events and enqueues
    // them for the next debounced flush.
    pub fn forward_events(self: &Rc<Self>, kind: lsproto::WatchKind, events: &[fswatch::Event]) {
        if self.closed.get() {
            return;
        }
        {
            let mut pending = self.pending.borrow_mut();
            let pending = pending.get_or_insert_with(|| IndexMap::with_capacity(events.len()));
            for event in events {
                // PORT: Go also has a `default: continue` arm for other kinds;
                // the Rust enum has only these two.
                let change_type = match event.kind {
                    fswatch::EventKind::Update => {
                        // fswatch intentionally doesn't distinguish create vs update.
                        // For LSP consumers this is fine: callers infer create/update
                        // from their own cache and both should invalidate stale state.
                        if kind.0 & (lsproto::WatchKind::CREATE.0 | lsproto::WatchKind::CHANGE.0)
                            == 0
                        {
                            continue;
                        }
                        lsproto::FileChangeType::CHANGED
                    }
                    fswatch::EventKind::Delete => {
                        if kind.0 & lsproto::WatchKind::DELETE.0 == 0 {
                            continue;
                        }
                        lsproto::FileChangeType::DELETED
                    }
                };

                // ts#64159: the event path is rooted and normalized
                // (lspwatcher.go:48 `RootedPathFromAbsolute`).
                let path = lsproto::rooted_path_from_absolute(&event.path);
                let uri = lsconv::file_name_to_document_uri(&path);
                pending.insert(
                    uri.0.clone(),
                    lsproto::FileEvent {
                        uri,
                        type_: change_type,
                    },
                );
            }
        }
        self.schedule_flush_locked();
    }

    // Go: lsp/lspwatcher/lspwatcher.go:442 Watcher.emitSyntheticCreates
    // emitSyntheticCreates enqueues synthetic create events after a target watch is
    // (re)installed following a missing→present transition, so the session
    // re-resolves files that appeared before the real watch existed. The target
    // directory itself is always included; for a non-recursive watch its immediate
    // children are added, and for a recursive watch its whole subtree is walked.
    // Nothing is emitted if the watch doesn't request create notifications.
    pub fn emit_synthetic_creates(
        self: &Rc<Self>,
        directory: &str,
        kind: lsproto::WatchKind,
        recursive: bool,
    ) {
        if kind.0 & lsproto::WatchKind::CREATE.0 == 0 {
            return;
        }
        let mut paths: Vec<String> = vec![directory.to_string()];
        if recursive {
            // ts#64277: Go `vfs.WalkDir(w.fs, directory, ...)`.
            let _ = vfs::walk_dir(
                &*self.fs,
                directory,
                &mut |path: &str,
                      _entry: Option<&vfs::DirEntry>,
                      err: Option<vfs::FsError>|
                 -> Result<(), vfs::FsError> {
                    if err.is_none() && path != directory {
                        paths.push(path.to_string());
                    }
                    Ok(())
                },
            );
        } else {
            let entries = self.fs.get_accessible_entries(directory);
            for name in &entries.files {
                paths.push(tspath::combine_paths(directory, &[name]));
            }
            for name in &entries.directories {
                paths.push(tspath::combine_paths(directory, &[name]));
            }
        }
        self.enqueue_synthetic_creates(&paths);
    }

    // Go: lsp/lspwatcher/lspwatcher.go:469 Watcher.enqueueSyntheticCreates
    // enqueueSyntheticCreates adds synthetic create events for paths, without
    // clobbering a more specific event already pending for the same path (e.g. a
    // real delete).
    pub fn enqueue_synthetic_creates(self: &Rc<Self>, paths: &[String]) {
        if self.closed.get() {
            return;
        }
        {
            let mut pending = self.pending.borrow_mut();
            let pending = pending.get_or_insert_with(|| IndexMap::with_capacity(paths.len()));
            for path in paths {
                let uri = lsconv::file_name_to_document_uri(path);
                if pending.contains_key(&uri.0) {
                    continue;
                }
                pending.insert(
                    uri.0.clone(),
                    lsproto::FileEvent {
                        uri,
                        type_: lsproto::FileChangeType::CREATED,
                    },
                );
            }
        }
        self.schedule_flush_locked();
    }

    // Go: lsp/lspwatcher/lspwatcher.go:494 Watcher.scheduleFlushLocked
    // scheduleFlushLocked arms the debounce flush timer if it isn't already armed.
    // Callers must hold w.mu.
    pub fn schedule_flush_locked(self: &Rc<Self>) {
        if self.flush_timer.borrow().is_none() {
            let w = self.clone();
            let timer = local::after_func(THROTTLE_WINDOW, Box::new(move || w.flush()));
            *self.flush_timer.borrow_mut() = Some(timer);
        }
    }

    // Go: lsp/lspwatcher/lspwatcher.go:500 Watcher.flush
    pub fn flush(&self) {
        if self.closed.get() {
            return;
        }
        let pending = self.pending.borrow_mut().take();
        *self.flush_timer.borrow_mut() = None;

        let Some(pending) = pending else {
            return;
        };
        if pending.is_empty() {
            return;
        }
        // PORT: Go ranges over the pending map (random order); the port keeps
        // insertion order.
        let changes: Vec<lsproto::FileEvent> = pending.into_values().collect();
        (self.on_changes)(changes);
    }
}

impl Watch {
    // Go: lsp/lspwatcher/lspwatcher.go:219 watch.close
    // close tears down the watch's current subscription and prevents any in-flight
    // reconcile from reinstalling one.
    pub fn close(&self) {
        self.closed.set(true);
        let subscription = self.subscription.borrow_mut().take();
        *self.watched_directory.borrow_mut() = String::new();
        if let Some(subscription) = subscription {
            let _ = subscription.close();
        }
    }

    // Go: lsp/lspwatcher/lspwatcher.go:244 watch.reconcile
    // reconcile installs or advances this watch toward the target directory based
    // on the current filesystem state. It is called at registration, whenever a
    // ancestor watch observes activity, and after a target watch is terminated by
    // deletion.
    //
    // emitSynthetic controls whether promoting to the target emits synthetic
    // create events: false for the initial install when the target already exists
    // (the session already knows about those files), true for any missing→present
    // recovery.
    //
    // It returns a non-nil error only on a genuine backend failure to install a
    // watch; a missing target directory is handled by installing an ancestor watch
    // and returns nil.
    pub fn reconcile(self: &Rc<Self>, mut emit_synthetic_creates: bool) -> Result<(), GoError> {
        let watcher = self.watcher.clone();
        loop {
            if self.closed.get() {
                return Ok(());
            }
            if watcher.fs.directory_exists(&self.requested_directory) {
                if self.watching_target.get() && self.subscription.borrow().is_some() {
                    return Ok(()); // already watching the target
                }
                let target_directory = self.requested_directory.clone();
                let mut options: Vec<Box<dyn fswatch::WatchOption>> = Vec::new();
                if self.recursive {
                    options.push(fswatch::with_recursive());
                }
                let subscription = watcher.backend.watch_directory(
                    &target_directory,
                    self.target_callback(&target_directory),
                    &options,
                )?;
                let previous = self.subscription.borrow_mut().replace(subscription);
                *self.watched_directory.borrow_mut() = target_directory.clone();
                self.watching_target.set(true);
                if let Some(previous) = previous {
                    let _ = previous.close();
                }
                if emit_synthetic_creates {
                    watcher.emit_synthetic_creates(&target_directory, self.kind, self.recursive);
                }
                return Ok(());
            }

            let (ancestor, ok) = nearest_existing_ancestor(&*watcher.fs, &self.requested_directory);
            if !ok {
                // Nothing exists to watch (even the root is gone); drop any subscription.
                let previous = self.subscription.borrow_mut().take();
                if let Some(previous) = previous {
                    *self.watched_directory.borrow_mut() = String::new();
                    self.watching_target.set(false);
                    let _ = previous.close();
                }
                return Ok(());
            }
            let ancestor_directory = ancestor;
            if !self.watching_target.get()
                && self.subscription.borrow().is_some()
                && *self.watched_directory.borrow() == ancestor_directory
            {
                return Ok(()); // already watching the correct ancestor
            }
            let subscription = watcher.backend.watch_directory(
                &ancestor_directory,
                self.ancestor_callback(),
                &[],
            )?;
            let previous = self.subscription.borrow_mut().replace(subscription);
            *self.watched_directory.borrow_mut() = ancestor_directory;
            self.watching_target.set(false);
            if let Some(previous) = previous {
                let _ = previous.close();
            }
            // The target may have appeared between the DirectoryExists check above
            // and installing this ancestor subscription (e.g. an atomic tree
            // creation), so loop to descend further or promote immediately. Any
            // promotion from here on is a missing→present transition, so synthesize
            // creates.
            emit_synthetic_creates = true;
        }
    }

    // Go: lsp/lspwatcher/lspwatcher.go:318 watch.targetCallback
    // targetCallback returns the fswatch callback for a target watch rooted at
    // watchedReal. It forwards events to the session and, on ErrWatchTerminated
    // (the watched directory was deleted), falls back to watching the nearest
    // existing ancestor so the watch re-attaches when the directory is recreated.
    pub fn target_callback(self: &Rc<Self>, watched_directory: &str) -> LocalWatchCallback {
        let w = self.clone();
        let watcher = self.watcher.clone();
        let watched_directory = watched_directory.to_string();
        Rc::new(move |events: Vec<fswatch::Event>, err: Option<GoError>| {
            let mut terminated = false;
            if let Some(err) = &err {
                if errors::is(err, &fswatch::ERR_OVERFLOW) {
                    watcher.logger.logf(&format!(
                        "lspwatcher: watch overflow in {} (some events may have been dropped): {}",
                        strconv::quote(&watched_directory),
                        err.error()
                    ));
                } else if errors::is(err, &fswatch::ERR_WATCH_TERMINATED) {
                    terminated = true;
                    watcher.logger.logf(&format!(
                        "lspwatcher: watch terminated in {} (directory removed): {}",
                        strconv::quote(&watched_directory),
                        err.error()
                    ));
                } else {
                    watcher.logger.logf(&format!(
                        "lspwatcher: watch error in {}: {}",
                        strconv::quote(&watched_directory),
                        err.error()
                    ));
                }
            }
            if !events.is_empty() {
                watcher.forward_events(w.kind, &events);
            }
            if terminated {
                // The delete event for the directory was forwarded above; now
                // re-attach to the nearest existing ancestor.
                w.handle_terminated();
            }
        })
    }

    // Go: lsp/lspwatcher/lspwatcher.go:351 watch.handleTerminated
    // handleTerminated clears the dead target watch (the backend has already
    // removed it) and re-evaluates, falling back to an ancestor watch on the nearest
    // existing ancestor so the watch re-attaches when the directory reappears.
    // Clearing the state first is essential: reconcile would otherwise see
    // watchingTarget && subscription != nil and conclude the target is already
    // watched, even though the subscription is dead — losing recovery if the
    // directory is recreated before reconcile runs.
    pub fn handle_terminated(self: &Rc<Self>) {
        if self.closed.get() {
            return;
        }
        let previous = self.subscription.borrow_mut().take();
        *self.watched_directory.borrow_mut() = String::new();
        self.watching_target.set(false);
        if let Some(previous) = previous {
            let _ = previous.close();
        }
        let _ = self.reconcile(true /*emitSyntheticCreates*/);
    }

    // Go: lsp/lspwatcher/lspwatcher.go:372 watch.ancestorCallback
    // ancestorCallback returns the fswatch callback for an ancestor watch. Ancestor
    // watches exist only to detect the target — or an intermediate path component —
    // being created; their events are about ancestor directories the session
    // doesn't track, so they are ignored and the watch is simply re-evaluated.
    pub fn ancestor_callback(self: &Rc<Self>) -> LocalWatchCallback {
        let w = self.clone();
        Rc::new(move |_events: Vec<fswatch::Event>, _err: Option<GoError>| {
            let _ = w.reconcile(true /*emitSyntheticCreates*/);
        })
    }
}

// Go: lsp/lspwatcher/lspwatcher.go:381 nearestExistingAncestor
// nearestExistingAncestor returns the deepest existing directory that is dir or
// an ancestor of dir, walking upward. ok is false only if nothing in the chain
// (including the root) exists.
pub fn nearest_existing_ancestor(fs: &dyn vfs::Fs, dir: &str) -> (String, bool) {
    let mut dir = dir.to_string();
    loop {
        if fs.directory_exists(&dir) {
            return (dir, true);
        }
        let parent = tspath::get_directory_path(&dir);
        if parent == dir {
            return (String::new(), false);
        }
        dir = parent;
    }
}

// Go: lsp/lspwatcher/lspwatcher.go:531 watchRoot
// watchRoot extracts the directory the fswatch subscription should be
// rooted at from a FileSystemWatcher. The patterns the project layer
// produces are of the form `<dir>/**/*` (recursive) or `<dir>/*`
// (non-recursive, used by granular watch mode), either as a Pattern
// with a fully-qualified directory or as a RelativePattern with a
// file:// BaseUri, so the heuristic of "everything before the first
// glob meta character" is reliable. Use [isRecursiveGlob] to determine
// whether the subscription should be recursive.
//
// Returned roots are tspath-normalized (forward-slash) absolute paths.
// ts#64159: a pattern's root must be absolute. A relative pattern's root
// is resolved against its base (so "../shared/*" in "/workspace/app" is
// "/workspace/shared"), not joined to it first.
pub fn watch_root(file_system_watcher: &lsproto::FileSystemWatcher) -> (String, bool) {
    if let Some(pattern) = &file_system_watcher.glob_pattern.pattern {
        return to_watch_root(&root_from_glob(pattern));
    }
    if let Some(relative_pattern) = &file_system_watcher.glob_pattern.relative_pattern {
        if let Some(uri) = &relative_pattern.base_uri.uri {
            let base = lsproto::DocumentUri(uri.0.clone()).file_name();
            if !base.is_empty() {
                let root = root_from_glob(&relative_pattern.pattern);
                // Go: RootedDirectoryPath.ResolveDirectory(root)
                if root.is_empty() {
                    return (base, true);
                }
                return (
                    lsproto::rooted_path_from_absolute(&tspath::get_normalized_absolute_path(
                        &root, &base,
                    )),
                    true,
                );
            }
        }
        return (String::new(), false);
    }
    (String::new(), false)
}

// Go: lsp/lspwatcher/lspwatcher.go:566 toWatchRoot (ts#64159)
// PORT: Go also checks that the root is normalized
// (`RootedDirectoryPathFromNormalized`); `root_from_glob` normalizes it.
fn to_watch_root(root: &str) -> (String, bool) {
    if root.is_empty() || !tspath::path_is_absolute(root) {
        return (String::new(), false);
    }
    (root.to_string(), true)
}

// Go: lsp/lspwatcher/lspwatcher.go:573 rootFromGlob
pub fn root_from_glob(pattern: &str) -> String {
    let pattern = tspath::normalize_slashes(pattern);
    let mut meta_index: i32 = -1;
    for (i, b) in pattern.bytes().enumerate() {
        if let b'*' | b'?' | b'[' | b'{' = b {
            meta_index = i as i32;
        }
        if meta_index != -1 {
            break;
        }
    }
    if meta_index == -1 {
        return tspath::normalize_path(pattern.trim_end_matches('/'));
    }
    let directory = pattern[..meta_index as usize].trim_end_matches('/');
    if directory.is_empty() {
        return String::new();
    }
    tspath::normalize_path(directory)
}

// Go: lsp/lspwatcher/lspwatcher.go:570 watchPatternString
pub fn watch_pattern_string(file_system_watcher: &lsproto::FileSystemWatcher) -> String {
    if let Some(pattern) = &file_system_watcher.glob_pattern.pattern {
        return pattern.clone();
    }
    if let Some(relative_pattern) = &file_system_watcher.glob_pattern.relative_pattern {
        let mut base = String::new();
        if let Some(uri) = &relative_pattern.base_uri.uri {
            base = uri.0.clone();
        }
        return base + "/" + &relative_pattern.pattern;
    }
    String::new()
}

// Go: lsp/lspwatcher/lspwatcher.go:587 isRecursiveGlob
// isRecursiveGlob reports whether a FileSystemWatcher's pattern requests
// recursive watching (contains a `**` segment). Granular watch mode emits
// non-recursive `<dir>/*` patterns, which watch only the immediate directory.
pub fn is_recursive_glob(file_system_watcher: &lsproto::FileSystemWatcher) -> bool {
    watch_pattern_string(file_system_watcher).contains("**")
}

// Go: lsp/lspwatcher/lspwatcher.go:591 effectiveKind
pub fn effective_kind(file_system_watcher: &lsproto::FileSystemWatcher) -> lsproto::WatchKind {
    if let Some(kind) = file_system_watcher.kind {
        return kind;
    }
    lsproto::WatchKind(
        lsproto::WatchKind::CREATE.0 | lsproto::WatchKind::CHANGE.0 | lsproto::WatchKind::DELETE.0,
    )
}
