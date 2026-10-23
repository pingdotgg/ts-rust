//! Port of Go `internal/project/watch_test.go`.

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use ts_goport::frontend::tspath;
use ts_goport::lsp::lsproto;
use ts_goport::project::{
    PatternsAndIgnored, SeenFiles, WatchedFiles, create_resolution_lookup_glob_mapper,
    file_system_watcher_glob_string, get_path_components_for_watching,
    new_recursive_directory_watcher, new_watched_files_for_paths,
};

fn components(path: &str) -> Vec<String> {
    get_path_components_for_watching(path, "")
}

// Go: watch_test.go:13 TestGetPathComponentsForWatching
#[test]
fn get_path_components_for_watching_test() {
    assert_eq!(components("/project"), ["/", "project"]);
    assert_eq!(components("C:\\project"), ["C:/", "project"]);
    assert_eq!(
        components("//server/share/project/tsconfig.json"),
        ["//server/share", "project", "tsconfig.json"]
    );
    assert_eq!(
        components(r"\\server\share\project\tsconfig.json"),
        ["//server/share", "project", "tsconfig.json"]
    );
    assert_eq!(components("C:\\Users"), ["C:/Users"]);
    assert_eq!(
        components("C:\\Users\\andrew\\project"),
        ["C:/Users/andrew", "project"]
    );
    assert_eq!(components("/home"), ["/home"]);
    assert_eq!(
        components("/home/andrew/project"),
        ["/home/andrew", "project"]
    );
}

// Go: watch_test.go:26 TestNilWatchedFilesClone
#[test]
fn nil_watched_files_clone() {
    let result = WatchedFiles::<i32>::clone_(None, 42);
    assert!(
        result.is_none(),
        "clone on a nil `WatchedFiles` should return nil"
    );
}

// Go: watch_test.go:34 TestWatchedFilesOmitsNetworkPaths (ts#64832)
// PORT: `new_watched_files_for_paths` takes a current directory for its
// path comparisons; the paths here are absolute, so it is "/".
#[test]
fn watched_files_omits_network_paths() {
    let files: Vec<String> = [
        "/workspace/a.ts",
        "/external/b.ts",
        "//server/share/c.ts",
        "/??/UNC/server/share/d.ts",
    ]
    .map(String::from)
    .to_vec();
    for (workspace, expected) in [
        (
            "/workspace",
            vec!["/workspace/a.ts", "file:///external/**/*"],
        ),
        (
            "//server/share",
            vec!["file:///external/**/*", "file:///workspace/**/*"],
        ),
    ] {
        let base = new_watched_files_for_paths(
            "test",
            lsproto::WatchKind::CREATE,
            true,
            workspace,
            "/",
            true,
        );
        let w = WatchedFiles::clone_(Some(&base), files.clone()).expect("clone");
        let watchers = w.watchers();
        let globs: Vec<String> = watchers
            .workspace_watchers
            .iter()
            .chain(&watchers.outside_workspace_watchers)
            .map(file_system_watcher_glob_string)
            .collect();
        assert_eq!(globs, expected, "workspace {workspace}");
    }
}

/// Go `var files collections.SyncMap[tspath.Path, string]` with
/// `files.Store(tspath.ToPath(fileName, "/", useCaseSensitiveFileNames), fileName)`.
fn seen_files(file_names: &[&str], use_case_sensitive_file_names: bool) -> Option<SeenFiles> {
    let mut files = FxHashMap::default();
    for file_name in file_names {
        files.insert(
            tspath::to_path(file_name, "/", use_case_sensitive_file_names),
            file_name.to_string(),
        );
    }
    Some(Rc::new(RefCell::new(files)))
}

/// Go `createResolutionLookupGlobMapper(workspace, lib, project, caseSensitivity)(&files)`.
fn lookup_globs(
    [workspace, lib, project]: [&str; 3],
    file_names: &[&str],
    use_case_sensitive_file_names: bool,
) -> PatternsAndIgnored {
    create_resolution_lookup_glob_mapper(workspace, lib, project, use_case_sensitive_file_names)(
        &seen_files(file_names, use_case_sensitive_file_names),
    )
}

const UPPER_DIRS: [&str; 3] = ["/Workspace", "/Lib", "/Project"];
const LOWER_DIRS: [&str; 3] = ["/workspace", "/lib", "/current"];

