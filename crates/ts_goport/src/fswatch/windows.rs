//! Go: internal/fswatch/windows.go (the Windows ReadDirectoryChangesW
//! backend).
//!
//! PORT: D-W1 (no `unsafe` in ts_goport). Go's overlapped
//! ReadDirectoryChangesW needs raw pointers (the OVERLAPPED struct, the
//! buffer the kernel fills, the FILE_NOTIFY_INFORMATION chain), so it cannot
//! be written in safe Rust. The port uses the `notify` crate's
//! `ReadDirectoryChangesWatcher` (safe API) as the syscall layer: one
//! `notify` watcher per subscription, as Go has one goroutine per watch. It
//! opens the directory with the same CreateFileW flags, arms the first read
//! before `watch` returns (Go arms it in subscribe), re-arms after each
//! completion and walks the FILE_NOTIFY_INFORMATION chain. Each record comes
//! back as one `notify` event, which the port maps to Go's processOne.
//!
//! PORT divergences from `notify`:
//! - The buffer is 16 KiB (Go: 1 MiB, 64 KiB on a network share), and the
//!   filter adds FILE_NOTIFY_CHANGE_ATTRIBUTES, _CREATION and _SECURITY, so
//!   an attribute change is a MODIFIED record (an update for a file).
//! - `notify` ends a watch on any completion error it does not handle,
//!   without the code. The port sees the end (`MetaEvent::SingleWatchComplete`
//!   that it did not ask for): when the directory is gone it does Go's
//!   ERROR_ACCESS_DENIED branch (the watch terminates as removed), otherwise
//!   Go's default branch (`errUnknown`). So ERROR_NOTIFY_ENUM_DIR (Go:
//!   `ErrOverflow`, the watch goes on) ends the watch with
//!   `ErrWatchTerminated`; the watch manager then rebuilds and watches again,
//!   as it does for an overflow.
//! - A ReadDirectoryChangesW call that fails at once leaves `notify`'s watch
//!   silent (Go: `errReadChanges` from subscribe).
//! - The open error text is Rust's Windows message for the error code (the
//!   "(os error N)" suffix removed), as Go's `windows.Errno` text.

use crate::fswatch::prelude::*;

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use notify::Watcher as _;
use notify::event::{ModifyKind, RenameMode};
use notify::windows::{MetaEvent, ReadDirectoryChangesWatcher};
use notify::{EventKind, RecursiveMode};

use crate::frontend::vfs::osvfs::{go_string_from_os, os_path};
use crate::fswatch::syscall;
use crate::gostd::errors;

// ---------------------------------------------------------------------------
// windows.go: Windows ReadDirectoryChangesW backend
//
// Uses the Win32 ReadDirectoryChangesW API with overlapped (asynchronous)
// I/O to monitor directory trees. Unlike the Unix backends, there is no
// shared event loop; each watch owns its own goroutine that
// independently polls for directory changes.
//
// Goroutines and threading:
//   - One goroutine per watch (run). It blocks in WaitForSingleObject
//     waiting for ReadDirectoryChangesW completions. processCompletion and
//     processOne execute on this goroutine. There is no shared event loop.
//   - subscribe runs on the caller's goroutine. It opens the directory handle,
//     arms the first ReadDirectoryChangesW, and spawns run().
//   - closeWatch runs on the caller's goroutine. It closes stopCh, which
//     triggers CancelIoEx (from a helper goroutine inside run's wait), waking
//     the run goroutine so it can exit cleanly.
//   - fatal() spawns a separate goroutine for handleWatcherError to avoid
//     deadlock: handleWatcherError → closeWatch → wait(doneCh), but doneCh
//     is only closed when run() returns. The indirection lets run() exit first.
//
// PORT: the `notify` watcher's thread waits for the completions and runs
// processOne (through the event handler). The run thread waits for the end
// of the `notify` watch (`MetaEvent::SingleWatchComplete`). Stopping drops
// the `notify` watcher, which cancels the read, closes the handle and then
// reports the end, so closeWatch returns after the handle is closed, as in Go.
//
// Callback delivery:
//   dirWatch.notify() posts to the shared process-wide debouncer. After a
//   coalescing window (50 ms min / 500 ms max), the debouncer invokes all
//   registered WatchCallbacks on its own dedicated goroutine; never on
//   the caller's goroutine or the per-watch goroutine.
//
// Event dispatch (processCompletion / processOne):
//   - FILE_ACTION_ADDED / RENAMED_NEW_NAME  → events.create (→ EventUpdate)
//   - FILE_ACTION_MODIFIED                  → events.update (→ EventUpdate)
//   - FILE_ACTION_REMOVED / RENAMED_OLD_NAME → events.remove + tree.remove
//   Then call dirWatch.notify() to trigger the debouncer.
// ---------------------------------------------------------------------------

