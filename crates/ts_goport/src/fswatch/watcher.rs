//! Go: internal/fswatch/watcher.go (the `Watcher` API, the package
//! watchers, the shared backend base and the per-directory watch state).
//!
//! PORT: threads. Go runs one goroutine per backend event loop and one per
//! debouncer; callbacks run on the debouncer goroutine. The port uses
//! `std::thread` and shares state through `Arc`. Each Go `sync.Mutex` is a
//! `std::sync::Mutex` over the fields it guards (`*Locked` structs). A Go
//! `*dirWatch` map key is the `Arc` pointer (`Arc::as_ptr(..) as usize`).
//!
//! PORT: D-W1 (no `libc`, no `unsafe`). The Linux and kqueue backends call
//! the `fswatch::unix` shim of their targets (safe syscall crates). The
//! Windows backend uses the `notify` crate's ReadDirectoryChangesW watcher
//! (see windows.rs). The FSEvents backend is not ported: its package watcher
//! keeps a `None` factory, so `available()` is false, as on Linux in Go, and
//! `Default()` on macOS picks kqueue (Go's fallback when FSEvents is not
//! available).

use crate::fswatch::prelude::*;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::frontend::vfs::osvfs::{filepath_clean, go_string_from_os, os_path};
use crate::fswatch::pathcompare::{ComparisonCache, ComparisonPath, PathComparer};
use crate::fswatch::syscall;
use crate::fswatch::walkdir::path_error;
use crate::gostd::{errors, strconv};

// Go: watcher.go:17 errNilCallback
pub static ERR_NIL_CALLBACK: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: callback must not be nil"));

// Go: watcher.go:21 errRootPath
/// errRootPath is returned by WatchFile when the supplied path is a
/// filesystem root with no parent directory to watch.
pub static ERR_ROOT_PATH: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: cannot watch a root path"));

// Go: watcher.go:25 errNotAbsolute
/// errNotAbsolute is returned by [Watcher.WatchDirectory] and
/// [Watcher.WatchFile] when the supplied path is not absolute.
pub static ERR_NOT_ABSOLUTE: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: path must be absolute"));

// Go: watcher.go:31 ErrOverflow
/// ErrOverflow indicates that the kernel event queue overflowed and
/// some filesystem changes were missed. The watch remains
/// active; further events will continue to be delivered. Callers
/// should treat this as a signal to rescan the watched directory.
pub static ERR_OVERFLOW: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: event overflow; some changes were missed"));

// Go: watcher.go:37 ErrWatchTerminated
/// ErrWatchTerminated indicates that the watch was terminated due to
/// an unrecoverable error (e.g. the watched directory was deleted or
/// the watch descriptor was revoked). No further events will be
/// delivered. Call Close to release remaining state.
pub static ERR_WATCH_TERMINATED: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: watch terminated"));

// Go: watcher.go:41 ErrUnavailable
/// ErrUnavailable indicates that a requested watcher is not
/// available on the current platform.
pub static ERR_UNAVAILABLE: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: watcher not available on this platform"));

// Go: watcher.go:50 ErrFilesystemUnsupported
/// ErrFilesystemUnsupported indicates that the active watcher backend cannot
/// operate on the target filesystem, even though the backend is available on
/// the current platform. This happens, for example, with the fanotify backend
/// on filesystems that do not support FID-based watching: name_to_handle_at
/// returning EOPNOTSUPP (some Docker bind mounts backed by virtiofs, gRPC FUSE,
/// or overlayfs) or fanotify_mark returning ENODEV (e.g. NTFS mounted via
/// fuseblk).
pub static ERR_FILESYSTEM_UNSUPPORTED: LazyLock<GoError> =
    LazyLock::new(|| errors::new("fswatch: watcher backend unsupported on this filesystem"));

// Go: watcher.go:58 Watcher
/// Watcher represents a filesystem watching implementation.
/// Use one of the constructor functions ([Inotify], [FSEvents], [Kqueue],
/// [Windows]) to obtain a value, or [Default] for the platform default.
///
/// All watchers exist on every platform. Subscribing with a watcher that
/// is not supported on the current OS returns [ErrUnavailable].
///
/// PORT: Go `WatchDirectory` checks `fn == nil`; a Rust `WatchCallback` is
/// never nil, so `ERR_NIL_CALLBACK` is not returned. Go returns the
/// `Watch` interface; the port returns `Box<dyn Watch>`.
pub trait Watcher: Send + Sync {
    /// Name returns a stable identifier ("inotify", "fsevents", "kqueue",
    /// "windows").
    fn name(&self) -> String;
    /// Available reports whether this watcher works on the current OS.
    fn available(&self) -> bool;
    /// HasFastRecursiveBackend reports whether this watcher supports efficient
    /// recursive watching without requiring a full userspace tree walk. This is
    /// true for Windows (ReadDirectoryChangesW subtree mode) and macOS FSEvents
    /// (inherently recursive), and false for all other backends.
    fn has_fast_recursive_backend(&self) -> bool;
    /// WatchDirectory watches dir for changes, calling fn with batched
    /// events. By default, only direct children are watched. Use
    /// [WithRecursive] to watch the entire directory tree.
    /// dir must be an absolute path to an existing directory. If dir is a
    /// symlink or reparse point to a directory, the OS subscription follows
    /// the target directory but delivered event paths remain rooted at dir.
    /// Userspace recursive traversal does not follow symlinked descendant
    /// directories.
    /// Returns [ErrUnavailable] if the watcher is not supported on
    /// the current platform.
    fn watch_directory(
        &self,
        dir: &str,
        fn_: WatchCallback,
        opts: &[Box<dyn WatchOption>],
    ) -> Result<Box<dyn Watch>, GoError>;
    /// WatchDirectories watches multiple directories as a batch. It has the
    /// same semantics as calling [Watcher.WatchDirectory] for each request, but
    /// lets backends arm the underlying OS watches once for the whole batch.
    /// Returned watches are in the same order as requests.
    fn watch_directories(
        &self,
        requests: &[WatchDirectoryRequest<'_>],
    ) -> Result<Vec<Box<dyn Watch>>, GoError>;
    /// WatchFile watches a single file for changes, calling fn with
    /// batched events. path must be an absolute path. The file does not
    /// need to exist at subscribe time; its creation will be reported.
    /// The parent directory must exist.
    ///
    /// Multiple WatchFile calls for files in the same directory
    /// share a single OS watch on the parent directory.
    ///
    /// If the parent directory is deleted, [ErrWatchTerminated] is
    /// delivered and the watch is dead. Unlike TypeScript's
    /// watchFile (which falls back to polling for missing entries),
    /// there is no automatic recovery. Callers that need to survive
    /// parent directory deletion should handle [ErrWatchTerminated]
    /// and re-subscribe when the directory is recreated.
    ///
    /// Returns [ErrUnavailable] if the watcher is not supported on
    /// the current platform.
    fn watch_file(&self, path: &str, fn_: WatchCallback) -> Result<Box<dyn Watch>, GoError>;
    fn unexported(&self);
}

// Go: watcher.go:107 WatchOption
/// WatchOption configures a watch.
pub trait WatchOption: Send + Sync {
    fn apply_watch_option(&self, opts: &mut WatchOptions);
}

// Go: watcher.go:113 WatchDirectoryRequest
/// WatchDirectoryRequest describes one directory subscription in a
/// [Watcher.WatchDirectories] batch.
///
/// PORT: the Go `[]WatchOption` slice is a borrowed slice.
pub struct WatchDirectoryRequest<'a> {
    pub dir: String,
    pub callback: WatchCallback,
    pub options: &'a [Box<dyn WatchOption>],
}

// Go: watcher.go:119 watchOptions
#[derive(Clone, Default)]
pub struct WatchOptions {
    pub ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    pub recursive: bool,
    // ts#64210
    pub file: String,
}

// Go: watcher.go:127 fileOption (ts#64210)
/// fileOption defers the file filter until the parent directory's comparer is
/// available, so WatchFile does not need a second filesystem query.
pub struct FileOption {
    pub path: String,
}

