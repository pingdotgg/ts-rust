//! Go: internal/execute/watchmanager/watchmanager.go (fswatch directory
//! watches, event accumulation and DoCycle signaling for `tsc --watch` and
//! `tsc -b --watch`).
//!
//! PORT: threads. The frontend `Writer`, file system and backend are `Rc`
//! and stay on the dispatch thread. fswatch callbacks run on the debouncer
//! thread. So the Go `WatchManager` is split: `WatchManager` holds the
//! dispatch-thread fields, and `WatchManagerShared` (an `Arc`) holds the
//! fields that `onWatchEvents` and `handleWatchTerminated` touch
//! (`mu`, `watchedDirs`, the `doCycleCh` sender, `changedMu` and its data).
//! Those two methods live on `WatchManagerShared`. Their `DebugLog` and
//! `warnWriter` output goes to the process stdout (`stdio::CliStdout`): Go
//! passes `sys.Writer()`, which is `os.Stdout` for the real system, and the
//! `Rc` writer cannot cross threads.

use crate::execute::watchmanager::prelude::*;

use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};

use crate::execute::tsc::{Writer, write_str};
use crate::frontend::core_ls_ext;
use crate::frontend::tspath;
use crate::fswatch;
use crate::gostd::errors;

// Go: watchmanager.go:15 watchedDir
/// PORT: shared as `Arc<WatchedDir>`; Go compares `*watchedDir` pointers
/// (`Arc::ptr_eq`). Go sets `closer` after the watch starts, so it sits in a
/// `Mutex` (`None` is a nil closer).
pub struct WatchedDir {
    /// ts#64159: the spelling of the watched directory (the map key is its
    /// path key).
    pub dir: String,
    pub closer: Mutex<Option<Box<dyn fswatch::Watch>>>,
    pub recursive: bool,
}

impl WatchedDir {
    // PORT: Go `wd.closer.Close()` (the error is ignored).
    fn close_closer(&self) {
        if let Some(closer) = self.closer.lock().unwrap().as_ref() {
            let _ = closer.close();
        }
    }
}

// Go: watchmanager.go:21 dirWatchUpdate
// ts#64159: `key` is the path key of `dir`.
#[derive(Clone)]
struct DirWatchUpdate {
    key: String,
    dir: String,
    recursive: bool,
}

/// Go `CaseSensitivity.PathKey` of a rooted, normalized directory.
fn path_key(dir: &str, use_case_sensitive_file_names: bool) -> String {
    tspath::to_path(dir, "", use_case_sensitive_file_names).0
}

// Go: watchmanager.go:35 WatchManager
/// WatchManager manages fswatch directory watches, event accumulation,
/// and DoCycle signaling. It is shared by the CLI watcher and the build
/// mode orchestrator.
///
/// Locking contract:
///   - Call Lock/Unlock around the entire DoCycle body.
///   - ReconcileWatches must be called under Lock.
///   - CloseAllWatches and handleWatchTerminated manage their own locking.
pub struct WatchManager {
    pub backend: Option<Rc<dyn WatchBackend>>,

    /// DebugLog receives verbose watch diagnostics when non-nil
    pub debug_log: Option<Writer>,

    /// PORT: only `onWatchEvents` writes to it in Go; that runs on the
    /// callback thread and writes to the process stdout (see the file
    /// comment).
    pub warn_writer: Writer,
    pub dir_exists: Box<dyn Fn(&str) -> bool>,
    /// Go `caseSensitivity` (ts#64159): `true` is case-sensitive.
    pub use_case_sensitive_file_names: bool,

    /// PORT: the fields the callback thread shares (see the file comment).
    pub shared: Arc<WatchManagerShared>,
}

