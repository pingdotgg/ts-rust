//! Go: `internal/execute/watchmanager/watchmanager_test.go` (added by
//! tsgo#4658).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use ts_goport::execute::tsc::Writer;
use ts_goport::execute::watchmanager::{
    WatchBackend, WatchDirectoryRequest, WatchManager, new_dir_watch_set, new_watch_manager,
};
use ts_goport::frontend::tspath::ComparePathsOptions;
use ts_goport::fswatch;
use ts_goport::gostd::GoError;

// Go: watchmanager_test.go:13 caseSensitiveOpts
fn case_sensitive_opts() -> ComparePathsOptions {
    ComparePathsOptions {
        use_case_sensitive_file_names: true,
        current_directory: "/repo".to_string(),
    }
}

// Go: watchmanager_test.go:14 caseInsensitiveOpts
fn case_insensitive_opts() -> ComparePathsOptions {
    ComparePathsOptions {
        use_case_sensitive_file_names: false,
        current_directory: "/repo".to_string(),
    }
}

// Go: watchmanager_test.go:20 TestDirWatchSetCoverage
/// TestDirWatchSetCoverage checks the core coverage rules: a recursive watch
/// covers itself and all descendants, while a non-recursive watch covers only
/// itself. Ancestors and unrelated paths are never covered.
#[test]
fn test_dir_watch_set_coverage() {
    let mut set = new_dir_watch_set(case_sensitive_opts());
    set.set("/repo/src", true); // recursive
    set.set("/repo/config", false); // non-recursive
    set.set("/repo/node_modules/a", false); // non-recursive

    let tests: [(&str, bool); 9] = [
        ("/repo/src", true),             // exact recursive
        ("/repo/src/nested", true),      // descendant of recursive
        ("/repo/src/nested/deep", true), // deep descendant of recursive
        ("/repo/config", true),          // exact non-recursive
        ("/repo/config/nested", false),  // descendant of non-recursive: NOT covered
        ("/repo/node_modules/a", true),  // exact non-recursive
        ("/repo/node_modules/b", false), // sibling, absent
        ("/repo", false),                // ancestor of watched dirs: NOT covered
        ("/other", false),               // unrelated
    ];
    for (dir, want) in tests {
        assert_eq!(set.covered(dir), want, "Covered({dir:?})");
    }
}

// Go: watchmanager_test.go:49 TestDirWatchSetCaseSensitive
/// TestDirWatchSetCaseSensitive verifies that on a case-sensitive filesystem a
/// differently-cased directory is a distinct, uncovered directory.
#[test]
fn test_dir_watch_set_case_sensitive() {
    let mut set = new_dir_watch_set(case_sensitive_opts());
    set.set("/repo/node_modules/a", false);
    set.set("/repo/Src", true);

    assert!(set.covered("/repo/node_modules/a"));
    assert!(
        !set.covered("/repo/node_modules/A"),
        "case-sensitive FS must not cover differently-cased dir"
    );
    assert!(
        set.covered("/repo/Src/nested"),
        "recursive descendant with matching case is covered"
    );
    assert!(
        !set.covered("/repo/src/nested"),
        "case-sensitive FS must not cover differently-cased descendant"
    );
}

// Go: watchmanager_test.go:64 TestDirWatchSetCaseInsensitive
/// TestDirWatchSetCaseInsensitive verifies that on a case-insensitive filesystem
/// coverage ignores casing for both exact matches and recursive containment.
#[test]
fn test_dir_watch_set_case_insensitive() {
    let mut set = new_dir_watch_set(case_insensitive_opts());
    set.set("/repo/node_modules/a", false);
    set.set("/repo/Src", true);

    assert!(
        set.covered("/repo/node_modules/A"),
        "exact match should be case-insensitive"
    );
    assert!(
        set.covered("/REPO/NODE_MODULES/a"),
        "exact match should be case-insensitive across components"
    );
    assert!(
        set.covered("/repo/src/nested/deep"),
        "recursive containment should be case-insensitive"
    );
}