// Go: watch_test.go:57 TestResolutionLookupWatcherPreservesDirectorySpelling (ts#64159)
#[test]
fn resolution_lookup_watcher_preserves_directory_spelling() {
    let result = lookup_globs(LOWER_DIRS, &["/External/Dir/file.ts"], false);
    assert_eq!(result.directories_outside_workspace, ["/External/Dir"]);
    let watcher = new_recursive_directory_watcher(
        &result.directories_outside_workspace[0],
        lsproto::WatchKind::CREATE,
        true,
    );
    let base_uri = watcher
        .glob_pattern
        .relative_pattern
        .as_ref()
        .and_then(|pattern| pattern.base_uri.uri.as_ref())
        .expect("a relative pattern with a base URI");
    assert_eq!(base_uri.0, "file:///External/Dir");
}

// Go: watch_test.go:97 TestResolutionLookupWatcherPreservesIncludedDirectorySpelling (ts#64544)
#[test]
fn resolution_lookup_watcher_preserves_included_directory_spelling() {
    let result = lookup_globs(
        UPPER_DIRS,
        &[
            "/Workspace/src/index.ts",
            "/Project/src/index.ts",
            "/Lib/lib.d.ts",
        ],
        false,
    );
    assert_eq!(
        result.patterns_inside_workspace,
        ["/Workspace/**/*", "/Project/**/*", "/Lib/**/*"]
    );
}

// Go: watch_test.go:79 TestResolutionLookupWatcherPreservesNodeModulesSpelling (ts#64544, ts#64159)
#[test]
fn resolution_lookup_watcher_preserves_node_modules_spelling() {
    let result = lookup_globs(
        LOWER_DIRS,
        &["/External/Node_Modules/pkg/index.d.ts"],
        false,
    );
    assert_eq!(
        result.patterns_inside_workspace,
        ["/External/Node_Modules/**/*"]
    );
}

// Go: watch_test.go:124 TestResolutionLookupWatcherAggregatesUsingHostCaseSensitivity (ts#64544)
#[test]
fn resolution_lookup_watcher_aggregates_using_host_case_sensitivity() {
    for (name, use_case_sensitive_file_names) in
        [("case sensitive", true), ("case insensitive", false)]
    {
        let result = lookup_globs(
            UPPER_DIRS,
            &["/External/Lib/src/a.ts", "/external/LIB/test/b.ts"],
            use_case_sensitive_file_names,
        );
        if use_case_sensitive_file_names {
            assert_eq!(
                result.directories_outside_workspace,
                ["/External/Lib/src", "/external/LIB/test"],
                "{name}"
            );
        } else {
            assert_eq!(result.directories_outside_workspace.len(), 1, "{name}");
            let directory = &result.directories_outside_workspace[0];
            assert!(
                directory == "/External/Lib" || directory == "/external/LIB",
                "{name}: {directory}"
            );
        }
    }
}

child_test! {
    // followups74 (R226 reviewer item 2): Go updateRootFilesWatch
    // (configfileregistrybuilder.go:231-256) tests rooted paths
    // (`CaseSensitivity.ContainsPath`). On a case-insensitive file system Go
    // folds case with ToLower, which keeps 'ſ' (EqualFold would match it to
    // 's'), so the workspace "/home/ſ" does not hold the wildcard directory
    // "/home/s" and the tsconfig directory is watched. Go answer from a probe
    // at 3e9236361fa9 (target/continuation-r97-goport/after226/followups74/goprobe).
    fn root_files_watch_tests_rooted_paths_like_go() {
        let (session, _utils) = super::projecttestutil::setup_with_options(
            super::projecttestutil::files(&[
                ("/home/s/tsconfig.json", "{}"),
                ("/home/s/a.ts", "export {};"),
            ]),
            ts_goport::project::SessionOptions {
                current_directory: "/home/\u{17f}".to_string(),
                ..super::projecttestutil::default_session_options()
            },
        );
        super::util::open(&session, "file:///home/s/a.ts", "export {};");
        session.wait_for_background_tasks();
        let snapshot = session.snapshot();
        let entry = snapshot
            .config_file_registry
            .configs
            .get(&super::util::path("/home/s/tsconfig.json"))
            .expect("the config entry")
            .clone();
        let watch = entry
            .borrow()
            .root_files_watch
            .clone()
            .expect("a root files watch");
        assert_eq!(watch.input.patterns_inside_workspace, ["/home/s/**/*"]);
    }
}