/// PORT: the `WatchManager` fields that the fswatch callback thread
/// touches.
pub struct WatchManagerShared {
    /// Go `mu`. Go locks it in `Lock` and unlocks it in `Unlock`.
    pub mu: GoMutex,
    /// Go `watchedDirs` (guarded by `mu` in Go; the `Mutex` is Rust's
    /// data lock, taken only for short reads and writes). ts#64159: keyed
    /// by the path key of the directory.
    pub watched_dirs: Mutex<FxHashMap<String, Arc<WatchedDir>>>,
    /// Go `doCycleCh` (see `DoCycleCh`).
    pub do_cycle_ch: DoCycleCh,

    pub changed_mu: Mutex<WatchManagerChanged>,
}

/// PORT: the `WatchManager` fields that Go `changedMu` guards.
#[derive(Default)]
pub struct WatchManagerChanged {
    /// Go `map[string]fswatch.EventKind`; `None` is Go's nil map.
    pub changed_paths: Option<FxHashMap<String, fswatch::EventKind>>,
    pub changed_overflow: bool,
}

/// PORT: Go `sync.Mutex` that one method locks and another unlocks
/// (`Lock` and `Unlock` around a DoCycle body). A std `MutexGuard` cannot
/// cross those calls, so this is a flag and a condition variable.
#[derive(Default)]
pub struct GoMutex {
    locked: Mutex<bool>,
    cond: Condvar,
}

impl GoMutex {
    /// Go `m.Lock()`.
    pub fn lock(&self) {
        let mut locked = self.locked.lock().unwrap();
        while *locked {
            locked = self.cond.wait(locked).unwrap();
        }
        *locked = true;
    }

    /// Go `m.Unlock()`.
    pub fn unlock(&self) {
        let mut locked = self.locked.lock().unwrap();
        if !*locked {
            panic!("sync: unlock of unlocked mutex");
        }
        *locked = false;
        self.cond.notify_one();
    }
}

/// PORT: Go `doCycleCh`, a `chan struct{}` of capacity 1 that `RunLoop`
/// receives from and `signalDoCycle` sends to without blocking. A Go send
/// while the receiver waits hands the value to it and leaves the buffer
/// empty, so a second send fits before the receiver runs. A std
/// `sync_channel(1)` keeps the first value in its slot until the receiver
/// thread runs, and then drops the second send.
#[derive(Default)]
pub struct DoCycleCh {
    state: Mutex<DoCycleState>,
    cond: Condvar,
}

#[derive(Default)]
struct DoCycleState {
    /// Values sent and not received yet: one handed to the waiting
    /// receiver and one in the buffer at most.
    pending: u8,
    /// The receiver waits in `recv`.
    waiting: bool,
    /// The context of `run_loop` is done (`wake`): `recv` waits no more.
    woken: bool,
}

impl DoCycleCh {
    /// Go `select { case ch <- struct{}{}: default: }`. False when the
    /// value does not fit.
    pub fn try_send(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        let capacity = if state.waiting { 2 } else { 1 };
        if state.pending >= capacity {
            return false;
        }
        state.pending += 1;
        self.cond.notify_one();
        true
    }

    /// Go `select { case <-ctx.Done(): ...; case <-ch: ... }` of
    /// `RunLoop`: waits for a value or for `wake` (the `ctx.Done()` case).
    /// True when a value came.
    pub fn recv(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.pending == 0 && !state.woken {
            state.waiting = true;
            state = self
                .cond
                .wait_while(state, |state| state.pending == 0 && !state.woken)
                .unwrap();
            state.waiting = false;
        }
        if state.pending == 0 {
            return false;
        }
        state.pending -= 1;
        true
    }

    /// The `ctx.Done()` case of the `RunLoop` select: ends the wait in
    /// `recv`, and each later one until `reset_wake`.
    pub fn wake(&self) {
        self.state.lock().unwrap().woken = true;
        self.cond.notify_all();
    }

    /// A new `RunLoop`, with a new context.
    fn reset_wake(&self) {
        self.state.lock().unwrap().woken = false;
    }

    /// Go `select { case <-ch: return true; default: return false }`.
    #[cfg(test)]
    fn try_recv(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.pending == 0 {
            return false;
        }
        state.pending -= 1;
        true
    }
}

