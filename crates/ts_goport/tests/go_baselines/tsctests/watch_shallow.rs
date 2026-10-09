//! Port of internal/execute/tsctests/watch_shallow_test.go (ts#64366):
//! `tsc --watch` and `tsc -b --watch` on a project close to the file system
//! root.
//!
//! PORT: the compiler runs in this process with the OS override for the
//! test system, which is for the whole process. So each test runs in a
//! child process of its own (`run_test_in_child`), as in watcher_race.rs.

use std::rc::Rc;

use ts_goport::execute::tsc::Watcher;
use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::context;

use crate::support::child::{command_line_in_process, new_in_process_test_sys, run_test_in_child};
use crate::support::runner::{FileMap, TscInput};
use crate::support::test_sys::TestSys;

/// Go `FileMap{...}` literal.
fn file_map<const N: usize>(entries: [(&str, String); N]) -> FileMap {
    entries
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.into()))
        .collect()
}

// Go: watch_shallow_test.go:16 shallowProjectFiles
/// shallowProjectFiles is a project close to the filesystem root (a common Docker WORKDIR) that imports a file from a
/// sibling directory which is not part of "include", plus a bare import whose failed lookups walk up to /node_modules.
fn shallow_project_files(compiler_options: &str) -> FileMap {
    file_map([
        (
            "/app/tsconfig.json",
            format!(
                r#"{{"compilerOptions":{{{compiler_options}"rootDir":"..","outDir":"out","noLib":true}},"include":["*.ts"]}}"#
            ),
        ),
        (
            "/app/index.ts",
            "import { s } from \"../shared/s\";\n// @ts-ignore\nimport \"missing-package\";\nexport const x = s;"
                .to_string(),
        ),
        ("/shared/s.ts", "export const s = 1;".to_string()),
    ])
}

// Go: watch_shallow_test.go:28 isWatched
/// isWatched reports whether dir has an open watch. The mock keeps closed watches in Dirs, so a nil check is not enough.
fn is_watched(sys: &TestSys, dir: &str) -> bool {
    sys.mock_watch_backend()
        .dirs
        .lock()
        .unwrap()
        .get(dir)
        .is_some_and(|w| !w.is_closed())
}

// Go: watch_shallow_test.go:33 assertShallowProjectWatches
fn assert_shallow_project_watches(sys: &TestSys) {
    assert!(
        is_watched(sys, "/app"),
        "the tsconfig directory /app must be watched"
    );
    assert!(
        is_watched(sys, "/shared"),
        "the directory of the imported program file /shared/s.ts must be watched"
    );
    // Failed lookups of "missing-package" reach /node_modules. They must not turn into a watch on /.
    assert!(!is_watched(sys, "/"), "/ must never be watched");
}

/// Go `sys.mockWatchBackend.SendEvents([]fswatch.Event{{Kind: kind, Path: path}})`.
fn send_event(sys: &TestSys, kind: EventKind, path: &str) {
    sys.mock_watch_backend().send_events(vec![Event {
        kind,
        path: path.to_string(),
    }]);
}

// Go: watch_shallow_test.go:41 editShallowProjectFiles
fn edit_shallow_project_files(sys: &TestSys, w: &mut dyn Watcher) {
    let fs = sys.fs_from_file_map();

    sys.write_file_no_error("/shared/s.ts", "export const s = 2;");
    send_event(sys, EventKind::Update, "/shared/s.ts");
    w.do_cycle();
    let (out, _) = fs.read_file("/app/out/shared/s.js");
    assert!(
        out.contains("s = 2"),
        "editing /shared/s.ts must rebuild, got:\n{out}"
    );

    sys.write_file_no_error(
        "/app/index.ts",
        "import { s } from \"../shared/s\"; export const y = s;",
    );
    send_event(sys, EventKind::Update, "/app/index.ts");
    w.do_cycle();
    let (out, _) = fs.read_file("/app/out/app/index.js");
    assert!(
        out.contains("y = "),
        "editing /app/index.ts must rebuild, got:\n{out}"
    );
}