// Go: watchmanager_test.go:80 TestDirWatchSetCanonicalDedup
/// TestDirWatchSetCanonicalDedup verifies that on a case-insensitive filesystem
/// directories that differ only by casing collapse to a single canonical entry,
/// while a case-sensitive filesystem keeps them distinct.
#[test]
fn test_dir_watch_set_canonical_dedup() {
    let mut insensitive = new_dir_watch_set(case_insensitive_opts());
    insensitive.set("/repo/Node_Modules/PkgName", false);
    insensitive.set("/repo/node_modules/pkgname", false); // same dir, different casing

    let dirs = insensitive.dirs();
    assert_eq!(
        dirs.len(),
        1,
        "differently-cased dirs must collapse to one entry"
    );
    let original = dirs.contains_key("/repo/Node_Modules/PkgName");
    assert!(
        original,
        "Dirs must retain the original spelling used for registration"
    );

    let mut sensitive = new_dir_watch_set(case_sensitive_opts());
    sensitive.set("/repo/Node_Modules/PkgName", false);
    sensitive.set("/repo/node_modules/pkgname", false); // distinct dirs when case-sensitive
    assert_eq!(
        sensitive.dirs().len(),
        2,
        "case-sensitive FS keeps differently-cased dirs distinct"
    );
}

// Go: watchmanager_test.go:100 TestDirWatchSetUpgradeToRecursive
/// TestDirWatchSetUpgradeToRecursive verifies that upgrading a directory from
/// non-recursive to recursive begins covering its descendants.
#[test]
fn test_dir_watch_set_upgrade_to_recursive() {
    let mut set = new_dir_watch_set(case_sensitive_opts());
    set.set("/repo/src", false);
    assert!(set.covered("/repo/src"));
    assert!(
        !set.covered("/repo/src/nested"),
        "descendant not covered while non-recursive"
    );

    set.set("/repo/src", true);
    assert!(
        set.covered("/repo/src/nested"),
        "descendant covered after upgrade to recursive"
    );
    assert!(set.dirs().get("/repo/src").copied().unwrap_or(false));
}

// Go: watchmanager_test.go:115 TestDirWatchSetNeverDowngrades
/// TestDirWatchSetNeverDowngrades verifies a recursive watch is not downgraded by
/// a subsequent non-recursive Set of the same directory.
#[test]
fn test_dir_watch_set_never_downgrades() {
    let mut set = new_dir_watch_set(case_sensitive_opts());
    set.set("/repo/src", true);
    set.set("/repo/src", false);

    assert!(set.dirs().get("/repo/src").copied().unwrap_or(false));
    assert!(
        set.covered("/repo/src/nested"),
        "recursive coverage retained after non-recursive Set"
    );
}

// Go: watchmanager_test.go:128 TestDirWatchSetDirs
/// TestDirWatchSetDirs verifies the emitted map reflects every added directory
/// with the expected recursive flags.
#[test]
fn test_dir_watch_set_dirs() {
    let mut set = new_dir_watch_set(case_sensitive_opts());
    set.set("/repo/a", false);
    set.set("/repo/b", true);
    set.set("/repo/a", false); // duplicate non-recursive add is idempotent

    let dirs = set.dirs();
    assert_eq!(dirs.len(), 2);
    assert!(!dirs.get("/repo/a").copied().unwrap_or(false));
    assert!(dirs.get("/repo/b").copied().unwrap_or(false));
}

/// A watch manager whose `dirExists` answers from `existing` (Go:
/// `NewWatchManager(io.Discard, func(dir string) bool { return existing[dir] })`).
fn watch_manager_with_existing_dirs(existing: &'static [&'static str]) -> WatchManager {
    let discard: Writer = Rc::new(RefCell::new(std::io::sink()));
    new_watch_manager(
        discard,
        Box::new(move |dir: &str| existing.contains(&dir)),
        true,
    )
}

/// Go `map[string]bool{...}` literal.
fn dir_map<const N: usize>(entries: [(&str, bool); N]) -> FxHashMap<String, bool> {
    entries
        .into_iter()
        .map(|(dir, recursive)| (dir.to_string(), recursive))
        .collect()
}

// Go: watchmanager_test.go:144 TestResolveDesiredDirsShallowProject (ts#64366)
/// TestResolveDesiredDirsShallowProject verifies that a directory that exists and was asked for is watched at any
/// depth. A project close to the filesystem root (/app, /srv/app, a Docker WORKDIR) must not be silently ignored.
#[test]
fn test_resolve_desired_dirs_shallow_project() {
    let wm = watch_manager_with_existing_dirs(&[
        "/",
        "/app",
        "/app/src",
        "/srv",
        "/srv/app",
        "/home",
        "/home/user",
        "/home/user/project",
    ]);

    let resolved = wm.resolve_desired_dirs(&dir_map([
        ("/app", true),
        ("/app/src", false),
        ("/srv/app", true),
        ("/home/user/project", true),
    ]));

    assert_eq!(
        resolved,
        dir_map([
            ("/app", true),
            ("/app/src", false),
            ("/srv/app", true),
            ("/home/user/project", true),
        ])
    );
}

