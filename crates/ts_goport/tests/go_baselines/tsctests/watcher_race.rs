//! Go: internal/execute/tsctests/watcher_race_test.go (watcher concurrency
//! tests).
//!
//! PORT: Go calls `DoCycle` from many goroutines at once and relies on
//! `go test -race`. The `execute::watcher::Watcher` holds `Rc`s and is not
//! `Send`, so Rust already rejects concurrent `DoCycle` calls at compile
//! time. Each test keeps Go's operations and counts. The file writes and
//! removes of the Go goroutines run on threads through the `Send` `MapFs`
//! handle (Go `sys.fsFromFileMap()` is the iovfs view over it, which is
//! `MapFs::fs`). All `DoCycle` calls of the Go goroutines run on the test
//! thread while those writer threads run.
//!
//! PORT: the compiler runs in this process with the OS override for the
//! test system, which is for the whole process. So each test runs in a
//! child process of its own (`run_test_in_child`).
//! `watcher_starts_from_existing_build_info` runs two commands, so it runs
//! each in a command child, as the tsc runner does (`command_line_in_child`).

use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use ts_goport::execute;
use ts_goport::execute::tsc::{CommandLineResult, ExitStatus, Watcher};
use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::{Context, context};

use crate::support::child::{
    ChildRun, command_line_in_process, new_in_process_test_sys, run_command_in_child,
    run_test_in_child,
};
use crate::support::runner::{FileMap, TscInput};
use crate::support::test_sys::{TestSys, new_test_sys};
use crate::support::vfstest::MapFile;

/// The stack of the thread that runs the build in
/// `build_watch_stops_when_context_is_cancelled` (as child.rs).
const STACK_SIZE: usize = 1 << 30;

/// Go `FileMap{...}` literal.
fn file_map<const N: usize>(entries: [(&str, MapFile); N]) -> FileMap {
    entries
        .into_iter()
        .map(|(path, file)| (path.to_string(), file))
        .collect()
}

/// Go `execute.CommandLine(ctx, sys, args, sys)`, run in this process.
fn command_line(ctx: &Context, sys: &Rc<TestSys>, args: &[&str]) -> CommandLineResult {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    command_line_in_process(ctx, sys, &args)
}

/// Go `execute.CommandLine(context.Background(), sys, args, sys)` in a
/// command child of its own (child.rs), for a runner `sys`. A child with no
/// response (a panic) or with unported code fails the test.
fn command_line_in_child(sys: &TestSys, args: &[&str]) -> ChildRun {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let result = run_command_in_child(sys, &args)
        .unwrap_or_else(|err| panic!("tsgo {}: {err}", args.join(" ")));
    assert!(
        result.unported.is_none(),
        "tsgo {}: unported {:?}",
        args.join(" "),
        result.unported
    );
    result
}

/// PORT: `n` `DoCycle` calls of Go goroutines, run on the test thread (the
/// watcher is not `Send`).
fn do_cycles(w: &mut dyn Watcher, n: usize) {
    for _ in 0..n {
        w.do_cycle();
    }
}

// Go: watcher_race_test.go:18 createTestWatcher
/// createTestWatcher sets up a minimal project with a tsconfig and
/// returns a Watcher ready for concurrent testing, plus the TestSys
/// for file manipulation.
///
/// PORT: Go also asserts `result.Watcher.(*execute.Watcher)`. The
/// `tsc::Watcher` trait object has no downcast, and the tests only call
/// `do_cycle`, so this returns the trait object.
fn create_test_watcher() -> (Box<dyn Watcher>, Rc<TestSys>) {
    let input = TscInput {
        files: file_map([
            (
                "/home/src/workspaces/project/a.ts",
                "const a: number = 1;".into(),
            ),
            (
                "/home/src/workspaces/project/b.ts",
                r#"import { a } from "./a"; export const b = a;"#.into(),
            ),
            ("/home/src/workspaces/project/tsconfig.json", "{}".into()),
        ]),
        command_line_args: vec!["--watch".to_string()],
        ..Default::default()
    };
    let sys = new_in_process_test_sys(&input);
    let result = command_line(&context::background(), &sys, &["--watch"]);
    let w = result
        .watcher
        .expect("expected Watcher to be non-nil in watch mode");
    (w, sys)
}