/// Go `newTestSys(&tscInput{files: files, cwd: "/app"}, false)`.
fn new_shallow_test_sys(files: FileMap) -> Rc<TestSys> {
    new_in_process_test_sys(&TscInput {
        files,
        cwd: "/app".to_string(),
        ..Default::default()
    })
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

// Go: watch_shallow_test.go:60 TestWatchShallowProjectWithImportedFile
/// TestWatchShallowProjectWithImportedFile verifies that tsc --watch rebuilds a project that lives close to the
/// filesystem root, both for its own files and for a file it imports from outside "include".
#[test]
fn test_watch_shallow_project_with_imported_file() {
    run_test_in_child(
        "tsctests::watch_shallow::test_watch_shallow_project_with_imported_file",
        || {
            let sys = new_shallow_test_sys(shallow_project_files(""));
            let result = command_line_in_process(&context::background(), &sys, &args(&["--watch"]));
            let mut w = result.watcher.expect("result.Watcher != nil");

            assert_shallow_project_watches(&sys);
            edit_shallow_project_files(&sys, &mut *w);
        },
    );
}

// Go: watch_shallow_test.go:72 TestBuildWatchShallowProjectWithImportedFile
/// TestBuildWatchShallowProjectWithImportedFile is the tsc -b --watch variant of
/// TestWatchShallowProjectWithImportedFile.
#[test]
fn test_build_watch_shallow_project_with_imported_file() {
    run_test_in_child(
        "tsctests::watch_shallow::test_build_watch_shallow_project_with_imported_file",
        || {
            let sys = new_shallow_test_sys(shallow_project_files(r#""composite":true,"#));
            let (ctx, cancel) = context::with_cancel(&context::background());
            let result = command_line_in_process(&ctx, &sys, &args(&["--build", "--watch"]));
            let mut w = result.watcher.expect("result.Watcher != nil");

            assert_shallow_project_watches(&sys);
            edit_shallow_project_files(&sys, &mut *w);
            cancel();
        },
    );
}

// Go: watch_shallow_test.go:85 shallowRootFileProjectFiles
/// shallowRootFileProjectFiles is a project in /app whose "files" list a root file in the sibling directory /shared.
fn shallow_root_file_project_files(compiler_options: &str) -> FileMap {
    file_map([
        (
            "/app/tsconfig.json",
            format!(
                r#"{{"compilerOptions":{{{compiler_options}"rootDir":"..","outDir":"out","noLib":true}},"files":["index.ts","../shared/root.ts"]}}"#
            ),
        ),
        ("/app/index.ts", "export const x = 1;".to_string()),
        ("/shared/root.ts", "export const r = 1;".to_string()),
    ])
}

// Go: watch_shallow_test.go:95 deleteAndRecreateShallowRootFile
/// deleteAndRecreateShallowRootFile deletes /shared/root.ts and writes it back. While the file is missing it is not
/// part of the program, but it is still a root file, so /shared must stay watched for the rebuild on recreation.
fn delete_and_recreate_shallow_root_file(sys: &TestSys, w: &mut dyn Watcher) {
    let fs = sys.fs_from_file_map();
    assert!(
        is_watched(sys, "/shared"),
        "the directory of the root file /shared/root.ts must be watched"
    );

    sys.remove_no_error("/shared/root.ts");
    send_event(sys, EventKind::Delete, "/shared/root.ts");
    w.do_cycle();
    assert!(
        is_watched(sys, "/shared"),
        "/shared must stay watched while the root file /shared/root.ts is missing"
    );
    assert!(!is_watched(sys, "/"), "/ must never be watched");

    sys.write_file_no_error("/shared/root.ts", "export const r = 2;");
    send_event(sys, EventKind::Update, "/shared/root.ts");
    w.do_cycle();
    let (out, _) = fs.read_file("/app/out/shared/root.js");
    assert!(
        out.contains("r = 2"),
        "recreating /shared/root.ts must rebuild, got:\n{out}"
    );
}

// Go: watch_shallow_test.go:115 TestWatchShallowProjectRecreatedRootFile
/// TestWatchShallowProjectRecreatedRootFile verifies that tsc --watch rebuilds when a root file near the filesystem
/// root is deleted and created again.
#[test]
fn test_watch_shallow_project_recreated_root_file() {
    run_test_in_child(
        "tsctests::watch_shallow::test_watch_shallow_project_recreated_root_file",
        || {
            let sys = new_shallow_test_sys(shallow_root_file_project_files(""));
            let result = command_line_in_process(&context::background(), &sys, &args(&["--watch"]));
            let mut w = result.watcher.expect("result.Watcher != nil");

            delete_and_recreate_shallow_root_file(&sys, &mut *w);
        },
    );
}

// Go: watch_shallow_test.go:126 TestBuildWatchShallowProjectRecreatedRootFile
/// TestBuildWatchShallowProjectRecreatedRootFile is the tsc -b --watch variant of
/// TestWatchShallowProjectRecreatedRootFile.
#[test]
fn test_build_watch_shallow_project_recreated_root_file() {
    run_test_in_child(
        "tsctests::watch_shallow::test_build_watch_shallow_project_recreated_root_file",
        || {
            let sys = new_shallow_test_sys(shallow_root_file_project_files(r#""composite":true,"#));
            let (ctx, cancel) = context::with_cancel(&context::background());
            let result = command_line_in_process(&ctx, &sys, &args(&["--build", "--watch"]));
            let mut w = result.watcher.expect("result.Watcher != nil");

            delete_and_recreate_shallow_root_file(&sys, &mut *w);
            cancel();
        },
    );
}
