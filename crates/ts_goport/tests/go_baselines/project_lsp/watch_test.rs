//! Port of Go `internal/project/watch_test.go`.

use std::cell::RefCell;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use ts_goport::frontend::tspath;
use ts_goport::lsp::lsproto;
use ts_goport::project::{
    PatternsAndIgnored, SeenFiles, WatchedFiles, create_resolution_lookup_glob_mapper,
    get_path_components_for_watching, new_recursive_directory_watcher,
};

fn components(path: &str) -> Vec<String> {
    get_path_components_for_watching(path, "")
}

// Go: watch_test.go:9 TestGetPathComponentsForWatching
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

// Go: watch_test.go:22 TestNilWatchedFilesClone
#[test]
fn nil_watched_files_clone() {
    let result = WatchedFiles::<i32>::clone_(None, 42);
    assert!(
        result.is_none(),
        "clone on a nil `WatchedFiles` should return nil"
    );
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

// Go: watch_test.go:33 TestResolutionLookupWatcherPreservesDirectorySpelling (ts#64159)
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

// Go: watch_test.go:73 TestResolutionLookupWatcherPreservesIncludedDirectorySpelling (ts#64544)
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

// Go: watch_test.go:57 TestResolutionLookupWatcherPreservesNodeModulesSpelling (ts#64544)
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

// Go: watch_test.go:74 TestResolutionLookupWatcherAggregatesUsingHostCaseSensitivity (ts#64544)
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