impl WatchOption for FileOption {
    // Go: watcher.go:131 fileOption.applyWatchOption
    fn apply_watch_option(&self, opts: &mut WatchOptions) {
        opts.file = self.path.clone();
    }
}

// Go: watcher.go:135 ignoreOption
pub struct IgnoreOption {
    pub fn_: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl WatchOption for IgnoreOption {
    // Go: watcher.go:139 ignoreOption.applyWatchOption
    fn apply_watch_option(&self, opts: &mut WatchOptions) {
        opts.ignore = Some(self.fn_.clone());
    }
}

// Go: watcher.go:147 WithIgnore
/// WithIgnore returns a [WatchOption] that filters events before delivery.
/// If the function returns true for a path, events for that path are
/// silently dropped. The filtering is per-subscriber; multiple watches
/// on the same directory may have different ignore functions.
pub fn with_ignore(fn_: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> Box<dyn WatchOption> {
    Box::new(IgnoreOption { fn_ })
}

// Go: watcher.go:151 recursiveOption
pub struct RecursiveOption;

impl WatchOption for RecursiveOption {
    // Go: watcher.go:153 recursiveOption.applyWatchOption
    fn apply_watch_option(&self, opts: &mut WatchOptions) {
        opts.recursive = true;
    }
}

// Go: watcher.go:166 WithRecursive
/// WithRecursive returns a [WatchOption] that enables recursive watching
/// of the entire directory tree. Without this option,
/// [Watcher.WatchDirectory] watches only direct children of dir.
///
/// In recursive mode, events for all descendants at any depth are
/// delivered. On inotify/fanotify, a watch descriptor is added for
/// every subdirectory. On kqueue, an fd is opened for every entry.
/// On Windows, bWatchSubtree=TRUE is passed to ReadDirectoryChangesW.
/// On FSEvents, the kernel is inherently recursive.
pub fn with_recursive() -> Box<dyn WatchOption> {
    Box::new(RecursiveOption)
}

// Go: watcher.go:172 Watch
/// Watch represents a live watch. Close stops watching
/// and releases resources. It is idempotent.
///
/// PORT: Go `io.Closer` users (watchmanager, lspwatcher) hold a
/// `Box<dyn Watch>`.
pub trait Watch: Send + Sync {
    fn close(&self) -> Result<(), GoError>;
    fn unexported(&self);
}

/// PORT: Go `error` in the callback. Plan decision D-CTX: one error type,
/// `GoError`; test sentinels with `errors::is(&err, &ERR_OVERFLOW)`.
pub type WatchError = GoError;

// Go: watcher.go:185 WatchCallback
/// WatchCallback receives batched filesystem events. Rapid changes
/// are coalesced before delivery.
///
/// For a given Watch, the callback is never invoked concurrently
/// with itself. It runs on a library goroutine, not the caller's.
///
/// When err is non-nil, use [errors.Is] to check for [ErrOverflow]
/// (recoverable) or [ErrWatchTerminated] (terminal).
///
/// PORT: Go `nil` events are an empty `Vec`.
pub type WatchCallback = Arc<dyn Fn(Vec<Event>, Option<WatchError>) + Send + Sync>;

/// PORT: Go `func() watcherImpl` factory.
pub type WatcherFactory = fn() -> Arc<dyn WatcherImpl>;

// Go: watcher.go:188 package-level watcher instances
// Package-level watcher instances. Platform init() functions set the factory.
//
// PORT: Go package vars set up by the platform `init()` functions. The
// port builds each one on first use and calls the platform `init` there
// (inotify and fanotify on Linux, FSEvents on macOS, kqueue on darwin and
// the BSDs, windows on Windows).
pub static INOTIFY_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("inotify", |w| {
        #[cfg(target_os = "linux")]
        crate::fswatch::inotify_linux::init(w);
    })
});
pub static FSEVENTS_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("fsevents", |_w| {
        #[cfg(target_os = "macos")]
        crate::fswatch::fsevents_darwin::init(_w);
    })
});
pub static KQUEUE_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("kqueue", |_w| {
        #[cfg(any(
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        crate::fswatch::kqueue::init(_w);
    })
});
pub static WINDOWS_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("windows", |_w| {
        #[cfg(windows)]
        crate::fswatch::windows::init(_w);
    })
});
pub static FANOTIFY_WATCHER: LazyLock<Arc<WatcherStruct>> = LazyLock::new(|| {
    new_watcher("fanotify", |w| {
        #[cfg(target_os = "linux")]
        crate::fswatch::fanotify_linux::init(w);
    })
});
pub static FANOTIFY_FALLBACK_WATCHER: LazyLock<Arc<FallbackWatcher>> = LazyLock::new(|| {
    Arc::new(FallbackWatcher {
        primary: FANOTIFY_WATCHER.clone(),
        secondary: INOTIFY_WATCHER.clone(),
    })
});

// PORT: Go `&watcher{name: name}` composite literal, followed by the
// platform `init()` that sets the factory. The watcher also keeps a weak
// pointer to its own `Arc`, which Go gets from the `*watcher` receiver.
pub fn new_watcher(name: &str, init: impl FnOnce(&mut WatcherStruct)) -> Arc<WatcherStruct> {
    Arc::new_cyclic(|self_: &Weak<WatcherStruct>| {
        let mut w = WatcherStruct {
            name: name.to_string(),
            mu: Mutex::new(WatcherStructLocked::default()),
            factory: None,
            self_: self_.clone(),
        };
        init(&mut w);
        w
    })
}

// Go: watcher.go:203 AllWatchers
/// AllWatchers returns a fresh slice listing every watcher backend the package
/// knows about. Use [Watcher.Available] to check which ones work on the current
/// OS.
pub fn all_watchers() -> Vec<Arc<dyn Watcher>> {
    let inotify_watcher: Arc<dyn Watcher> = INOTIFY_WATCHER.clone();
    let fsevents_watcher: Arc<dyn Watcher> = FSEVENTS_WATCHER.clone();
    let kqueue_watcher: Arc<dyn Watcher> = KQUEUE_WATCHER.clone();
    let windows_watcher: Arc<dyn Watcher> = WINDOWS_WATCHER.clone();
    let fanotify_fallback_watcher: Arc<dyn Watcher> = FANOTIFY_FALLBACK_WATCHER.clone();
    vec![
        inotify_watcher,
        fsevents_watcher,
        kqueue_watcher,
        windows_watcher,
        fanotify_fallback_watcher,
    ]
}

// Go: watcher.go:214 Inotify
/// Inotify returns the inotify watcher (Linux and Android).
pub fn inotify() -> Arc<dyn Watcher> {
    INOTIFY_WATCHER.clone()
}

// Go: watcher.go:217 FSEvents
/// FSEvents returns the FSEvents watcher (macOS).
pub fn fs_events() -> Arc<dyn Watcher> {
    FSEVENTS_WATCHER.clone()
}

// Go: watcher.go:220 Kqueue
/// Kqueue returns the kqueue watcher (macOS, FreeBSD, and other BSDs).
pub fn kqueue() -> Arc<dyn Watcher> {
    KQUEUE_WATCHER.clone()
}

// Go: watcher.go:223 Windows
/// Windows returns the ReadDirectoryChangesW watcher (Windows).
pub fn windows() -> Arc<dyn Watcher> {
    WINDOWS_WATCHER.clone()
}

// Go: watcher.go:227 Fanotify
/// Fanotify returns the fanotify watcher (Linux, kernel ≥ 5.13). Directories on
/// filesystems that don't support fanotify watches automatically use inotify instead.
pub fn fanotify() -> Arc<dyn Watcher> {
    FANOTIFY_FALLBACK_WATCHER.clone()
}

// Go: watcher.go:230 Default
/// Default returns the recommended watcher for the current OS.
///
/// PORT: Go `runtime.GOOS` is `std::env::consts::OS` ("macos" for Go
/// "darwin"). On Linux this returns fanotify when the `fanotify_init` probe
/// succeeds and inotify when it fails, as Go does. On macOS it returns
/// FSEvents (fsevents_darwin.rs, on the `notify` crate).
pub fn default() -> Arc<dyn Watcher> {
    match std::env::consts::OS {
        "linux" => {
            if fanotify().available() {
                return fanotify();
            }
            inotify()
        }
        "android" => inotify(),
        "macos" => {
            if fs_events().available() {
                return fs_events();
            }
            kqueue()
        }
        "windows" => windows(),
        "freebsd" | "openbsd" | "netbsd" | "dragonfly" => kqueue(),
        _ => new_watcher("unsupported", |_| {}),
    }
}