// Go: watchmanager.go:53 NewWatchManager
// ts#64159: the watches are keyed by path key under `use_case_sensitive_file_names`.
pub fn new_watch_manager(
    warn_writer: Writer,
    dir_exists: Box<dyn Fn(&str) -> bool>,
    use_case_sensitive_file_names: bool,
) -> WatchManager {
    WatchManager {
        backend: None,
        debug_log: None,
        warn_writer,
        dir_exists,
        use_case_sensitive_file_names,
        shared: Arc::new(WatchManagerShared {
            mu: GoMutex::default(),
            watched_dirs: Mutex::new(FxHashMap::default()),
            do_cycle_ch: DoCycleCh::default(),
            changed_mu: Mutex::new(WatchManagerChanged::default()),
        }),
    }
}

impl WatchManager {
    // Go: watchmanager.go:63 WatchManager.SetBackend
    pub fn set_backend(&mut self, b: Rc<dyn WatchBackend>) {
        self.backend = Some(b);
    }

    // Go: watchmanager.go:65 WatchManager.Backend
    pub fn backend(&self) -> Option<Rc<dyn WatchBackend>> {
        self.backend.clone()
    }

    // Go: watchmanager.go:67 WatchManager.EnsureDefaultBackend
    pub fn ensure_default_backend(&mut self) {
        if self.backend.is_none() {
            let fsw = fswatch::default();
            self.backend = Some(Rc::new(FsWatchBackend { inner: fsw.clone() }));
            if let Some(debug_log) = &self.debug_log {
                write_str(
                    debug_log,
                    &format!("[watch] using {} backend\n", fsw.name()),
                );
            }
        }
    }

    // Go: watchmanager.go:77 WatchManager.Lock
    pub fn lock(&self) {
        self.shared.mu.lock();
    }

    // Go: watchmanager.go:79 WatchManager.Unlock
    pub fn unlock(&self) {
        self.shared.mu.unlock();
    }

    // Go: watchmanager.go:81 WatchManager.DoCycleCh
    pub fn do_cycle_ch(&self) -> &DoCycleCh {
        &self.shared.do_cycle_ch
    }

    // Go: watchmanager.go:83 WatchManager.DrainEvents
    /// PORT: Go returns a nil map when nothing changed; the port returns an
    /// empty map.
    pub fn drain_events(&self) -> (FxHashMap<String, fswatch::EventKind>, bool) {
        let mut changed = self.shared.changed_mu.lock().unwrap();
        let changed_paths = changed.changed_paths.take().unwrap_or_default();
        let overflow = changed.changed_overflow;
        changed.changed_overflow = false;
        drop(changed);
        (changed_paths, overflow)
    }

    // Go: watchmanager.go:93 WatchManager.ForceOverflow
    pub fn force_overflow(&self) {
        let mut changed = self.shared.changed_mu.lock().unwrap();
        changed.changed_overflow = true;
    }

    // Go: watchmanager.go:171 WatchManager.CloseAllWatches
    pub fn close_all_watches(&self) {
        self.shared.mu.lock();
        let closers: Vec<Arc<WatchedDir>> = {
            let mut watched_dirs = self.shared.watched_dirs.lock().unwrap();
            let mut closers = Vec::with_capacity(watched_dirs.len());
            for (_dir, wd) in watched_dirs.drain() {
                closers.push(wd);
            }
            closers
        };
        self.shared.mu.unlock();
        for c in &closers {
            c.close_closer();
        }
    }