// Go: watcher_race_test.go:44 TestWatcherConcurrentDoCycle
/// TestWatcherConcurrentDoCycle calls DoCycle from multiple goroutines
/// while modifying source files, exposing data races on Watcher fields
/// such as configModified, program, config, and the underlying
/// FileWatcher state. Run with -race to detect.
#[test]
fn watcher_concurrent_do_cycle() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_concurrent_do_cycle",
        || {
            let (mut w, sys) = create_test_watcher();
            let map_fs = sys.map_fs();

            thread::scope(|s| {
                for i in 0..8 {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..10 {
                            let _ = fs.write_file(
                                "/home/src/workspaces/project/a.ts",
                                &format!("const a: number = {};", i * 10 + j),
                            );
                        }
                    });
                }
                // The DoCycle calls of the 8 goroutines above.
                do_cycles(w.as_mut(), 8 * 10);
            });
        },
    );
}

// Go: watcher_race_test.go:70 TestWatcherDoCycleWithConcurrentStateReads
/// TestWatcherDoCycleWithConcurrentStateReads calls DoCycle from
/// multiple goroutines, some modifying files and some not, to test
/// concurrent access to all Watcher and FileWatcher state.
#[test]
fn watcher_do_cycle_with_concurrent_state_reads() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_do_cycle_with_concurrent_state_reads",
        || {
            let (mut w, sys) = create_test_watcher();
            let map_fs = sys.map_fs();

            thread::scope(|s| {
                // DoCycle goroutines
                for i in 0..4 {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..15 {
                            let _ = fs.write_file(
                                "/home/src/workspaces/project/a.ts",
                                &format!("const a: number = {};", i * 15 + j),
                            );
                        }
                    });
                }
                // The DoCycle calls of the 4 goroutines above, then of the 8 state
                // reader goroutines (50 rounds of 4 calls each).
                do_cycles(w.as_mut(), 4 * 15 + 8 * 50 * 4);
            });
        },
    );
}

// Go: watcher_race_test.go:109 TestWatcherConcurrentFileChangesAndDoCycle
/// TestWatcherConcurrentFileChangesAndDoCycle creates, modifies, and
/// deletes files from multiple goroutines while DoCycle runs, testing
/// races between FS mutations and watch state updates.
#[test]
fn watcher_concurrent_file_changes_and_do_cycle() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_concurrent_file_changes_and_do_cycle",
        || {
            let (mut w, sys) = create_test_watcher();
            let map_fs = sys.map_fs();

            thread::scope(|s| {
                // File creators
                for i in 0..4 {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..20 {
                            let path = format!("/home/src/workspaces/project/gen_{i}_{j}.ts");
                            let _ = fs.write_file(&path, &format!("export const x{i}_{j} = {j};"));
                        }
                    });
                }

                // File deleters
                {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..20 {
                            let _ =
                                fs.remove(&format!("/home/src/workspaces/project/gen_0_{j}.ts"));
                        }
                    });
                }

                // DoCycle callers
                do_cycles(w.as_mut(), 4 * 10);
            });
        },
    );
}