// Go: watcher.go:255 fallbackWatcher
/// fallbackWatcher keeps the primary backend for supported filesystems while
/// routing individual unsupported watches to the secondary backend.
pub struct FallbackWatcher {
    pub primary: Arc<dyn Watcher>,
    pub secondary: Arc<dyn Watcher>,
}

impl Watcher for FallbackWatcher {
    // Go: watcher.go:260 fallbackWatcher.Name
    fn name(&self) -> String {
        self.primary.name()
    }

    // Go: watcher.go:261 fallbackWatcher.Available
    fn available(&self) -> bool {
        self.primary.available()
    }

    // Go: watcher.go:262 fallbackWatcher.HasFastRecursiveBackend
    fn has_fast_recursive_backend(&self) -> bool {
        self.primary.has_fast_recursive_backend()
    }

    // Go: watcher.go:264 fallbackWatcher.WatchDirectory
    fn watch_directory(
        &self,
        dir: &str,
        fn_: WatchCallback,
        opts: &[Box<dyn WatchOption>],
    ) -> Result<Box<dyn Watch>, GoError> {
        let watches = self.watch_directories(&[WatchDirectoryRequest {
            dir: dir.to_string(),
            callback: fn_,
            options: opts,
        }])?;
        Ok(watches
            .into_iter()
            .next()
            .expect("WatchDirectories returns one watch per request"))
    }

    // Go: watcher.go:276 fallbackWatcher.WatchDirectories
    fn watch_directories(
        &self,
        requests: &[WatchDirectoryRequest<'_>],
    ) -> Result<Vec<Box<dyn Watch>>, GoError> {
        let err = match self.primary.watch_directories(requests) {
            Ok(watches) => return Ok(watches),
            Err(err) => err,
        };
        if !errors::is(&err, &ERR_FILESYSTEM_UNSUPPORTED) {
            return Err(err);
        }

        let mut watches: Vec<Box<dyn Watch>> = Vec::with_capacity(requests.len());
        let rollback = |watches: &[Box<dyn Watch>]| {
            for watch in watches.iter().rev() {
                let _ = watch.close();
            }
        };
        for request in requests {
            let mut result = self.primary.watch_directory(
                &request.dir,
                request.callback.clone(),
                request.options,
            );
            if let Err(err) = &result {
                if errors::is(err, &ERR_FILESYSTEM_UNSUPPORTED) {
                    result = self.secondary.watch_directory(
                        &request.dir,
                        request.callback.clone(),
                        request.options,
                    );
                }
            }
            match result {
                Ok(watch) => watches.push(watch),
                Err(err) => {
                    rollback(&watches);
                    return Err(errors::errorf(
                        format!(
                            "fswatch: failed to watch directory {}: {}",
                            strconv::quote(&request.dir),
                            err.error()
                        ),
                        vec![err],
                    ));
                }
            }
        }
        Ok(watches)
    }

    // Go: watcher.go:302 fallbackWatcher.WatchFile
    fn watch_file(&self, path: &str, fn_: WatchCallback) -> Result<Box<dyn Watch>, GoError> {
        let result = self.primary.watch_file(path, fn_.clone());
        if let Err(err) = &result {
            if errors::is(err, &ERR_FILESYSTEM_UNSUPPORTED) {
                return self.secondary.watch_file(path, fn_);
            }
        }
        result
    }

    // Go: watcher.go:310 fallbackWatcher.unexported
    fn unexported(&self) {}
}

// Go: watcher.go:315 watcher
/// watcher is the concrete implementation of [Watcher]. Each platform
/// watcher is a package-level *watcher whose factory is set by the
/// platform's init() function.
///
/// PORT: named `WatcherStruct` because the Go interface `Watcher` and the
/// struct `watcher` have the same Rust name. Go `mu` guards the fields in
/// `WatcherStructLocked`. `factory` is set once, before the watcher is
/// shared. Go `sequence` is set only by the FSEvents backend
/// (fsevents_darwin.go), to the FSEvents event ID. The port's FSEvents
/// backend gets no event IDs from `notify`, so it is always nil and is not a
/// field here.
pub struct WatcherStruct {
    pub name: String,
    pub mu: Mutex<WatcherStructLocked>,
    /// nil if not available on this platform
    pub factory: Option<WatcherFactory>,
    /// PORT: the `*watcher` receiver as an `Arc` (see `new_watcher`).
    pub self_: Weak<WatcherStruct>,
}

/// PORT: the `watcher` fields that Go `w.mu` guards.
#[derive(Default)]
pub struct WatcherStructLocked {
    pub impl_: Option<Arc<dyn WatcherImpl>>,
    /// Go `map[string]*dirWatch`; `None` is Go's nil map.
    pub dir_watches: Option<FxHashMap<String, Arc<DirWatch>>>,
    /// lazily created in getOrCreateDirWatch
    pub debounce: Option<Arc<Debounce>>,
}

// Go: watcher.go:325 recursiveConsolidateThreshold
pub const RECURSIVE_CONSOLIDATE_THRESHOLD: usize = 10;

impl WatcherStruct {
    // Go: watcher.go:328 watcher.String
    pub fn string(&self) -> String {
        self.name.clone()
    }

    // Go: watcher.go:342 watcher.canShareRecursiveDirWatches
    pub fn can_share_recursive_dir_watches(&self) -> bool {
        // TODO: Re-enable this for Windows once coalesced recursive watches have
        // more real-world bake time.
        self.name == "fsevents"
    }

    // PORT: Go uses the `*watcher` receiver pointer; the port upgrades the
    // weak self pointer (the watcher is alive while `self` is borrowed).
    fn self_arc(&self) -> Arc<WatcherStruct> {
        self.self_.upgrade().expect("fswatch: watcher is alive")
    }

    // Go: watcher.go:348 watcher.getImpl
    pub fn get_impl(&self) -> Result<Arc<dyn WatcherImpl>, GoError> {
        let factory = {
            let w = self.mu.lock().unwrap();
            if let Some(impl_) = &w.impl_ {
                let impl_ = impl_.clone();
                return Ok(impl_);
            }
            self.factory
        };

        let Some(factory) = factory else {
            return Err(ERR_UNAVAILABLE.clone());
        };

        let impl_ = factory();
        impl_.run()?;

        let mut w = self.mu.lock().unwrap();
        let existing = w.impl_.clone();
        if let Some(existing) = existing {
            drop(w);
            impl_.shutdown();
            return Ok(existing);
        }
        w.impl_ = Some(impl_.clone());
        drop(w);
        Ok(impl_)
    }

    // Go: watcher.go:378 watcher.keyForDirWatch
    pub fn key_for_dir_watch(&self, dir: &str, recursive: bool) -> String {
        if recursive {
            return format!("{dir}\x00recursive");
        }
        dir.to_string()
    }

    // Go: watcher.go:385 watcher.findCoveringRecursiveWatchLocked (ts#64210: takes the comparer)
    // PORT: takes the data that `w.mu` guards.
    pub fn find_covering_recursive_watch_locked(
        &self,
        w: &WatcherStructLocked,
        dir: &str,
        physical_dir: &str,
        comparer: PathComparer,
    ) -> Option<Arc<DirWatch>> {
        let mut best: Option<&Arc<DirWatch>> = None;
        for dw in w.dir_watches.iter().flat_map(|m| m.values()) {
            if !dw.recursive
                || dw.comparer != comparer
                || !is_in_directory_or_self(&dw.dir, dir)
                || !is_in_directory_or_self(&dw.physical_dir, physical_dir)
            {
                continue;
            }
            if best.is_none_or(|best| dw.dir.len() > best.dir.len()) {
                best = Some(dw);
            }
        }
        best.cloned()
    }