// Go: windows.go:106 errGetFileInfo
pub static ERR_GET_FILE_INFO: LazyLock<GoError> =
    LazyLock::new(|| errors::new("could not get file information"));
// Go: windows.go:107 errReadChanges
pub static ERR_READ_CHANGES: LazyLock<GoError> =
    LazyLock::new(|| errors::new("failed to read changes"));
// Go: windows.go:108 errGetOverlappedResult
pub static ERR_GET_OVERLAPPED_RESULT: LazyLock<GoError> =
    LazyLock::new(|| errors::new("GetOverlappedResult failed"));
// Go: windows.go:109 errUnknown
pub static ERR_UNKNOWN: LazyLock<GoError> = LazyLock::new(|| errors::new("unknown error"));

// Go: windows.go:123 windowsBackend
/// windowsBackend.
///
/// PORT: `self_` is the backend's own `Arc`, for fatal's goroutine.
pub struct WindowsBackend {
    pub base: WatcherBase,
    self_: Weak<WindowsBackend>,
}

// Go: windows.go:127 init
// PORT: Go's `init()` sets the factory on the package var `windowsWatcher`;
// the port's package var calls this when it is built.
pub fn init(windows_watcher: &mut WatcherStruct) {
    let factory: WatcherFactory = || -> Arc<dyn WatcherImpl> { new_windows_backend() };
    windows_watcher.factory = Some(factory);
}

// Go: windows.go:131 newWindowsBackend
pub fn new_windows_backend() -> Arc<WindowsBackend> {
    Arc::new_cyclic(|self_: &Weak<WindowsBackend>| {
        let b = WindowsBackend {
            base: WatcherBase::default(),
            self_: self_.clone(),
        };
        let self_impl: Weak<dyn WatcherImpl> = self_.clone();
        b.base.init(self_impl);
        b
    })
}

impl WatcherImpl for WindowsBackend {
    // Go: windows.go:139 windowsBackend.start
    /// start notifies that the watcherImpl is ready. Each watch owns
    /// its own goroutine, so there's no shared event loop to start.
    fn start(&self) -> Result<(), GoError> {
        self.base.notify_started();
        Ok(())
    }

    // Go: windows.go:433 windowsBackend.subscribe
    /// subscribe mirrors `windowsBackend::subscribe`.
    fn subscribe(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        let sub = new_windows_subscription(self, w)?;
        // Arm the first ReadDirectoryChangesW synchronously so that any file
        // operation a caller performs after subscribe returns is guaranteed
        // to be observed. Doing this in run() would race the spawning
        // goroutine with the caller's first filesystem op, occasionally
        // missing the initial create event or seeing it as a stray modify.
        sub.begin_read()?;
        *w.state.lock().unwrap() = Some(Box::new(sub.clone()));
        std::thread::spawn(move || sub.run());
        Ok(())
    }

    // Go: windows.go:459 windowsBackend.closeWatch
    /// closeWatch mirrors `windowsBackend::closeWatch`. Signals the watch
    /// goroutine to stop and waits for it to finish; that way the directory
    /// handle is guaranteed to be closed before this returns, so a follow-on
    /// operation (e.g. immediately re-watching, deleting the directory) sees
    /// a clean slate.
    fn close_watch(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        let state = w.state.lock().unwrap().take();
        let Some(sub) = state.and_then(|s| s.downcast::<Arc<WindowsSubscription>>().ok()) else {
            return Ok(());
        };
        sub.stop();
        sub.done_ch.wait();
        Ok(())
    }

    // Go: windows.go:471 windowsBackend.shutdown
    /// shutdown mirrors `windowsBackend::~windowsBackend`.
    fn shutdown(&self) {
        // Nothing to do; each watch owns its goroutine and is stopped
        // by closeWatch.
    }

    fn base(&self) -> &WatcherBase {
        &self.base
    }
}

// Go: windows.go:145 windowsSubscription
/// windowsSubscription.
///
/// PORT: Go's `handle`, `bufBytes` and `first` are the `notify` watcher
/// (`WindowsSubscriptionLocked::watcher`) and the channel that reports the
/// end of its watch (`meta_rx`). Go's `stopCh` woke the helper goroutine
/// that cancels the read; dropping the `notify` watcher does that here.
pub struct WindowsSubscription {
    pub mu: Mutex<WindowsSubscriptionLocked>,
    pub watcher_impl: Weak<WindowsBackend>,
    pub dir_watch: Arc<DirWatch>,
    pub done_ch: SignalChan,
    meta_rx: Mutex<Option<mpsc::Receiver<MetaEvent>>>,
}