// Go: watcher_race_test.go:152 TestWatcherRapidConfigChanges
/// TestWatcherRapidConfigChanges modifies tsconfig.json rapidly from
/// multiple goroutines while DoCycle runs, testing races on
/// config-related fields (configModified, configHasErrors,
/// configFilePaths, config, extendedConfigCache).
#[test]
fn watcher_rapid_config_changes() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_rapid_config_changes",
        || {
            let (mut w, sys) = create_test_watcher();
            let map_fs = sys.map_fs();

            const CONFIGS: [&str; 4] = [
                "{}",
                r#"{"compilerOptions": {"strict": true}}"#,
                r#"{"compilerOptions": {"target": "ES2020"}}"#,
                r#"{"compilerOptions": {"noEmit": true}}"#,
            ];

            thread::scope(|s| {
                // Config modifiers + DoCycle
                for i in 0..3 {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..10 {
                            let _ = fs.write_file(
                                "/home/src/workspaces/project/tsconfig.json",
                                CONFIGS[(i + j) % CONFIGS.len()],
                            );
                        }
                    });
                }

                // Concurrent source file modifications
                for i in 0..2 {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..15 {
                            let _ = fs.write_file(
                                "/home/src/workspaces/project/a.ts",
                                &format!("const a: number = {};", i * 15 + j),
                            );
                        }
                    });
                }

                // The DoCycle calls of the config and source goroutines above, then
                // of the 4 state reader goroutines (30 rounds of 2 calls each).
                do_cycles(w.as_mut(), 3 * 10 + 2 * 15 + 4 * 30 * 2);
            });
        },
    );
}

// Go: watcher_race_test.go:211 TestWatcherConcurrentDoCycleNoChanges
/// TestWatcherConcurrentDoCycleNoChanges calls DoCycle from many
/// goroutines when no files have changed, testing the early-return
/// path where WatchState is read and HasChanges is called.
#[test]
fn watcher_concurrent_do_cycle_no_changes() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_concurrent_do_cycle_no_changes",
        || {
            let (mut w, _sys) = create_test_watcher();

            // The DoCycle calls of the 16 goroutines.
            do_cycles(w.as_mut(), 16 * 50);
        },
    );
}

// Go: watcher_race_test.go:231 TestWatcherAlternatingModifyAndDoCycle
/// TestWatcherAlternatingModifyAndDoCycle alternates between modifying
/// a file and calling DoCycle from different goroutines, creating a
/// realistic scenario where the file watcher detects changes mid-cycle.
#[test]
fn watcher_alternating_modify_and_do_cycle() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_alternating_modify_and_do_cycle",
        || {
            let (mut w, sys) = create_test_watcher();
            let map_fs = sys.map_fs();

            thread::scope(|s| {
                // Writer goroutine: continuously modifies files
                {
                    let map_fs = map_fs.clone();
                    s.spawn(move || {
                        let fs = map_fs.fs();
                        for j in 0..100 {
                            let _ = fs.write_file(
                                "/home/src/workspaces/project/a.ts",
                                &format!("const a: number = {j};"),
                            );
                        }
                    });
                }

                // The DoCycle calls of the 4 DoCycle goroutines, then of the 4 state
                // reader goroutines.
                do_cycles(w.as_mut(), 4 * 25 + 4 * 100);
            });
        },
    );
}

// Go: watcher_race_test.go:268 TestBuildWatchStopsWhenContextIsCancelled
///
/// PORT: Go builds `sys` on the test goroutine and runs CommandLine on
/// another goroutine. `TestSys` is not `Send`, so the thread builds the same
/// `TestSys` from the input. The result's watcher is not `Send` either, so
/// the thread sends the status and whether the watcher is set. A panic in
/// that thread ends the channel; Go would crash the test binary instead.
#[test]
fn build_watch_stops_when_context_is_cancelled() {
    run_test_in_child(
        "tsctests::watcher_race::build_watch_stops_when_context_is_cancelled",
        || {
            let input = TscInput {
                files: file_map([
                    (
                        "/home/src/workspaces/project/tsconfig.json",
                        r#"{"compilerOptions":{"composite":true},"files":["index.ts"]}"#.into(),
                    ),
                    (
                        "/home/src/workspaces/project/index.ts",
                        "export const x = 1;".into(),
                    ),
                ]),
                ..Default::default()
            };
            let (ctx, cancel) = context::with_cancel(&context::background());
            cancel();

            let (result_tx, result_rx) = mpsc::sync_channel::<(ExitStatus, bool)>(1);
            // ts#64457 removed --watchInterval from these arguments.
            const ARGS: [&str; 2] = ["--build", "--watch"];
            thread::Builder::new()
                .stack_size(STACK_SIZE)
                .spawn(move || {
                    let sys = new_in_process_test_sys(&input);
                    let result = command_line(&ctx, &sys, &ARGS);
                    let _ = result_tx.send((result.status, result.watcher.is_some()));
                })
                .expect("spawn the build thread");

            match result_rx.recv_timeout(Duration::from_secs(2)) {
                Ok((status, has_watcher)) => {
                    assert_eq!(status, ExitStatus::Success);
                    assert!(has_watcher);
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("build watch did not stop after context cancellation")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("build watch panicked before it returned a result")
                }
            }
        },
    );
}