    // Go: watcher.go:398 watcher.findConsolidationDirLocked
    // PORT: takes the data that `w.mu` guards.
    pub fn find_consolidation_dir_locked(
        &self,
        w: &WatcherStructLocked,
        dir: &str,
        physical_dir: &str,
    ) -> String {
        if !self.can_share_recursive_dir_watches() {
            return String::new();
        }
        let mut dir = dir.to_string();
        let mut parent = filepath_dir(&dir);
        while parent != dir && parent != "." {
            if filepath_dir(&parent) == parent {
                break;
            }
            let physical_parent = physical_dir_for(&parent);
            if !is_in_directory_or_self(&physical_parent, physical_dir) {
                return String::new();
            }
            let mut count = 1;
            for dw in w.dir_watches.iter().flat_map(|m| m.values()) {
                if is_in_directory_or_self(&parent, &dw.dir)
                    && is_in_directory_or_self(&physical_parent, &dw.physical_dir)
                {
                    count += 1;
                    if count >= RECURSIVE_CONSOLIDATE_THRESHOLD {
                        return parent;
                    }
                }
            }
            let next = filepath_dir(&parent);
            if next == parent {
                break;
            }
            dir = parent;
            parent = next;
        }
        String::new()
    }

    // Go: watcher.go:430 watcher.getOrCreateDirWatch (ts#64210: takes the comparer, returns an error)
    pub fn get_or_create_dir_watch(
        &self,
        dir: &str,
        physical_dir: &str,
        recursive: bool,
        comparer: PathComparer,
    ) -> Result<Arc<DirWatch>, GoError> {
        let mut w = self.mu.lock().unwrap();
        if w.dir_watches.is_none() {
            w.dir_watches = Some(FxHashMap::default());
        }
        if w.debounce.is_none() {
            w.debounce = Some(new_debounce());
        }

        let mut dir = dir.to_string();
        let mut physical_dir = physical_dir.to_string();
        let mut recursive = recursive;
        if self.can_share_recursive_dir_watches() {
            if let Some(dw) =
                self.find_covering_recursive_watch_locked(&w, &dir, &physical_dir, comparer)
            {
                return Ok(dw);
            }
            let consolidation_dir = self.find_consolidation_dir_locked(&w, &dir, &physical_dir);
            if !consolidation_dir.is_empty() {
                let parent_comparer = self.path_comparer(&consolidation_dir)?;
                if parent_comparer == comparer {
                    dir = consolidation_dir;
                    physical_dir = physical_dir_for(&dir);
                    recursive = true;
                    if let Some(dw) =
                        self.find_covering_recursive_watch_locked(&w, &dir, &physical_dir, comparer)
                    {
                        return Ok(dw);
                    }
                }
            }
        }

        let key = self.key_for_dir_watch(&dir, recursive);
        if let Some(dw) = w.dir_watches.as_ref().unwrap().get(&key) {
            return Ok(dw.clone());
        }
        // PORT: Go sets `dw.recursive = recursive` and calls
        // `dw.setComparer(comparer)` right after newDirWatch, before the
        // dirWatch is shared; the port passes both in. Go also sets
        // `dw.sequence = w.sequence`, which is always nil here (see
        // `WatcherStruct`).
        let dw = new_dir_watch(
            &dir,
            &physical_dir,
            w.debounce.clone().unwrap(),
            recursive,
            comparer,
        );
        w.dir_watches.as_mut().unwrap().insert(key, dw.clone());
        Ok(dw)
    }

    // Go: watcher.go:472 watcher.removeDirWatch
    pub fn remove_dir_watch(&self, dw: &DirWatch) {
        let mut w = self.mu.lock().unwrap();
        let key = self.key_for_dir_watch(&dw.dir, dw.recursive);
        let same = match w.dir_watches.as_ref().and_then(|m| m.get(&key)) {
            Some(existing) => std::ptr::eq(Arc::as_ptr(existing), dw),
            None => false,
        };
        if same {
            w.dir_watches.as_mut().unwrap().remove(&key);
            dw.destroy_debounce();
        }
    }
}

impl Watcher for WatcherStruct {
    // Go: watcher.go:327 watcher.Name
    fn name(&self) -> String {
        self.name.clone()
    }

    // Go: watcher.go:329 watcher.Available
    fn available(&self) -> bool {
        self.factory.is_some()
    }

    // Go: watcher.go:330 watcher.unexported
    fn unexported(&self) {}

    // Go: watcher.go:333 watcher.HasFastRecursiveBackend
    /// HasFastRecursiveBackend implements [Watcher.HasFastRecursiveBackend].
    fn has_fast_recursive_backend(&self) -> bool {
        match self.name.as_str() {
            "windows" | "fsevents" => true,
            _ => false,
        }
    }

    // Go: watcher.go:482 watcher.WatchDirectory
    fn watch_directory(
        &self,
        dir: &str,
        fn_: WatchCallback,
        opts: &[Box<dyn WatchOption>],
    ) -> Result<Box<dyn Watch>, GoError> {
        let watches = self.watch_directories(&[WatchDirectoryRequest {
            dir: dir.to_string(),
            callback: fn_,
            options: opts,
        }])?;
        Ok(watches
            .into_iter()
            .next()
            .expect("WatchDirectories returns one watch per request"))
    }