/// PORT: the `windowsSubscription` fields that Go `s.mu` guards.
pub struct WindowsSubscriptionLocked {
    pub stopped: bool,
    pub watcher: Option<ReadDirectoryChangesWatcher>,
}

// PORT: Go `windows.Errno.Error()` is the system message. Rust's text is the
// same message followed by " (os error N)".
fn win_error_text(err: &std::io::Error) -> String {
    let text = err.to_string();
    match (err.raw_os_error(), text.rfind(" (os error ")) {
        (Some(_), Some(i)) => text[..i].to_string(),
        _ => text,
    }
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

// Go: windows.go:163 newWindowsSubscription
/// PORT: Go opens the directory handle here (CreateFile) and checks
/// FILE_ATTRIBUTE_DIRECTORY. The `notify` watcher opens the handle in
/// beginRead; the checks are `std::fs::metadata` (the same CreateFile open
/// and GetFileInformationByHandle).
pub fn new_windows_subscription(
    watcher_impl: &WindowsBackend,
    w: &Arc<DirWatch>,
) -> Result<Arc<WindowsSubscription>, GoError> {
    let info = match std::fs::metadata(os_path(&w.physical_dir)) {
        Ok(info) => info,
        Err(err) => {
            let err = errors::new(win_error_text(&err));
            return Err(DirWatchError {
                err: errors::errorf(format!("invalid handle: {}", err.error()), vec![err]),
                dir_watch: w.clone(),
            }
            .to_go_error());
        }
    };
    if !info.is_dir() {
        return Err(DirWatchError {
            err: errors::from_value(syscall::ENOTDIR),
            dir_watch: w.clone(),
        }
        .to_go_error());
    }
    Ok(Arc::new(WindowsSubscription {
        mu: Mutex::new(WindowsSubscriptionLocked {
            stopped: false,
            watcher: None,
        }),
        watcher_impl: watcher_impl.self_.clone(),
        dir_watch: w.clone(),
        done_ch: SignalChan::new(),
        meta_rx: Mutex::new(None),
    }))
}

// PORT: Go `defer close(s.doneCh)` in run.
struct DoneGuard<'a>(&'a SignalChan);

impl Drop for DoneGuard<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl WindowsSubscription {
    // Go: windows.go:200 windowsSubscription.beginRead
    /// PORT: Go arms one ReadDirectoryChangesW and returns the request; the
    /// `notify` watcher arms the first one in `watch` and the next ones
    /// itself. Returns false where Go returns a nil request (stopped).
    pub fn begin_read(self: &Arc<Self>) -> Result<bool, GoError> {
        let mut l = self.mu.lock().unwrap();
        if l.stopped {
            return Ok(false);
        }

        let (meta_tx, meta_rx) = mpsc::channel();
        let sub = Arc::downgrade(self);
        let handler: Arc<Mutex<dyn notify::EventHandler>> =
            Arc::new(Mutex::new(move |res: notify::Result<notify::Event>| {
                if let (Some(sub), Ok(event)) = (sub.upgrade(), res) {
                    sub.process_completion(&event);
                }
            }));
        let read_changes_error = || {
            DirWatchError {
                err: ERR_READ_CHANGES.clone(),
                dir_watch: self.dir_watch.clone(),
            }
            .to_go_error()
        };
        let mut watcher = ReadDirectoryChangesWatcher::create(handler, meta_tx)
            .map_err(|_| read_changes_error())?;
        let mode = if self.dir_watch.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(Path::new(&*os_path(&self.dir_watch.physical_dir)), mode)
            .map_err(|_| read_changes_error())?;
        l.watcher = Some(watcher);
        *self.meta_rx.lock().unwrap() = Some(meta_rx);
        Ok(true)
    }

    // Go: windows.go:261 windowsSubscription.run
    /// run is the per-watch goroutine. It loops on ReadDirectoryChangesW
    /// until the watch is stopped or an unrecoverable error occurs.
    ///
    /// PORT: it waits for the end of the `notify` watch. An end that stop()
    /// did not ask for is a completion error that `notify` did not handle;
    /// the port runs Go's error branches for it (see the file comment).
    pub fn run(self: Arc<Self>) {
        let _done = DoneGuard(&self.done_ch);
        let meta_rx = self.meta_rx.lock().unwrap().take();
        let Some(meta_rx) = meta_rx else {
            // subscribe always arms the initial read before spawning run.
            // Guard the invariant rather than silently producing a watch
            // that delivers neither events nor errors if it ever breaks.
            self.fatal(
                DirWatchError {
                    err: errors::new("fswatch: windows: missing initial read"),
                    dir_watch: self.dir_watch.clone(),
                }
                .to_go_error(),
            );
            return;
        };
        // PORT: a closed channel means the `notify` thread is gone.
        while let Ok(event) = meta_rx.recv() {
            if !matches!(event, MetaEvent::SingleWatchComplete) {
                continue;
            }
            if self.mu.lock().unwrap().stopped {
                return;
            }
            // Possibly the watched dir was deleted; check and handle.
            let exists =
                std::fs::metadata(os_path(&self.dir_watch.physical_dir)).is_ok_and(|m| m.is_dir());
            if !exists {
                self.dir_watch.events.remove(&self.dir_watch.dir);
                self.dir_watch.events.set_error(watched_directory_removed());
                self.dir_watch.notify();
                self.stop();
                return;
            }
            self.fatal(
                DirWatchError {
                    err: ERR_UNKNOWN.clone(),
                    dir_watch: self.dir_watch.clone(),
                }
                .to_go_error(),
            );
            return;
        }
    }