/// Go `result.Watcher.(*execute.Watcher)`.
fn as_execute_watcher(w: &dyn Watcher) -> &execute::watcher::Watcher {
    w.as_any()
        .downcast_ref::<execute::watcher::Watcher>()
        .expect("the watcher is an *execute.Watcher")
}

/// Go `w.FastPathBuilds(), w.FullBuilds()`.
fn counts(w: &dyn Watcher) -> (i32, i32) {
    let w = as_execute_watcher(w);
    (w.fast_path_builds(), w.full_builds())
}

// Go: watcher_race_test.go:296 TestWatcherStartsFromExistingBuildInfo
///
/// PORT: Go runs both commands in the test process. Here the plain compile
/// installs one program for its process (`core::set_prog`), and watch mode
/// registers program versions, which such a process refuses. So each
/// command runs in a command child of its own, as in the tsc runner
/// (`command_line_in_child`). `sys` keeps the state between them, so the
/// watcher starts with the build info that the first command wrote. Go
/// recovers a panic of the watch start and fails the test; here a child
/// that panics fails it.
#[test]
fn watcher_starts_from_existing_build_info() {
    let input = TscInput {
        files: file_map([
            (
                "/home/src/workspaces/project/index.ts",
                "export const x: number = 1;".into(),
            ),
            (
                "/home/src/workspaces/project/tsconfig.json",
                r#"{"compilerOptions":{"composite":true},"files":["index.ts"]}"#.into(),
            ),
        ]),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);

    let result = command_line_in_child(&sys, &["-p", "tsconfig.json", "--pretty", "false"]);
    assert_eq!(result.status, ExitStatus::Success);
    assert!(
        sys.fs_from_file_map()
            .file_exists("/home/src/workspaces/project/tsconfig.tsbuildinfo")
    );

    sys.clear_output();
    let result = command_line_in_child(&sys, &["--watch", "--noEmit", "--pretty", "false"]);
    assert_eq!(result.status, ExitStatus::Success);
    assert!(result.watcher.is_some());
}