    // Go: watcher.go:494 watcher.WatchDirectories (ts#64210: the path comparer)
    fn watch_directories(
        &self,
        requests: &[WatchDirectoryRequest<'_>],
    ) -> Result<Vec<Box<dyn Watch>>, GoError> {
        if !self.available() {
            return Err(ERR_UNAVAILABLE.clone());
        }
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        struct PreparedWatch {
            dw: Arc<DirWatch>,
            id: u64,
            recursive: bool,
            dir: String,
        }
        let mut prepared: Vec<PreparedWatch> = Vec::with_capacity(requests.len());
        let mut unique_dir_watches: Vec<Arc<DirWatch>> = Vec::with_capacity(requests.len());
        // PORT: Go `map[*dirWatch]struct{}`, keyed by the `Arc` pointer.
        let mut seen_dir_watches: FxHashSet<usize> = FxHashSet::default();
        let rollback = |prepared: &[PreparedWatch]| {
            for p in prepared.iter().rev() {
                p.dw.unwatch(p.id);
                p.dw.unref(self);
            }
        };

        for request in requests {
            let dir = &request.dir;
            let fn_ = request.callback.clone();
            // PORT: Go `if fn == nil { rollback(); return nil, errNilCallback }`;
            // a Rust callback is never nil.
            let mut dir = filepath_clean(dir);
            if !filepath_is_abs(&dir) {
                rollback(&prepared);
                return Err(ERR_NOT_ABSOLUTE.clone());
            }
            dir = canonicalize_path(&dir);
            if self.can_share_recursive_dir_watches() {
                if let Err(err) = validate_watch_directory(&dir) {
                    rollback(&prepared);
                    return Err(err);
                }
            }
            let physical_dir = physical_dir_for(&dir);

            let mut sopts = WatchOptions::default();
            for o in request.options {
                o.apply_watch_option(&mut sopts);
            }

            // ts#64210
            let comparer = match self.path_comparer(&dir) {
                Ok(comparer) => comparer,
                Err(err) => {
                    rollback(&prepared);
                    return Err(err);
                }
            };
            let dw = match self.get_or_create_dir_watch(
                &dir,
                &physical_dir,
                sopts.recursive,
                comparer,
            ) {
                Ok(dw) => dw,
                Err(err) => {
                    rollback(&prepared);
                    return Err(err);
                }
            };
            let (id, _) = dw.add_callback(
                &dir,
                &physical_dir,
                sopts.recursive,
                fn_,
                sopts.ignore.clone(),
                &sopts.file,
            );
            prepared.push(PreparedWatch {
                dw: dw.clone(),
                id,
                recursive: sopts.recursive,
                dir,
            });
            if seen_dir_watches.insert(Arc::as_ptr(&dw) as usize) {
                unique_dir_watches.push(dw);
            }
        }

        let impl_ = match self.get_impl() {
            Ok(impl_) => impl_,
            Err(err) => {
                rollback(&prepared);
                return Err(err);
            }
        };
        if let Err(err) = impl_.watch_add_many(&unique_dir_watches) {
            rollback(&prepared);
            return Err(err);
        }

        let mut watches: Vec<Box<dyn Watch>> = Vec::with_capacity(prepared.len());
        for p in prepared {
            watches.push(Box::new(WatchStruct {
                mu: Mutex::new(false),
                w: self.self_arc(),
                dw: p.dw,
                impl_: impl_.clone(),
                id: p.id,
            }));
        }
        Ok(watches)
    }

    // Go: watcher.go:591 watcher.WatchFile (ts#64210: fileOption)
    fn watch_file(&self, path: &str, fn_: WatchCallback) -> Result<Box<dyn Watch>, GoError> {
        // PORT: Go `if fn == nil { return nil, errNilCallback }`; a Rust
        // callback is never nil.
        if !self.available() {
            return Err(ERR_UNAVAILABLE.clone());
        }
        let mut path = filepath_clean(path);
        if !filepath_is_abs(&path) {
            return Err(ERR_NOT_ABSOLUTE.clone());
        }
        path = canonicalize_path(&path);
        let dir = filepath_dir(&path);
        if dir == path {
            return Err(ERR_ROOT_PATH.clone());
        }

        // ts#64210: the parent directory watch filters for the file.
        self.watch_directory(&dir, fn_, &[Box::new(FileOption { path })])
    }
}

// Go: watcher.go:580 validateWatchDirectory
pub fn validate_watch_directory(dir: &str) -> Result<(), GoError> {
    let info = match std::fs::metadata(os_path(dir)) {
        Ok(info) => info,
        Err(err) => return Err(path_error("stat", dir, &err)),
    };
    if !info.is_dir() {
        return Err(errors::from_value(syscall::ENOTDIR));
    }
    Ok(())
}

// Go: path/filepath/path_unix.go IsAbs
// PORT: Go standard library (unix). The crate's `filepath_clean` is the
// unix `filepath.Clean` too (the Windows one on Windows).
#[cfg(not(windows))]
fn filepath_is_abs(path: &str) -> bool {
    path.starts_with('/')
}

// Go: internal/filepathlite/path_windows.go IsAbs
// PORT: Go standard library (windows).
#[cfg(windows)]
fn filepath_is_abs(path: &str) -> bool {
    use crate::frontend::vfs::osvfs::{filepath_volume_name_len, win_is_path_separator};
    let b = path.as_bytes();
    let l = filepath_volume_name_len(b);
    if l == 0 {
        return false;
    }
    // If the volume name starts with a double slash, this is an absolute path.
    if win_is_path_separator(b[0]) && win_is_path_separator(b[1]) {
        return true;
    }
    let rest = &b[l..];
    !rest.is_empty() && win_is_path_separator(rest[0])
}

// Go: path/filepath/path.go Dir
// PORT: Go standard library (windows: `VolumeName` is the volume prefix
// with slashes made separators).
#[cfg(windows)]
fn filepath_dir(path: &str) -> String {
    use crate::frontend::vfs::osvfs::{filepath_volume_name_len, win_is_path_separator};
    let vol_len = filepath_volume_name_len(path.as_bytes());
    let vol = path[..vol_len].replace('/', "\\");
    let bytes = path.as_bytes();
    let mut i = bytes.len() as isize - 1;
    while i >= vol_len as isize && !win_is_path_separator(bytes[i as usize]) {
        i -= 1;
    }
    let dir = filepath_clean(&path[vol_len..(i + 1) as usize]);
    if dir == "." && vol.len() > 2 {
        // must be UNC
        return vol;
    }
    format!("{vol}{dir}")
}

// Go: path/filepath/path.go Dir
// PORT: Go standard library (unix: `VolumeName` is empty).
#[cfg(not(windows))]
fn filepath_dir(path: &str) -> String {
    let vol = "";
    let bytes = path.as_bytes();
    let mut i = bytes.len() as isize - 1;
    while i >= vol.len() as isize && bytes[i as usize] != b'/' {
        i -= 1;
    }
    let dir = filepath_clean(&path[vol.len()..(i + 1) as usize]);
    if dir == "." && vol.len() > 2 {
        // must be UNC
        return vol.to_string();
    }
    format!("{vol}{dir}")
}

// Go: watcher.go:611 watch
/// PORT: named `WatchStruct` because the Go interface `Watch` and the
/// struct `watch` have the same Rust name. Go `mu` guards `cancelled`,
/// which is the `bool` inside `mu`.
pub struct WatchStruct {
    pub mu: Mutex<bool>,
    pub w: Arc<WatcherStruct>,
    pub dw: Arc<DirWatch>,
    pub impl_: Arc<dyn WatcherImpl>,
    pub id: u64,
}

impl Watch for WatchStruct {
    // Go: watcher.go:620 watch.Close
    fn close(&self) -> Result<(), GoError> {
        let mut cancelled = self.mu.lock().unwrap();
        if *cancelled {
            return Ok(());
        }
        *cancelled = true;
        let last = self.dw.unwatch(self.id);
        if last {
            self.impl_.watch_remove(&self.dw);
            self.dw.unref(&self.w);
        }
        Ok(())
    }

    // Go: watcher.go:635 watch.unexported
    fn unexported(&self) {}
}

// Go: watcher.go:638 watcherImpl
/// watcherImpl is the internal interface implemented by each platform watcher.
///
/// PORT: Go backends embed `watcherBase`, whose methods are promoted. The
/// default methods here forward to `base()` (the embedded value); a
/// backend overrides a method the way a Go backend declares its own.
pub trait WatcherImpl: Send + Sync {
    fn start(&self) -> Result<(), GoError>;
    fn run(&self) -> Result<(), GoError> {
        self.base().run()
    }
    fn shutdown(&self) {
        self.base().shutdown();
    }

    fn watch_add(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        self.base().watch_add(w)
    }
    fn watch_add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), GoError> {
        self.base().watch_add_many(watches)
    }
    fn watch_remove(&self, w: &Arc<DirWatch>) {
        self.base().watch_remove(w);
    }
    fn handle_watcher_error(&self, err: DirWatchError) {
        self.base().handle_watcher_error(err);
    }

    fn subscribe(&self, w: &Arc<DirWatch>) -> Result<(), GoError>;
    fn close_watch(&self, w: &Arc<DirWatch>) -> Result<(), GoError>;

    /// Go: the optional `subscribeMany` method that `watchAddMany` looks
    /// for (only the FSEvents backend has it). `None`: the backend has none.
    fn subscribe_many(&self, _watches: &[Arc<DirWatch>]) -> Option<Result<(), GoError>> {
        None
    }

    /// PORT: the embedded Go `watcherBase`.
    fn base(&self) -> &WatcherBase;
}

/// PORT: Go `chan struct{}` that is only closed and received from (a
/// one-shot signal). Go replaces such a channel with `make` to reuse it;
/// the port makes a new `SignalChan`.
pub struct SignalChan {
    closed: Mutex<bool>,
    cond: Condvar,
}

impl Default for SignalChan {
    fn default() -> Self {
        SignalChan::new()
    }
}

impl SignalChan {
    /// Go `make(chan struct{})`.
    pub fn new() -> SignalChan {
        SignalChan {
            closed: Mutex::new(false),
            cond: Condvar::new(),
        }
    }

    /// Go `close(ch)`. Closing a closed channel panics, as in Go.
    pub fn close(&self) {
        let mut closed = self.closed.lock().unwrap();
        if *closed {
            panic!("close of closed channel");
        }
        *closed = true;
        self.cond.notify_all();
    }

    /// Go `select { case <-ch: ...; default: ... }`: true when closed.
    pub fn is_closed(&self) -> bool {
        *self.closed.lock().unwrap()
    }

    /// Go `<-ch`: blocks until the channel is closed.
    pub fn wait(&self) {
        let mut closed = self.closed.lock().unwrap();
        while !*closed {
            closed = self.cond.wait(closed).unwrap();
        }
    }

