//! Go: internal/execute/watchmanager/watchbackend.go (the fswatch backend
//! seam and the watch path filters).

use crate::execute::watchmanager::prelude::*;

use std::sync::Arc;

use crate::frontend::stringutil_ls;
use crate::frontend::tspath;
use crate::fswatch;

// Go: watchbackend.go:12 WatchBackend
/// WatchBackend abstracts fswatch.Watcher for testing
///
/// PORT: Go returns an `io.Closer`; the port returns the fswatch
/// `Box<dyn fswatch::Watch>` (its `close` is `Close`). Go `ignore` is a
/// `func(string) bool` that can be nil; it runs on the fswatch debouncer
/// thread, so it is `Send + Sync`.
pub trait WatchBackend {
    fn watch_directory(
        &self,
        dir: &str,
        fn_: fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> Result<Box<dyn fswatch::Watch>, GoError>;
    /// PORT: Go takes a `[]WatchDirectoryRequest` slice; the port takes the
    /// requests by value, because a backend keeps their callbacks.
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Box<dyn fswatch::Watch>>, GoError>;
}

// Go: watchbackend.go:23 WatchDirectoryRequest
pub struct WatchDirectoryRequest {
    pub dir: String,
    pub callback: fswatch::WatchCallback,
    pub recursive: bool,
    pub ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
}

// Go: watchbackend.go:32 CommandLineTestingWithWatchBackend
/// CommandLineTestingWithWatchBackend is an optional extension of
/// [CommandLineTesting] that supplies a [WatchBackend] for test mode
pub trait CommandLineTestingWithWatchBackend {
    fn watch_backend(&self) -> Rc<dyn WatchBackend>;
}

// Go: watchbackend.go:36 FSWatchBackend
pub struct FsWatchBackend {
    pub inner: Arc<dyn fswatch::Watcher>,
}

impl WatchBackend for FsWatchBackend {
    // Go: watchbackend.go:32 FSWatchBackend.WatchDirectory (at 673a5f17d713; removed by
    // ts#64159)
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

    // Go: watchbackend.go:38 FSWatchBackend.WatchDirectories
    /// PORT: an fswatch request borrows its options, so all option lists are
    /// made before the requests. Go converts each `fswatch.Watch` to an
    /// `io.Closer`; the port returns the watches.
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Box<dyn fswatch::Watch>>, GoError> {
        let opts: Vec<Vec<Box<dyn fswatch::WatchOption>>> = requests
            .iter()
            .map(|request| {
                let mut opts: Vec<Box<dyn fswatch::WatchOption>> = Vec::new();
                if request.recursive {
                    opts.push(fswatch::with_recursive());
                }
                if let Some(ignore) = &request.ignore {
                    opts.push(fswatch::with_ignore(ignore.clone()));
                }
                opts
            })
            .collect();
        let fswatch_requests: Vec<fswatch::WatchDirectoryRequest<'_>> = requests
            .into_iter()
            .zip(&opts)
            .map(|(request, options)| fswatch::WatchDirectoryRequest {
                dir: request.dir,
                callback: request.callback,
                options,
            })
            .collect();
        self.inner.watch_directories(&fswatch_requests)
    }
}

// Go: watchbackend.go:76 ShouldIgnoreWatchPath
pub fn should_ignore_watch_path(path: &str) -> bool {
    let p = tspath::normalize_slashes(path);
    p.ends_with("/.git")
        || p.contains("/.git/")
        || p.contains("/node_modules/.")
        || p.contains("/.#")
}

// Go: watchbackend.go:84 CanWatchDirectory
pub fn can_watch_directory(dir: &str) -> bool {
    let components = tspath::get_path_components(dir, "");
    let length = components.len() as i32;
    if length <= 2 {
        return false;
    }
    let root_length = perceived_os_root_length_for_watching(&components);
    length > root_length + 1
}

// Go: watchbackend.go:94 PerceivedOsRootLengthForWatching
// PORT: Go `strings.EqualFold` is `stringutil_ls::equate_string_case_insensitive`
// (Go `stringutil.EquateStringCaseInsensitive` is `strings.EqualFold`).
pub fn perceived_os_root_length_for_watching(components: &[String]) -> i32 {
    let length = components.len() as i32;
    if length <= 1 {
        return 1;
    }
    let root = components[0].as_bytes();
    let mut index_after_os_root: i32 = 1;
    let mut is_dos_style =
        root.len() >= 2 && tspath::is_volume_character(root[0]) && root[1] == b':';

    if components[0] != "/" && !is_dos_style && components.len() > 1 {
        let c1 = components[1].as_bytes();
        if c1.len() >= 2 && tspath::is_volume_character(c1[0]) && components[1].ends_with('$') {
            if length == 2 {
                return 2;
            }
            index_after_os_root = 2;
            is_dos_style = true;
        }
    }

    if is_dos_style
        && (index_after_os_root >= length
            || !stringutil_ls::equate_string_case_insensitive(
                &components[index_after_os_root as usize],
                "users",
            ))
    {
        return index_after_os_root;
    }

    if index_after_os_root < length
        && stringutil_ls::equate_string_case_insensitive(
            &components[index_after_os_root as usize],
            "workspaces",
        )
    {
        return index_after_os_root + 1;
    }

    index_after_os_root + 2
}