// Go: watcher_race_test.go:321 TestWatcherRebuildsWhenJsxImportSourcePragmaChanges
#[test]
fn watcher_rebuilds_when_jsx_import_source_pragma_changes() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_rebuilds_when_jsx_import_source_pragma_changes",
        || {
            let input = TscInput {
                files: file_map([
                    (
                        "/home/src/workspaces/project/index.tsx",
                        "/** @jsxImportSource foo */\nexport const x = <div />;".into(),
                    ),
                    (
                        "/home/src/workspaces/project/tsconfig.json",
                        "{\n\t\t\t\t\"compilerOptions\":{\"jsx\":\"react-jsx\",\"module\":\"esnext\",\"moduleResolution\":\"bundler\",\"noEmit\":true},\n\t\t\t\t\"files\":[\"index.tsx\"]\n\t\t\t}".into(),
                    ),
                ]),
                command_line_args: vec!["--watch".to_string()],
                ..Default::default()
            };
            let sys = new_in_process_test_sys(&input);
            let result = command_line(
                &context::background(),
                &sys,
                &["--watch", "--pretty", "false"],
            );
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            // Go: w := result.Watcher.(*execute.Watcher)
            as_execute_watcher(w.as_ref());

            sys.set_output_bytes(Vec::new());
            let _ = sys.fs_from_file_map().write_file(
                "/home/src/workspaces/project/index.tsx",
                "/** @jsxImportSource bar */\nexport const x = <div />;",
            );
            sys.mock_watch_backend().send_events(vec![Event {
                kind: EventKind::Update,
                path: "/home/src/workspaces/project/index.tsx".to_string(),
            }]);
            w.do_cycle();

            let out = sys.output_text();
            assert!(
                out.contains("bar/jsx-runtime"),
                "expected updated JSX runtime diagnostic, got: {out}"
            );
            assert!(
                !out.contains("foo/jsx-runtime"),
                "expected stale JSX runtime diagnostic to be gone, got: {out}"
            );
        },
    );
}

// Go: watcher_race_test.go:360 TestWatcherUpdateProgramFastPath
/// TestWatcherUpdateProgramFastPath verifies that the UpdateProgram optimization
/// produces correct compilation results for body-only edits (fast path) and
/// correctly falls back to full NewProgram when the set of imported modules
/// changes. The build path taken is asserted via FastPathBuilds/FullBuilds so a
/// regression that stops using (or stops falling back from) the fast path is
/// caught, not just a diagnostics difference.
#[test]
fn watcher_update_program_fast_path() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_update_program_fast_path",
        || {
            let input = TscInput {
                files: file_map([
                    (
                        "/home/src/workspaces/project/a.ts",
                        "export const a: number = 1;".into(),
                    ),
                    (
                        "/home/src/workspaces/project/b.ts",
                        r#"import { a } from "./a"; export const b = a;"#.into(),
                    ),
                    (
                        "/home/src/workspaces/project/c.ts",
                        "export const c: number = 10;".into(),
                    ),
                    ("/home/src/workspaces/project/tsconfig.json", "{}".into()),
                ]),
                command_line_args: vec!["--watch".to_string()],
                ..Default::default()
            };
            let sys = new_in_process_test_sys(&input);
            let result = command_line(&context::background(), &sys, &["--watch"]);
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            // Helper to write a file, send the event, cycle, and return output
            let edit_and_cycle = |w: &mut dyn Watcher, path: &str, content: &str| {
                sys.set_output_bytes(Vec::new());
                let _ = sys.fs_from_file_map().write_file(path, content);
                sys.mock_watch_backend().send_events(vec![Event {
                    kind: EventKind::Update,
                    path: path.to_string(),
                }]);
                w.do_cycle();
                sys.output_text()
            };

            // Body-only edit — should use UpdateProgram fast path, no errors
            let (fast, full) = counts(w.as_ref());
            let out = edit_and_cycle(
                w.as_mut(),
                "/home/src/workspaces/project/a.ts",
                "export const a: number = 2;",
            );
            assert!(
                out.contains("Found 0 errors"),
                "expected 0 errors after body edit, got: {out}"
            );
            assert_eq!(
                counts(w.as_ref()).0,
                fast + 1,
                "body-only edit should take the UpdateProgram fast path"
            );
            assert_eq!(
                counts(w.as_ref()).1,
                full,
                "body-only edit should not trigger a full rebuild"
            );

            // Introduce a type error via body-only edit — fast path should detect it
            let (fast, full) = counts(w.as_ref());
            let out = edit_and_cycle(
                w.as_mut(),
                "/home/src/workspaces/project/a.ts",
                r#"export const a: number = "not a number";"#,
            );
            assert!(
                !out.contains("Found 0 errors"),
                "expected errors after type error, got: {out}"
            );
            assert_eq!(
                counts(w.as_ref()).0,
                fast + 1,
                "type error via body edit should still take the fast path"
            );
            assert_eq!(
                counts(w.as_ref()).1,
                full,
                "type error via body edit should not trigger a full rebuild"
            );

            // Fix the type error — fast path should clear it
            let out = edit_and_cycle(
                w.as_mut(),
                "/home/src/workspaces/project/a.ts",
                "export const a: number = 3;",
            );
            assert!(
                out.contains("Found 0 errors"),
                "expected 0 errors after fix, got: {out}"
            );

            // Change b.ts's imported module (./a -> ./c). The set of imported module
            // specifiers changes, so the file cannot be replaced in place and the build
            // must fall back to a full NewProgram rebuild.
            let (fast, full) = counts(w.as_ref());
            let out = edit_and_cycle(
                w.as_mut(),
                "/home/src/workspaces/project/b.ts",
                r#"import { c } from "./c"; export const b = c;"#,
            );
            assert!(
                out.contains("Found 0 errors"),
                "expected 0 errors after import change, got: {out}"
            );
            assert_eq!(
                counts(w.as_ref()).1,
                full + 1,
                "changing the imported module should fall back to a full NewProgram rebuild"
            );
            assert_eq!(
                counts(w.as_ref()).0,
                fast,
                "changing the imported module should not take the fast path"
            );

            // Body edit after import change — should use fast path again, no errors
            let (fast, full) = counts(w.as_ref());
            let out = edit_and_cycle(
                w.as_mut(),
                "/home/src/workspaces/project/b.ts",
                r#"import { c } from "./c"; export const b = c + 1;"#,
            );
            assert!(
                out.contains("Found 0 errors"),
                "expected 0 errors after body edit post-import-change, got: {out}"
            );
            assert_eq!(
                counts(w.as_ref()).0,
                fast + 1,
                "body edit after an import change should take the fast path again"
            );
            assert_eq!(
                counts(w.as_ref()).1,
                full,
                "body edit after an import change should not trigger a full rebuild"
            );
        },
    );
}