    /// Go `select { case <-ch: ...; case <-time.After(d): ... }`: true when
    /// the channel is closed before `d` passes.
    pub fn wait_timeout(&self, d: Duration) -> bool {
        let closed = self.closed.lock().unwrap();
        let (closed, _) = self
            .cond
            .wait_timeout_while(closed, d, |closed| !*closed)
            .unwrap();
        *closed
    }
}

// Go: watcher.go:654 watcherBase
/// watcherBase provides shared watch-tracking and lifecycle logic.
/// Concrete backends embed it and override subscribe/closeWatch/start.
///
/// PORT: Go `mu` guards `WatcherBaseLocked`. The Go backends also hold
/// `b.mu` (this lock, promoted) while they touch their own fields; the
/// port keeps those fields in a second mutex that is always taken after
/// this one.
#[derive(Default)]
pub struct WatcherBase {
    pub mu: Mutex<WatcherBaseLocked>,
    pub started: SignalChan,

    /// back-reference for virtual dispatch
    pub self_: OnceLock<Weak<dyn WatcherImpl>>,
}

/// PORT: the `watcherBase` fields that Go `b.mu` guards.
#[derive(Default)]
pub struct WatcherBaseLocked {
    /// Go `map[*dirWatch]struct{}`, keyed by the `Arc` pointer.
    pub subscriptions: FxHashMap<usize, Arc<DirWatch>>,
    pub start_err: Option<GoError>,
}

impl WatcherBase {
    // Go: watcher.go:663 watcherBase.init
    // PORT: `started` is made with the value (`Default`).
    pub fn init(&self, self_: Weak<dyn WatcherImpl>) {
        let _ = self.self_.set(self_);
        self.mu.lock().unwrap().subscriptions = FxHashMap::default();
    }

    // PORT: Go reads `b.self`; the port upgrades the weak back-reference.
    fn self_impl(&self) -> Arc<dyn WatcherImpl> {
        self.self_
            .get()
            .and_then(Weak::upgrade)
            .expect("fswatch: watcherBase.init was called and the backend is alive")
    }

    // Go: watcher.go:669 watcherBase.notifyStarted
    pub fn notify_started(&self) {
        if self.started.is_closed() {
            // Do nothing; already started.
        } else {
            self.started.close();
        }
    }

    // Go: watcher.go:678 watcherBase.shutdown
    pub fn shutdown(&self) {}

    // Go: watcher.go:680 watcherBase.run
    // PORT: the goroutine is a `std::thread`; Go's `recover()` is
    // `catch_unwind`, so an `unported!` panic in `start` becomes the start
    // error, as a Go panic does. The thread gets the Go stack size: an
    // event for a new directory walks it (`walk_dir`), one call per level.
    pub fn run(&self) -> Result<(), GoError> {
        let self_impl = self.self_impl();
        let thread = std::thread::Builder::new().stack_size(crate::gostd::stack::max_stack_size());
        thread
            .spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| self_impl.start()));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => self_impl.base().handle_start_error(err),
                    Err(r) => {
                        // Go: err, ok := r.(error); if !ok { err = fmt.Errorf("%v", r) }
                        let text = if let Some(panic) = r.downcast_ref::<crate::core::GoPanic>() {
                            panic.message.clone()
                        } else if let Some(s) = r.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = r.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            format!("{r:?}")
                        };
                        self_impl
                            .base()
                            .handle_start_error(errors::errorf(text, vec![]));
                    }
                }
            })
            .expect("failed to spawn watcher thread");
        self.started.wait();
        let b = self.mu.lock().unwrap();
        match &b.start_err {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    // Go: watcher.go:701 watcherBase.handleStartError
    pub fn handle_start_error(&self, err: GoError) {
        let subs: Vec<Arc<DirWatch>> = {
            let mut b = self.mu.lock().unwrap();
            b.start_err = Some(err.clone());
            let mut subs = Vec::with_capacity(b.subscriptions.len());
            for w in b.subscriptions.values() {
                subs.push(w.clone());
            }
            subs
        };
        for w in &subs {
            w.notify_error(err.clone());
        }
        self.notify_started();
    }

    // Go: watcher.go:715 watcherBase.watchAdd
    pub fn watch_add(&self, w: &Arc<DirWatch>) -> Result<(), GoError> {
        self.watch_add_many(std::slice::from_ref(w))
    }

    // Go: watcher.go:719 watcherBase.watchAddMany
    pub fn watch_add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), GoError> {
        let mut b = self.mu.lock().unwrap();
        let mut to_add: Vec<&Arc<DirWatch>> = Vec::with_capacity(watches.len());
        for w in watches {
            if b.subscriptions.contains_key(&(Arc::as_ptr(w) as usize)) {
                continue;
            }
            to_add.push(w);
        }
        if to_add.is_empty() {
            return Ok(());
        }

        let self_impl = self.self_impl();
        let batch: Vec<Arc<DirWatch>> = to_add.iter().map(|w| (*w).clone()).collect();
        if let Some(result) = self_impl.subscribe_many(&batch) {
            result?;
            for w in batch {
                b.subscriptions.insert(Arc::as_ptr(&w) as usize, w);
            }
            return Ok(());
        }
        let mut added: Vec<&Arc<DirWatch>> = Vec::with_capacity(to_add.len());
        for w in to_add {
            if let Err(err) = self_impl.subscribe(w) {
                for added_watch in added {
                    b.subscriptions.remove(&(Arc::as_ptr(added_watch) as usize));
                    let _ = self_impl.close_watch(added_watch);
                }
                return Err(err);
            }
            b.subscriptions.insert(Arc::as_ptr(w) as usize, w.clone());
            added.push(w);
        }
        Ok(())
    }

    // Go: watcher.go:764 watcherBase.watchRemove
    pub fn watch_remove(&self, w: &Arc<DirWatch>) {
        let mut b = self.mu.lock().unwrap();
        let key = Arc::as_ptr(w) as usize;
        if !b.subscriptions.contains_key(&key) {
            return;
        }
        b.subscriptions.remove(&key);
        let _ = self.self_impl().close_watch(w);
    }

    // Go: watcher.go:775 watcherBase.handleWatcherError
    pub fn handle_watcher_error(&self, werr: DirWatchError) {
        self.watch_remove(&werr.dir_watch);
        let dir_watch = werr.dir_watch.clone();
        let text = format!("{}: {}", ERR_WATCH_TERMINATED.error(), werr.error());
        dir_watch.notify_error(errors::errorf(
            text,
            vec![ERR_WATCH_TERMINATED.clone(), werr.to_go_error()],
        ));
    }
}

// ----- dirWatch: per-directory watch state -------------------------

// Go: watcher.go:782 callback
#[derive(Clone)]
pub struct Callback {
    pub id: u64,
    pub dir: String,
    pub physical_dir: String,
    pub watch_dir: String,
    pub watch_physical_dir: String,
    pub recursive: bool,
    pub fn_: WatchCallback,
    pub ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    pub since_seq: u64,
    pub terminal: Option<GoError>,
    pub delivered: bool,
    // ts#64210
    pub comparer: PathComparer,
    pub dir_comparison: ComparisonPath<'static>,
    pub physical_comparison: ComparisonPath<'static>,
    pub file_comparison: ComparisonPath<'static>,
}

// Go: watcher.go:801 dirWatchError
/// dirWatchError associates an error with a specific directory watch.
#[derive(Clone)]
pub struct DirWatchError {
    pub err: GoError,
    pub dir_watch: Arc<DirWatch>,
}

impl DirWatchError {
    // Go: watcher.go:806 dirWatchError.Error
    pub fn error(&self) -> String {
        self.err.error()
    }

    // Go: watcher.go:807 dirWatchError.Unwrap
    pub fn unwrap(&self) -> GoError {
        self.err.clone()
    }

    /// PORT: Go returns the `*dirWatchError` as an `error`. The value keeps
    /// its `Error()` text, its `Unwrap()` result and its type
    /// (`errors::as_type::<DirWatchError>`).
    pub fn to_go_error(&self) -> GoError {
        errors::from_value_with_unwrap(self.clone(), self.unwrap())
    }
}

impl std::fmt::Display for DirWatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

impl std::fmt::Debug for DirWatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dirWatchError({:?}, {:?})",
            self.dir_watch.dir,
            self.err.error()
        )
    }
}

/// PORT: Go compares `*dirWatchError` pointers. The port compares the
/// wrapped error value and the dirWatch pointer.
impl PartialEq for DirWatchError {
    fn eq(&self, other: &DirWatchError) -> bool {
        self.err == other.err && Arc::ptr_eq(&self.dir_watch, &other.dir_watch)
    }
}