// Go: watchmanager_test.go:170 TestResolveDesiredDirsAncestorFallback (ts#64366)
/// TestResolveDesiredDirsAncestorFallback verifies that the depth check still guards the fallback to an ancestor,
/// so a missing directory never turns into a watch on something too generic like /, /home or /home/user.
#[test]
fn test_resolve_desired_dirs_ancestor_fallback() {
    let wm = watch_manager_with_existing_dirs(&[
        "/",
        "/app",
        "/home",
        "/home/user",
        "/repo",
        "/repo/a",
        "/repo/a/b",
        "/repo/a/b/c",
    ]);

    let resolved = wm.resolve_desired_dirs(&dir_map([
        ("/app/missing", true),             // ancestor /app is too shallow
        ("/home/user/missing", true),       // ancestor /home/user is too shallow
        ("/repo/a/b/c/missing/deep", true), // ancestor /repo/a/b/c is deep enough, and is never recursive
        ("/nothing/exists/anywhere", true), // no existing ancestor except /
    ]));

    assert_eq!(resolved, dir_map([("/repo/a/b/c", false)]));
}

// Go: watchmanager_test.go:191 TestResolveDesiredDirsSkipsNonDiskPaths (ts#64366)
/// TestResolveDesiredDirsSkipsNonDiskPaths verifies that a directory that is not on disk, such as the embedded libs
/// (bundled:///libs), is never watched, even though the wrapped FS reports that it exists.
#[test]
fn test_resolve_desired_dirs_skips_non_disk_paths() {
    let discard: Writer = Rc::new(RefCell::new(std::io::sink()));
    let wm = new_watch_manager(discard, Box::new(|_: &str| true), true);

    let resolved = wm.resolve_desired_dirs(&dir_map([("bundled:///libs", false), ("/app", true)]));

    assert_eq!(resolved, dir_map([("/app", true)]));
}

// Go: watchmanager_test.go:204 TestResolveDesiredDirsDeduplicatesCaseInsensitiveAncestors (ts#64159)
#[test]
fn test_resolve_desired_dirs_deduplicates_case_insensitive_ancestors() {
    let discard: Writer = Rc::new(RefCell::new(std::io::sink()));
    let manager = new_watch_manager(
        discard,
        Box::new(|dir: &str| dir.eq_ignore_ascii_case("/home/repo/project/src")),
        false,
    );
    let resolved = manager.resolve_desired_dirs(&dir_map([
        ("/home/Repo/Project/Src/missing/a", false),
        ("/home/repo/project/src/missing/b", true),
    ]));

    assert_eq!(resolved.len(), 1);
    for (dir, recursive) in &resolved {
        assert!(dir.eq_ignore_ascii_case("/home/repo/project/src"));
        assert!(!recursive);
    }
}

// Go: watchmanager_test.go:222 recordingWatchBackend
#[derive(Default)]
struct RecordingWatchBackend {
    requests: RefCell<Vec<WatchDirectoryRequest>>,
}

/// Go `io.NopCloser(strings.NewReader(""))`.
struct NopWatch;

impl fswatch::Watch for NopWatch {
    fn close(&self) -> Result<(), GoError> {
        Ok(())
    }
    fn unexported(&self) {}
}

impl WatchBackend for RecordingWatchBackend {
    fn watch_directory(
        &self,
        dir: &str,
        fn_: fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> Result<Box<dyn fswatch::Watch>, GoError> {
        let mut closers = self.watch_directories(vec![WatchDirectoryRequest {
            dir: dir.to_string(),
            callback: fn_,
            recursive,
            ignore,
        }])?;
        Ok(closers.remove(0))
    }

    // Go: watchmanager_test.go:226 recordingWatchBackend.WatchDirectories
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Box<dyn fswatch::Watch>>, GoError> {
        let closers = requests
            .iter()
            .map(|_| Box::new(NopWatch) as Box<dyn fswatch::Watch>)
            .collect();
        self.requests.borrow_mut().extend(requests);
        Ok(closers)
    }
}

// Go: watchmanager_test.go:235 TestReconcileWatchesIgnoresCaseOnlySpellingChanges (ts#64159)
#[test]
fn test_reconcile_watches_ignores_case_only_spelling_changes() {
    let discard: Writer = Rc::new(RefCell::new(std::io::sink()));
    let mut manager = new_watch_manager(discard, Box::new(|_: &str| true), false);
    let backend = Rc::new(RecordingWatchBackend::default());
    manager.set_backend(backend.clone());

    manager
        .reconcile_watches(&dir_map([("/Repo", false)]))
        .unwrap();
    manager
        .reconcile_watches(&dir_map([("/repo", false)]))
        .unwrap();

    let requests = backend.requests.borrow();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].dir, "/Repo");
}