    // Go: windows.go:320 windowsSubscription.processCompletion
    /// processCompletion mirrors the body of `Watch::processEvents` for
    /// the cases that translate cleanly to Go's overlapped wrapper.
    ///
    /// PORT: called on the `notify` thread with one record of the
    /// FILE_NOTIFY_INFORMATION chain (Go walks the chain here). The event
    /// path is the watched physical directory joined with the record's name.
    pub fn process_completion(&self, event: &notify::Event) {
        if self.mu.lock().unwrap().stopped {
            return;
        }
        let physical_dir = os_path(&self.dir_watch.physical_dir);
        for path in &event.paths {
            let Ok(name) = path.strip_prefix(&*physical_dir) else {
                continue;
            };
            self.process_one(&event.kind, &go_string_from_os(name.as_os_str()));
        }
        self.dir_watch.notify();
    }

    // Go: windows.go:376 windowsSubscription.processOne
    /// PORT: Go switches on the FILE_ACTION_* code; `notify` names them
    /// ADDED `Create`, RENAMED_NEW_NAME `Modify(Name(To))`, MODIFIED
    /// `Modify(Any)`, REMOVED `Remove` and RENAMED_OLD_NAME `Modify(Name(From))`.
    pub fn process_one(&self, kind: &EventKind, name: &str) {
        let path = format!("{}\\{name}", self.dir_watch.dir);
        let watch_path = format!("{}\\{name}", self.dir_watch.physical_dir);
        match kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                // Always emit the event, even if the file is already gone by the
                // time we look it up. The kernel told us it was added, and a
                // subsequent REMOVED needs to find this entry in the eventList so
                // the create+delete pair coalesces away.
                self.dir_watch.events.create(&path);
            }
            EventKind::Modify(ModifyKind::Any) => {
                // PORT: Go GetFileAttributesEx (it does not follow a reparse
                // point): `symlink_metadata` and FILE_ATTRIBUTE_DIRECTORY.
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
                if let Ok(data) = std::fs::symlink_metadata(os_path(&watch_path)) {
                    if data.file_attributes() & FILE_ATTRIBUTE_DIRECTORY == 0 {
                        self.dir_watch.events.update(&path);
                    }
                }
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                let seq = self.dir_watch.events.remove_and_get_sequence(&path);
                if self.dir_watch.terminate_callbacks_for_deleted_root(
                    &path,
                    seq,
                    watched_directory_removed(),
                ) {
                    self.dir_watch.notify();
                }
            }
            _ => {}
        }
    }

    // Go: windows.go:408 windowsSubscription.fatal
    /// fatal is invoked when the run goroutine hits an unrecoverable error.
    /// handleWatcherError eventually calls closeWatch which waits on doneCh,
    /// but doneCh isn't closed until run() returns. Calling handleWatcherError
    /// synchronously from inside run() would deadlock. Spawn a goroutine to do
    /// the cleanup so run() can exit and unblock the wait.
    pub fn fatal(&self, err: GoError) {
        let werr = DirWatchError {
            err,
            dir_watch: self.dir_watch.clone(),
        };
        if let Some(watcher_impl) = self.watcher_impl.upgrade() {
            std::thread::spawn(move || watcher_impl.handle_watcher_error(werr));
        }
        self.stop();
    }

    // Go: windows.go:414 windowsSubscription.stopLocked
    /// PORT: Go closes stopCh and cancels the read (CancelIoEx); dropping the
    /// `notify` watcher cancels it, closes the handle and ends run's wait.
    pub fn stop_locked(l: &mut WindowsSubscriptionLocked) {
        if l.stopped {
            return;
        }
        l.stopped = true;
        l.watcher = None;
    }

    // Go: windows.go:426 windowsSubscription.stop
    pub fn stop(&self) {
        let mut l = self.mu.lock().unwrap();
        Self::stop_locked(&mut l);
    }
}