// Go: watcher.go:811 dirWatch
/// dirWatch holds per-directory state: pending events, registered callbacks,
/// and a reference to the shared debouncer. Each watched directory has one.
///
/// PORT: Go `mu` guards the fields in `DirWatchLocked`. `state` is only
/// used by the fsevents and Windows backends. Go
/// `sequence` is always nil here (see `WatcherStruct`), so it is not a field.
/// Go sets `comparer`, `dirFold` and `physicalDirFold` with `setComparer`
/// before the dirWatch is shared; the port sets them in `new_dir_watch`.
pub struct DirWatch {
    /// dir is the caller-visible watch root used in delivered event paths.
    pub dir: String,
    /// physicalDir is the path passed to OS watcher APIs. It differs from dir
    /// when dir or an ancestor is a symlink or reparse point to a directory.
    pub physical_dir: String,
    pub recursive: bool,
    pub events: EventList,
    // ts#64210
    pub comparer: PathComparer,
    pub dir_fold: String,
    pub physical_dir_fold: String,

    /// state stores per-directory platform-specific bookkeeping (fsevents, windows).
    pub state: Mutex<Option<Box<dyn Any + Send>>>,

    pub mu: Mutex<DirWatchLocked>,
}

/// PORT: the `dirWatch` fields that Go `dw.mu` guards.
#[derive(Default)]
pub struct DirWatchLocked {
    pub callbacks: Vec<Callback>,
    pub debounce: Option<Arc<Debounce>>,
    pub next_cbid: u64,
}

// Go: watcher.go:834 newDirWatch
// PORT: `recursive` and `comparer` are parameters (see getOrCreateDirWatch):
// Go sets them on the new dirWatch before it is shared, the comparer with
// `setComparer`. The debounce key is the dirWatch pointer.
pub fn new_dir_watch(
    dir: &str,
    physical_dir: &str,
    db: Arc<Debounce>,
    recursive: bool,
    comparer: PathComparer,
) -> Arc<DirWatch> {
    // Go: watcher.go:841 dirWatch.setComparer (ts#64210)
    let dir_fold = comparer.prepare(dir).folded;
    let physical_dir_fold = if physical_dir == dir {
        dir_fold.clone()
    } else {
        comparer.prepare(physical_dir).folded
    };
    let dw = Arc::new(DirWatch {
        dir: dir.to_string(),
        physical_dir: physical_dir.to_string(),
        recursive,
        events: EventList::default(),
        comparer,
        dir_fold,
        physical_dir_fold,
        state: Mutex::new(None),
        mu: Mutex::new(DirWatchLocked::default()),
    });
    dw.mu.lock().unwrap().debounce = Some(db.clone());
    let dw_cb = dw.clone();
    db.add(
        Arc::as_ptr(&dw) as usize,
        Arc::new(move || dw_cb.trigger_callbacks()),
    );
    dw
}

// Go: watcher.go:854 physicalDirFor
/// physicalDirFor returns the physical path to watch for dir. If dir, or an
/// ancestor of dir, is a symlink or reparse point, events are subscribed on its
/// realpath while callbacks still use dir.
pub fn physical_dir_for(dir: &str) -> String {
    let realpath = match crate::frontend::nativepath::realpath(dir) {
        Ok(realpath) => realpath,
        Err(_) => return dir.to_string(),
    };
    if realpath == dir {
        return dir.to_string();
    }
    canonicalize_path(&filepath_clean(&realpath))
}

// Go: os.IsPathSeparator
// PORT: Go standard library. On unix only '/'; `MAIN_SEPARATOR` adds '\'
// on Windows, as Go does.
fn is_path_separator(c: u8) -> bool {
    c == b'/' || c == std::path::MAIN_SEPARATOR as u8
}

// Go: watcher.go:879 rebasePath
/// rebasePath replaces the from root in path with to, preserving any child
/// suffix. Prefix matches must end at a path separator so sibling paths like
/// "/foo2" are not rebased from "/foo".
pub fn rebase_path(path: &str, from: &str, to: &str) -> String {
    if from == to {
        return path.to_string();
    }
    if path == from {
        return to.to_string();
    }
    let Some(suffix) = path.strip_prefix(from) else {
        return path.to_string();
    };
    if !from.is_empty() && is_path_separator(from.as_bytes()[from.len() - 1]) {
        return join_path_suffix(to, suffix);
    }
    if suffix.is_empty() || !is_path_separator(suffix.as_bytes()[0]) {
        return path.to_string();
    }
    join_path_suffix(to, suffix)
}

// Go: watcher.go:899 joinPathSuffix
pub fn join_path_suffix(root: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        return root.to_string();
    }
    let root_ends_with_separator =
        !root.is_empty() && is_path_separator(root.as_bytes()[root.len() - 1]);
    if is_path_separator(suffix.as_bytes()[0]) {
        if root_ends_with_separator {
            return format!("{root}{}", &suffix[1..]);
        }
        return format!("{root}{suffix}");
    }
    if root_ends_with_separator {
        return format!("{root}{suffix}");
    }
    format!("{root}{}{suffix}", std::path::MAIN_SEPARATOR)
}

impl DirWatch {
    // Go: watcher.go:867 dirWatch.displayPath
    /// displayPath maps a physical event path back under the caller-visible
    /// watch root.
    pub fn display_path(&self, watch_path: &str) -> String {
        rebase_path(watch_path, &self.physical_dir, &self.dir)
    }

    // Go: watcher.go:872 dirWatch.physicalPath
    /// physicalPath maps a caller-visible path to the physical watched root.
    pub fn physical_path(&self, display_path: &str) -> String {
        rebase_path(display_path, &self.dir, &self.physical_dir)
    }

    // Go: watcher.go:915 dirWatch.destroyDebounce
    pub fn destroy_debounce(&self) {
        let db = {
            let mut dw = self.mu.lock().unwrap();
            dw.debounce.take()
        };
        if let Some(db) = db {
            db.remove(self as *const DirWatch as usize);
        }
    }

    // Go: watcher.go:925 dirWatch.notify
    pub fn notify(&self) {
        let (has_pending_cbs, has_terminal, has_events, has_error, db) = {
            let dw = self.mu.lock().unwrap();
            let has_pending_cbs = dw.callbacks.iter().any(|cb| !cb.delivered);
            let has_terminal = dw
                .callbacks
                .iter()
                .any(|cb| cb.terminal.is_some() && !cb.delivered);
            let has_events = self.events.size() > 0;
            let has_error = self.events.has_error();
            (
                has_pending_cbs,
                has_terminal,
                has_events,
                has_error,
                dw.debounce.clone(),
            )
        };

        if has_pending_cbs && (has_events || has_error || has_terminal) {
            if let Some(db) = db {
                db.trigger();
            }
        }
    }

    // Go: watcher.go:943 dirWatch.notifyError
    pub fn notify_error(&self, err: GoError) {
        let cbs = {
            let mut dw = self.mu.lock().unwrap();
            let cbs = dw.callbacks.clone();
            dw.callbacks = Vec::new();
            cbs
        };
        for cb in &cbs {
            (cb.fn_)(Vec::new(), Some(err.clone()));
        }
    }