    // Go: watchmanager.go:184 WatchManager.createDirWatchRequest
    fn create_dir_watch_request(
        &self,
        update: &DirWatchUpdate,
        entry: &Arc<WatchedDir>,
    ) -> WatchDirectoryRequest {
        let shared = self.shared.clone();
        let identity = entry.clone();
        let key = update.key.clone();
        // PORT: the callback runs on the fswatch debouncer thread; it logs
        // to stdout when DebugLog was set when the watch was made (Go reads
        // DebugLog at event time; callers set it before the first watch).
        let debug_log = self.debug_log.is_some();
        WatchDirectoryRequest {
            dir: update.dir.clone(),
            recursive: entry.recursive,
            ignore: Some(Arc::new(should_ignore_watch_path)),
            callback: Arc::new(move |events: Vec<fswatch::Event>, err: Option<GoError>| {
                if let Some(e) = &err {
                    if errors::is(e, &fswatch::ERR_WATCH_TERMINATED) {
                        shared.handle_watch_terminated(debug_log, &key, &identity);
                        return;
                    }
                }
                shared.on_watch_events(debug_log, events, err);
            }),
        }
    }

    // Go: watchmanager.go:199 WatchManager.ResolveDesiredDirs
    pub fn resolve_desired_dirs(
        &self,
        desired_dirs: &FxHashMap<String, bool>,
    ) -> FxHashMap<String, bool> {
        // ts#64159: ancestors that differ only in case are one watch.
        let mut resolved_by_path: FxHashMap<String, DirWatchUpdate> =
            FxHashMap::with_capacity_and_hasher(desired_dirs.len(), Default::default());
        for (dir, recursive) in desired_dirs {
            // ts#64366: Only directories on disk can be watched. The embedded libs (bundled:///libs) exist in the FS but not on disk.
            if !tspath::is_rooted_disk_path(dir) {
                if let Some(debug_log) = &self.debug_log {
                    write_str(debug_log, &format!("[watch] not a disk path: {dir}\n"));
                }
                continue;
            }
            let mut watch_dir = dir.clone();
            let mut watch_recursive = *recursive;
            while !(self.dir_exists)(&watch_dir) {
                let parent = tspath::get_directory_path(&watch_dir);
                if parent == watch_dir {
                    break;
                }
                watch_dir = parent;
                watch_recursive = false; // ancestor fallbacks are always non-recursive
            }
            // ts#64366: CanWatchDirectory only guards against falling back to an ancestor that is too generic to watch
            // (/, /home, ...). A directory that exists and was asked for is watched at any depth, otherwise a
            // project that lives near the filesystem root (say /app or /srv/app) would never be watched.
            if !(self.dir_exists)(&watch_dir)
                || (watch_dir != *dir && !can_watch_directory(&watch_dir))
            {
                if let Some(debug_log) = &self.debug_log {
                    write_str(
                        debug_log,
                        &format!("[watch] no watchable ancestor for {dir}\n"),
                    );
                }
                continue;
            }
            if watch_dir != *dir {
                if let Some(debug_log) = &self.debug_log {
                    write_str(
                        debug_log,
                        &format!("[watch] resolved {dir} to ancestor {watch_dir}\n"),
                    );
                }
            }
            let key = path_key(&watch_dir, self.use_case_sensitive_file_names);
            if let Some(existing) = resolved_by_path.get_mut(&key) {
                existing.recursive = existing.recursive || watch_recursive;
            } else {
                resolved_by_path.insert(
                    key.clone(),
                    DirWatchUpdate {
                        key,
                        dir: watch_dir,
                        recursive: watch_recursive,
                    },
                );
            }
        }
        let mut resolved: FxHashMap<String, bool> =
            FxHashMap::with_capacity_and_hasher(resolved_by_path.len(), Default::default());
        for watch in resolved_by_path.into_values() {
            resolved.insert(watch.dir, watch.recursive);
        }
        resolved
    }