// Go: watcher_race_test.go:433 TestWatcherOverflowForcesFullRebuild
/// TestWatcherOverflowForcesFullRebuild verifies that an event-queue overflow
/// forces a full NewProgram rebuild rather than reusing the existing program via
/// the single-file UpdateProgram fast path. For a single-file program with an
/// unresolved import, clearing the source-file cache on overflow leaves exactly
/// one cache miss, which the fast path would misread as a lone content edit and
/// reuse the stale (unresolved) program, never discovering a dependency created
/// while events were dropped. Program membership is observed via the emitted
/// output for the dependency.
#[test]
fn watcher_overflow_forces_full_rebuild() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_overflow_forces_full_rebuild",
        || {
            let input = TscInput {
                files: file_map([
                    (
                        "/home/src/workspaces/project/index.ts",
                        r#"import { dep } from "./dep"; export const x = dep;"#.into(),
                    ),
                    (
                        "/home/src/workspaces/project/tsconfig.json",
                        "{\n\t\t\t\t\"compilerOptions\":{\"noLib\":true,\"moduleResolution\":\"bundler\",\"module\":\"esnext\",\"outDir\":\"out\"},\n\t\t\t\t\"files\":[\"index.ts\"]\n\t\t\t}".into(),
                    ),
                ]),
                command_line_args: vec!["--watch".to_string()],
                ..Default::default()
            };
            let sys = new_in_process_test_sys(&input);
            let result = command_line(
                &context::background(),
                &sys,
                &["--watch", "--pretty", "false"],
            );
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            let fs = sys.fs_from_file_map();

            // The import is initially unresolved, so the dependency is not part of the
            // program and produces no emitted output.
            assert!(
                !fs.file_exists("/home/src/workspaces/project/out/dep.js"),
                "dep.js should not exist while ./dep is unresolved"
            );

            // Create the missing dependency, but deliver an overflow instead of a
            // precise event (as if the create event were dropped by the kernel queue).
            let _ = fs.write_file(
                "/home/src/workspaces/project/dep.ts",
                "export const dep: number = 1;",
            );
            let full = as_execute_watcher(w.as_ref()).full_builds();
            sys.mock_watch_backend().send_overflow();
            w.do_cycle();

            assert_eq!(
                as_execute_watcher(w.as_ref()).full_builds(),
                full + 1,
                "overflow must force a full rebuild, not the single-file fast path"
            );
            assert!(
                fs.file_exists("/home/src/workspaces/project/out/dep.js"),
                "overflow rebuild should rediscover the created dependency and emit dep.js"
            );
        },
    );
}