    // Go: watcher.go:953 dirWatch.triggerCallbacks
    pub fn trigger_callbacks(&self) {
        let (events_by_callback, err, cbs) = {
            let mut dw = self.mu.lock().unwrap();
            let has_error = self.events.has_error();
            let has_events = self.events.size() > 0;
            let mut cbs: Vec<Callback> = Vec::with_capacity(dw.callbacks.len());
            let mut has_terminal = false;
            for cb in &dw.callbacks {
                if cb.delivered {
                    continue;
                }
                if cb.terminal.is_some() {
                    has_terminal = true;
                }
                cbs.push(cb.clone());
            }
            if cbs.is_empty() {
                if has_events || has_error {
                    let _ = self.events.drain();
                }
                return;
            }
            if !has_events && !has_error && !has_terminal {
                return;
            }
            let start_seqs: Vec<u64> = cbs.iter().map(|cb| cb.since_seq).collect();
            let (events_by_callback, err) = self.events.drain_for_sequences(&start_seqs);
            for cb in &cbs {
                if cb.terminal.is_none() {
                    continue;
                }
                if let Some(c) = dw.callbacks.iter_mut().find(|c| c.id == cb.id) {
                    c.delivered = true;
                }
            }
            (events_by_callback, err, cbs)
        };

        // ts#64210
        let comparisons: Mutex<ComparisonCache> = Mutex::new(ComparisonCache::default());
        for (cb, cb_events) in cbs.iter().zip(events_by_callback) {
            let mut filtered: Vec<Event> = Vec::with_capacity(cb_events.len());
            let filter = cb.ignore.is_some()
                || !cb.recursive
                || cb.dir != self.dir
                || !cb.file_comparison.path.is_empty();
            for (e, included_watch_root) in cb_events {
                if !filter {
                    filtered.push(e);
                    continue;
                }
                let mut e = cb.map_event_cached(e, Some(&comparisons));
                if !cb.file_comparison.path.is_empty() {
                    let mut path = ComparisonPath {
                        path: e.path.clone(),
                        cache: Some(&comparisons),
                        ..Default::default()
                    };
                    let (suffix, ok) = cb.comparer.suffix_prepared(&cb.file_comparison, &mut path);
                    if !ok || !suffix.is_empty() {
                        continue;
                    }
                    e.path = cb.file_comparison.path.clone();
                }
                if let Some(ignore) = &cb.ignore {
                    if ignore(&e.path) {
                        continue;
                    }
                }
                if cb.dir != self.dir
                    && !included_watch_root
                    && e.path == cb.dir
                    && e.kind == EventKind::Update
                {
                    continue;
                }
                if cb.recursive {
                    if cb.dir != self.dir && !is_in_directory_or_self(&cb.dir, &e.path) {
                        continue;
                    }
                } else if !is_direct_child(&cb.dir, &e.path)
                    && !(cb.dir != self.dir && e.path == cb.dir)
                {
                    continue;
                }
                filtered.push(e);
            }
            let mut cb_err = err.clone();
            if cb.terminal.is_some() {
                cb_err = cb.terminal.clone();
            }
            if !filtered.is_empty() || cb_err.is_some() {
                (cb.fn_)(filtered, cb_err);
            }
        }
    }

    // Go: watcher.go:1063 dirWatch.terminateCallbacksForDeletedRoot
    pub fn terminate_callbacks_for_deleted_root(&self, path: &str, seq: u64, err: GoError) -> bool {
        let mut dw = self.mu.lock().unwrap();
        let mut changed = false;
        // ts#64210
        let comparisons: Mutex<ComparisonCache> = Mutex::new(ComparisonCache::default());
        let deleted = ComparisonPath {
            path: path.to_string(),
            cache: Some(&comparisons),
            ..Default::default()
        };
        for cb in dw.callbacks.iter_mut() {
            if cb.delivered || cb.terminal.is_some() || cb.since_seq >= seq {
                continue;
            }
            let physical_path = ComparisonPath {
                path: cb.event_physical_path(path),
                cache: Some(&comparisons),
                ..Default::default()
            };
            let (mut dir, mut physical) =
                (cb.dir_comparison.clone(), cb.physical_comparison.clone());
            let (_, logical_match) = cb.comparer.suffix_prepared(&deleted, &mut dir);
            let (_, physical_match) = cb.comparer.suffix_prepared(&physical_path, &mut physical);
            if logical_match || physical_match {
                cb.terminal = Some(err.clone());
                changed = true;
            }
        }
        changed
    }
}

impl Callback {
    // Go: watcher.go:1038 callback.mapEvent
    pub fn map_event(&self, e: Event) -> Event {
        self.map_event_cached(e, None)
    }

    // Go: watcher.go:1042 callback.mapEventCached (ts#64210)
    pub fn map_event_cached(&self, mut e: Event, cache: Option<&Mutex<ComparisonCache>>) -> Event {
        if !self.physical_dir.is_empty()
            && (self.physical_dir != self.dir || self.comparer.ignore_case)
        {
            let mut physical_path = ComparisonPath {
                path: self.event_physical_path(&e.path),
                cache,
                ..Default::default()
            };
            let mut root = self.physical_comparison.clone();
            if root.path.is_empty() {
                root.path = self.physical_dir.clone();
            }
            let (path, ok) = self
                .comparer
                .rebase_prepared(&mut physical_path, &root, &self.dir);
            if ok {
                e.path = path;
            }
        }
        e
    }

    // Go: watcher.go:1056 callback.eventPhysicalPath
    pub fn event_physical_path(&self, path: &str) -> String {
        if !self.watch_physical_dir.is_empty()
            && !self.watch_dir.is_empty()
            && self.watch_physical_dir != self.watch_dir
            && is_in_directory_or_self(&self.watch_dir, path)
        {
            return rebase_path(path, &self.watch_dir, &self.watch_physical_dir);
        }
        path.to_string()
    }
}

// Go: watcher.go:1086 isInDirectoryOrSelf
pub fn is_in_directory_or_self(dir: &str, path: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    if path == dir {
        return true;
    }
    let Some(rest) = path.strip_prefix(dir) else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if is_path_separator(dir.as_bytes()[dir.len() - 1]) {
        return true;
    }
    is_path_separator(rest.as_bytes()[0])
}

// Go: watcher.go:1108 isDirectChild
/// isDirectChild reports whether path is an immediate child of dir.
/// Both paths must be absolute. Returns false for path == dir.
pub fn is_direct_child(dir: &str, path: &str) -> bool {
    if !path.starts_with(dir) {
        return false;
    }
    let rest = &path.as_bytes()[dir.len()..];
    if rest.is_empty() {
        return false;
    }
    let separator = std::path::MAIN_SEPARATOR as u8;
    if rest[0] != b'/' && rest[0] != separator {
        return false;
    }
    let rest = &rest[1..];
    !rest.is_empty() && !rest.contains(&b'/') && !rest.contains(&separator)
}

impl DirWatch {
    // Go: watcher.go:1123 dirWatch.watch
    pub fn watch(
        &self,
        dir: &str,
        physical_dir: &str,
        recursive: bool,
        fn_: WatchCallback,
        ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> (u64, bool) {
        self.add_callback(dir, physical_dir, recursive, fn_, ignore, "")
    }

    // Go: watcher.go:1127 dirWatch.addCallback (ts#64210)
    // PORT: Go reads `dw.sequence()` when it is set; it is always nil here
    // (see `WatcherStruct`).
    pub fn add_callback(
        &self,
        dir: &str,
        physical_dir: &str,
        recursive: bool,
        fn_: WatchCallback,
        ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
        file: &str,
    ) -> (u64, bool) {
        let mut dw = self.mu.lock().unwrap();
        dw.next_cbid += 1;
        let id = dw.next_cbid;
        let since_seq = self.events.sequence();
        dw.callbacks.push(Callback {
            id,
            dir: dir.to_string(),
            physical_dir: physical_dir.to_string(),
            watch_dir: self.dir.clone(),
            watch_physical_dir: self.physical_dir.clone(),
            recursive,
            fn_,
            ignore,
            since_seq,
            terminal: None,
            delivered: false,
            comparer: self.comparer,
            dir_comparison: self.comparer.prepare(dir),
            physical_comparison: self.comparer.prepare(physical_dir),
            file_comparison: self.comparer.prepare(file),
        });
        (id, true)
    }

    // Go: watcher.go:1145 dirWatch.unwatch
    pub fn unwatch(&self, id: u64) -> bool {
        let mut dw = self.mu.lock().unwrap();
        for i in 0..dw.callbacks.len() {
            if dw.callbacks[i].id == id {
                dw.callbacks.remove(i);
                return dw.callbacks.is_empty();
            }
        }
        false
    }

    // Go: watcher.go:1157 dirWatch.unref
    pub fn unref(&self, w: &WatcherStruct) {
        let empty = {
            let dw = self.mu.lock().unwrap();
            dw.callbacks.is_empty()
        };
        if empty {
            w.remove_dir_watch(self);
        }
    }
}