    // Go: watchmanager.go:246 WatchManager.ReconcileWatches
    // PORT: Go ranges over `wm.watchedDirs` while the callbacks delete
    // entries (Go allows that). The port passes a copy of the map and the
    // callbacks change the live map.
    pub fn reconcile_watches(&self, desired_dirs: &FxHashMap<String, bool>) -> Result<(), GoError> {
        if self.backend.is_none() {
            return Ok(());
        }

        // ts#64159: the watches are keyed by path key, so a change of case
        // only does not close and open a watch.
        let mut desired_by_path: FxHashMap<String, DirWatchUpdate> =
            FxHashMap::with_capacity_and_hasher(desired_dirs.len(), Default::default());
        for (dir, recursive) in desired_dirs {
            let key = path_key(dir, self.use_case_sensitive_file_names);
            if let Some(existing) = desired_by_path.get_mut(&key) {
                existing.recursive = existing.recursive || *recursive;
            } else {
                desired_by_path.insert(
                    key.clone(),
                    DirWatchUpdate {
                        key,
                        dir: dir.clone(),
                        recursive: *recursive,
                    },
                );
            }
        }

        let mut additions: Vec<DirWatchUpdate> = Vec::new();
        let mut changes: Vec<DirWatchUpdate> = Vec::new();

        let watched_dirs: FxHashMap<String, Arc<WatchedDir>> =
            self.shared.watched_dirs.lock().unwrap().clone();
        let mut on_added = |_key: &String, desired: &DirWatchUpdate| {
            if let Some(debug_log) = &self.debug_log {
                write_str(
                    debug_log,
                    &format!(
                        "[watch] watching directory {} (recursive={})\n",
                        desired.dir, desired.recursive
                    ),
                );
            }
            additions.push(desired.clone());
        };
        let mut on_removed = |key: &String, wd: &Arc<WatchedDir>| {
            if let Some(debug_log) = &self.debug_log {
                write_str(
                    debug_log,
                    &format!("[watch] closing stale dir watch: {}\n", wd.dir),
                );
            }
            wd.close_closer();
            self.shared.watched_dirs.lock().unwrap().remove(key);
        };
        let mut on_changed = |key: &String, wd: &Arc<WatchedDir>, desired: &DirWatchUpdate| {
            if let Some(debug_log) = &self.debug_log {
                write_str(
                    debug_log,
                    &format!(
                        "[watch] recreating dir watch {} (recursive {}→{})\n",
                        wd.dir, wd.recursive, desired.recursive
                    ),
                );
            }
            wd.close_closer();
            self.shared.watched_dirs.lock().unwrap().remove(key);
            changes.push(desired.clone());
        };
        core_ls_ext::diff_maps_func::<String, Arc<WatchedDir>, DirWatchUpdate>(
            &watched_dirs,
            &desired_by_path,
            |wd: &Arc<WatchedDir>, desired: &DirWatchUpdate| wd.recursive == desired.recursive,
            Some(&mut on_added),
            Some(&mut on_removed),
            Some(&mut on_changed),
        );
        additions.append(&mut changes);
        self.create_dir_watches(additions)
    }

    // Go: watchmanager.go:295 WatchManager.createDirWatches
    fn create_dir_watches(&self, updates: Vec<DirWatchUpdate>) -> Result<(), GoError> {
        if updates.is_empty() {
            return Ok(());
        }
        let mut requests: Vec<WatchDirectoryRequest> = Vec::with_capacity(updates.len());
        let mut entries: Vec<Arc<WatchedDir>> = Vec::with_capacity(updates.len());
        for update in &updates {
            let entry = Arc::new(WatchedDir {
                dir: update.dir.clone(),
                closer: Mutex::new(None),
                recursive: update.recursive,
            });
            requests.push(self.create_dir_watch_request(update, &entry));
            entries.push(entry);
        }
        let backend = self.backend.as_ref().expect("watchmanager: backend is set");
        let err = match backend.watch_directories(requests) {
            Ok(closers) => {
                // PORT: the closers are set before `watched_dirs` is locked,
                // so no closer lock is taken under that lock.
                let mut closers = closers.into_iter();
                for entry in &entries {
                    let closer = closers.next().expect(
                        "index out of range: WatchDirectories returns one closer per request",
                    );
                    *entry.closer.lock().unwrap() = Some(closer);
                }
                let mut watched_dirs = self.shared.watched_dirs.lock().unwrap();
                for (update, entry) in updates.into_iter().zip(entries) {
                    watched_dirs.insert(update.key, entry);
                }
                return Ok(());
            }
            Err(err) => err,
        };
        if let Some(debug_log) = &self.debug_log {
            for update in &updates {
                write_str(
                    debug_log,
                    &format!(
                        "[watch] failed to watch directory {}: {}\n",
                        update.dir,
                        err.error()
                    ),
                );
            }
        }
        Err(err)
    }