// Go: watcher_race_test.go:477 TestWatcherNonSourceDependencyForcesFullRebuild
/// TestWatcherNonSourceDependencyForcesFullRebuild verifies that when a
/// non-source build dependency changes in the same cycle as a source edit, the
/// single-file fast path is rejected. A previously-missing module path is
/// tracked in seenFiles (via failed resolution) but is never stored in the
/// source-file cache, so counting source-cache misses alone would report a lone
/// changed file and reuse stale module resolutions.
#[test]
fn watcher_non_source_dependency_forces_full_rebuild() {
    run_test_in_child(
        "tsctests::watcher_race::watcher_non_source_dependency_forces_full_rebuild",
        || {
            let input = TscInput {
                files: file_map([
                    (
                        "/home/src/workspaces/project/index.ts",
                        r#"import { dep } from "./dep"; export const x = dep;"#.into(),
                    ),
                    (
                        "/home/src/workspaces/project/tsconfig.json",
                        "{\n\t\t\t\t\"compilerOptions\":{\"noLib\":true,\"moduleResolution\":\"bundler\",\"module\":\"esnext\",\"outDir\":\"out\"},\n\t\t\t\t\"files\":[\"index.ts\"]\n\t\t\t}".into(),
                    ),
                ]),
                command_line_args: vec!["--watch".to_string()],
                ..Default::default()
            };
            let sys = new_in_process_test_sys(&input);
            let result = command_line(
                &context::background(),
                &sys,
                &["--watch", "--pretty", "false"],
            );
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            let fs = sys.fs_from_file_map();

            // "./dep" is unresolved initially; the failed resolution probes dep.ts,
            // recording it as a (missing) non-source dependency in seenFiles.
            assert!(
                !fs.file_exists("/home/src/workspaces/project/out/dep.js"),
                "dep.js should not exist while ./dep is unresolved"
            );

            // In a single cycle, edit index.ts's body (a lone source-cache miss) and
            // create the previously-missing dependency. The batched dependency change
            // must reject the fast path so the new module resolution is discovered.
            let _ = fs.write_file(
                "/home/src/workspaces/project/index.ts",
                r#"import { dep } from "./dep"; export const x = dep + 0;"#,
            );
            let _ = fs.write_file(
                "/home/src/workspaces/project/dep.ts",
                "export const dep: number = 1;",
            );
            let full = as_execute_watcher(w.as_ref()).full_builds();
            sys.mock_watch_backend().send_events(vec![
                Event {
                    kind: EventKind::Update,
                    path: "/home/src/workspaces/project/index.ts".to_string(),
                },
                Event {
                    kind: EventKind::Update,
                    path: "/home/src/workspaces/project/dep.ts".to_string(),
                },
            ]);
            w.do_cycle();

            assert_eq!(
                as_execute_watcher(w.as_ref()).full_builds(),
                full + 1,
                "a changed non-source dependency must force a full rebuild, not the fast path"
            );
            assert!(
                fs.file_exists("/home/src/workspaces/project/out/dep.js"),
                "full rebuild should resolve the created dependency and emit dep.js"
            );
        },
    );
}