    // Go: watchmanager.go:373 WatchManager.IsPathUnderWatch
    // ts#64159: Go `CaseSensitivity.ContainsPath` on the watched spelling,
    // under the manager's case sensitivity.
    pub fn is_path_under_watch(&self, path: &str) -> bool {
        let watched_dirs = self.shared.watched_dirs.lock().unwrap();
        for watch in watched_dirs.values() {
            if tspath::relative_path_within_directory(
                &watch.dir,
                path,
                self.use_case_sensitive_file_names,
            )
            .is_some()
            {
                return true;
            }
        }
        false
    }

    // Go: watchmanager.go:382 WatchManager.RunLoop
    // PORT: Go selects on `ctx.Done()` and `doCycleCh`. Here a waker on
    // `ctx.Done()` ends the wait on the channel (`DoCycleCh::wake`), so a
    // SIGINT or SIGTERM ends the loop at once, as in Go (a wait with a
    // 50 ms timeout ended it up to 50 ms later). When both cases are ready
    // Go picks one at random; the port takes the context. `doCycle` is the
    // caller's DoCycle method value. After a signal, the cycle waits until
    // the debouncer has delivered the fire that sent it
    // (`fswatch::wait_for_fires`), as Go's does: otherwise it can drain a
    // deleted directory's event before that fire's "watch terminated"
    // overflow, and then build a second time.
    pub fn run_loop(&self, ctx: &Context, do_cycle: &mut dyn FnMut()) {
        self.shared.do_cycle_ch.reset_wake();
        let done = ctx.done();
        // `None`: the context is never done (a nil channel), or it is done
        // already, which the first check below sees.
        let waker = done.as_ref().and_then(|done| {
            let shared = self.shared.clone();
            done.register_waker(move || shared.do_cycle_ch.wake())
        });
        loop {
            if ctx.err().is_some() {
                if let (Some(done), Some(id)) = (&done, waker) {
                    done.unregister_waker(id);
                }
                self.close_all_watches();
                return;
            }
            if self.shared.do_cycle_ch.recv() {
                fswatch::wait_for_fires();
                do_cycle();
            }
        }
    }
}

impl WatchManagerShared {
    // Go: watchmanager.go:99 WatchManager.signalDoCycle
    pub fn signal_do_cycle(&self) {
        // Signal sent; the DoCycle loop will pick it up. Or a signal is
        // already pending; coalesced.
        let _ = self.do_cycle_ch.try_send();
    }

    // Go: watchmanager.go:108 WatchManager.onWatchEvents
    // PORT: runs on the fswatch callback thread. `debug_log` is whether Go
    // `wm.DebugLog` is non-nil; the text goes to the process stdout, as does
    // the `warnWriter` warning (see the file comment).
    pub fn on_watch_events(
        &self,
        debug_log: bool,
        events: Vec<fswatch::Event>,
        err: Option<GoError>,
    ) {
        if let Some(err) = err {
            if errors::is(&err, &fswatch::ERR_OVERFLOW) {
                if debug_log {
                    write_stdout("[watch] event overflow, triggering rebuild\n");
                }
                {
                    let mut changed = self.changed_mu.lock().unwrap();
                    changed.changed_overflow = true;
                }
                self.signal_do_cycle();
                return;
            }
            write_stdout(&format!("Warning: File watch error: {}\n", err.error()));
            return;
        }

        if !events.is_empty() {
            if debug_log {
                let mut text = format!("[watch] {} event(s): ", events.len());
                for (i, e) in events.iter().enumerate() {
                    if i > 0 {
                        text.push_str(", ");
                    }
                    if i >= 5 {
                        text.push_str(&format!("... and {} more", events.len() - i));
                        break;
                    }
                    text.push_str(&format!("{} {}", e.kind.string(), e.path));
                }
                text.push('\n');
                write_stdout(&text);
            }
            {
                let mut changed = self.changed_mu.lock().unwrap();
                let changed_paths = changed.changed_paths.get_or_insert_with(|| {
                    FxHashMap::with_capacity_and_hasher(events.len(), Default::default())
                });
                for e in &events {
                    changed_paths.insert(e.path.clone(), e.kind);
                }
            }
            self.signal_do_cycle();
        }
    }

    // Go: watchmanager.go:151 WatchManager.handleWatchTerminated
    // PORT: runs on the fswatch callback thread (see onWatchEvents).
    // ts#64159: `key` is the path key of the watch.
    pub fn handle_watch_terminated(&self, debug_log: bool, key: &str, identity: &Arc<WatchedDir>) {
        if debug_log {
            write_stdout(&format!("[watch] watch terminated: {}\n", identity.dir));
        }
        let mut stale_closer: Option<Arc<WatchedDir>> = None;
        self.mu.lock();
        {
            let mut watched_dirs = self.watched_dirs.lock().unwrap();
            if let Some(wd) = watched_dirs.get(key) {
                if Arc::ptr_eq(wd, identity) {
                    stale_closer = Some(wd.clone());
                    watched_dirs.remove(key);
                }
            }
        }
        self.mu.unlock();
        if let Some(stale_closer) = stale_closer {
            stale_closer.close_closer();
        }
        {
            let mut changed = self.changed_mu.lock().unwrap();
            changed.changed_overflow = true;
        }
        self.signal_do_cycle();
    }
}

// Go: watchmanager.go:326 DirWatchSet
/// DirWatchSet accumulates the set of directories that should be watched while
/// answering coverage queries efficiently. A directory is "covered" when it is
/// already present in the set, or when it is contained within a recursive watch
/// directory already in the set.
// PORT: `names` maps each canonical key to the spelling first registered
// for it (ts#64210).
pub struct DirWatchSet {
    opts: tspath::ComparePathsOptions,
    dirs: FxHashMap<String, bool>,
    names: FxHashMap<String, String>,
}

// Go: watchmanager.go:331 NewDirWatchSet
pub fn new_dir_watch_set(opts: tspath::ComparePathsOptions) -> DirWatchSet {
    DirWatchSet {
        opts,
        dirs: FxHashMap::default(),
        names: FxHashMap::default(),
    }
}

impl DirWatchSet {
    // Go: watchmanager.go:309 DirWatchSet.canonical (at 673a5f17d713;
    // ts#64159 uses CaseSensitivity.PathKey, watchmanager.go:339 in Set)
    fn canonical(&self, dir: &str) -> String {
        path_key(dir, self.opts.use_case_sensitive_file_names)
    }

    // Go: watchmanager.go:338 DirWatchSet.Set (ts#64210)
    pub fn set(&mut self, dir: &str, recursive: bool) {
        let original = dir;
        let dir = self.canonical(dir);
        if !self.names.contains_key(&dir) {
            self.names.insert(dir.clone(), original.to_string());
        }
        let entry = self.dirs.entry(dir).or_insert(false);
        *entry = *entry || recursive;
    }

    // Go: watchmanager.go:348 DirWatchSet.Covered
    pub fn covered(&self, dir: &str) -> bool {
        let mut dir = self.canonical(dir);
        if self.dirs.contains_key(&dir) {
            return true;
        }
        let root_length = tspath::get_root_length(&dir);
        while dir.len() > root_length {
            dir = tspath::get_directory_path(&dir);
            if self.dirs.get(&dir).copied().unwrap_or(false) {
                return true;
            }
        }
        false
    }

    // Go: watchmanager.go:365 DirWatchSet.Dirs (ts#64210)
    // PORT: Go ranges over a map; the result is a map, so the order does
    // not matter.
    pub fn dirs(&self) -> FxHashMap<String, bool> {
        let mut dirs: FxHashMap<String, bool> =
            FxHashMap::with_capacity_and_hasher(self.dirs.len(), Default::default());
        for (key, recursive) in &self.dirs {
            dirs.insert(self.names[key].clone(), *recursive);
        }
        dirs
    }
}

// PORT: Go `fmt.Fprintf(w, ...)` on the callback thread, where `w` is the
// real system's `os.Stdout` (see the file comment): one write of
// `stdio::CliStdout`. Errors are ignored as in Go. `text` is in the port
// form, so this writes its Go bytes.
fn write_stdout(text: &str) {
    let _ = crate::execute::tsc::write_go_output(
        &mut crate::execute::tsc::stdio::CliStdout,
        text.as_bytes(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // PORT: not in Go. `DoCycleCh` keeps the Go channel rule: without a
    // waiting receiver one value fits; a waiting receiver takes the first
    // value, so a second one fits in the buffer.
    #[test]
    fn do_cycle_ch_hands_a_value_to_a_waiting_receiver() {
        let ch = Arc::new(DoCycleCh::default());
        assert!(ch.try_send());
        assert!(!ch.try_send());
        assert!(ch.try_recv());
        assert!(!ch.try_recv());

        let receiver = {
            let ch = ch.clone();
            std::thread::spawn(move || ch.recv())
        };
        while !ch.state.lock().unwrap().waiting {
            std::thread::yield_now();
        }
        assert!(ch.try_send());
        assert!(ch.try_send());
        assert!(!ch.try_send());
        assert!(receiver.join().unwrap());
        assert!(ch.try_recv());
        assert!(!ch.try_recv());
    }

    // PORT: not in Go. `wake` is the `ctx.Done()` case of the `RunLoop`
    // select: it ends a wait in `recv` with no value, and each later one.
    #[test]
    fn do_cycle_ch_wake_ends_the_wait() {
        let ch = Arc::new(DoCycleCh::default());
        let receiver = {
            let ch = ch.clone();
            std::thread::spawn(move || ch.recv())
        };
        while !ch.state.lock().unwrap().waiting {
            std::thread::yield_now();
        }
        ch.wake();
        assert!(!receiver.join().unwrap());
        assert!(!ch.recv());
        ch.reset_wake();
        assert!(ch.try_send());
        assert!(ch.recv());
    }

    // PORT: not in Go. A cancel ends a waiting `run_loop` at once, as Go's
    // select does. The 50 ms wait that the loop had took 25 ms in the
    // median; the bound on the median of 20 runs leaves room for a busy
    // host.
    #[test]
    fn run_loop_ends_at_once_when_its_context_is_cancelled() {
        use crate::gostd::context;
        let wm = new_watch_manager(
            std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new())),
            Box::new(|_| false),
        );
        let mut took = Vec::new();
        for _ in 0..20 {
            let (ctx, cancel) = context::with_cancel(&context::background());
            let canceller = {
                let shared = wm.shared.clone();
                std::thread::spawn(move || {
                    while !shared.do_cycle_ch.state.lock().unwrap().waiting {
                        std::thread::yield_now();
                    }
                    // Into the wait itself, past the `waiting` flag.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    cancel();
                    std::time::Instant::now()
                })
            };
            let mut cycles = 0;
            wm.run_loop(&ctx, &mut || cycles += 1);
            let ended = std::time::Instant::now();
            took.push(ended.saturating_duration_since(canceller.join().unwrap()));
            assert_eq!(cycles, 0);
        }
        took.sort();
        assert!(
            took[10] < std::time::Duration::from_millis(10),
            "median {:?} from the cancel to the end of run_loop",
            took[10]
        );
    }
}
