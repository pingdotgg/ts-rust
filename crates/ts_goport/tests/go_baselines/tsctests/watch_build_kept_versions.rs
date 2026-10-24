//! Port-only tests of the file versions of `tsc -b --watch` (watchfix1
//! part 2, then watchreset1). Until watchreset1 the port kept parses from
//! one cycle to the next, and these tests checked which kept parses a
//! config change dropped (item 3) and that a task whose status is reset
//! lets go of the versions that its errors held (item 4). Since
//! watchreset1 the port keeps no parse across cycles, as Go
//! (build/orchestrator.go:506-513 `resetCaches`, build/host.go:50-56): each
//! cycle parses again every file of each project that it builds, the
//! bundled libs too. The tests keep their names. Each one now checks that
//! the count of new versions per cycle is Go's count of parses (an
//! instrumented Go at 3e9236361fa9 that runs the same sessions on Go's
//! test system: target/continuation-r97-goport/after231/watchreset1,
//! goinst/watchreset_parse_test.go and logs/go-wr1-test.log), and that each
//! version dies after its cycle unless an error of its task holds it.
//!
//! Go has no file versions: its GC frees a file when no program, cache or
//! diagnostic holds it. The output of each cycle is Go's: the lines that
//! the tests check are those of `tsgo-oracle-673a5f17d713 -b -w` on the same
//! files and edits with the OS watcher
//! (target/continuation-r97-goport/watchfix1/runs, mdrop and held2,
//! watchfix1/rb/runs/mjx, and watchfix1/rc/watch/runs, leg, frc and two).
//! The first version of each file is static; each later parse of it is a
//! freeable version (`ast::file_versions_made`, `ast::dead_file_versions`).

use std::time::{Duration, Instant};

use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::context;

use crate::support::child::{command_line_in_process, new_in_process_test_sys, run_test_in_child};
use crate::support::runner::TscInput;

const PROJECT: &str = "/home/src/workspaces/project";

/// A watch session of `args` on `files` (paths relative to `PROJECT`). Each
/// `edit` writes one file, sends its event and runs one cycle, then `check`
/// gets the edit's index and the cycle's output.
fn session(
    files: &[(&str, &str)],
    args: &[&str],
    edits: &[(&str, &str)],
    mut check: impl FnMut(usize, &str),
) {
    let input = TscInput {
        files: files
            .iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), (*text).into()))
            .collect(),
        ..Default::default()
    };
    let sys = new_in_process_test_sys(&input);
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    let result = command_line_in_process(&context::background(), &sys, &args);
    let mut w = result
        .watcher
        .expect("expected Watcher to be non-nil in watch mode");
    let fs = sys.fs_from_file_map();
    let out = sys.output_text();
    assert!(
        out.contains("Found 0 errors. Watching for file changes."),
        "first build: {out}"
    );
    for (i, (name, text)) in edits.iter().enumerate() {
        sys.set_output_bytes(Vec::new());
        let path = format!("{PROJECT}/{name}");
        let _ = fs.write_file(&path, text);
        sys.mock_watch_backend().send_events(vec![Event {
            kind: EventKind::Update,
            path,
        }]);
        w.do_cycle();
        check(i, &sys.output_text());
    }
}

/// `(made, dead)` file versions once the dead count reaches `dead` or 10 s
/// pass: a version of a released program that no other holder keeps dies
/// at the release, and a free thread frees its data.
fn file_versions(dead: usize) -> (usize, usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        ts_goport::program::wait_for_background_releases();
        let now = (
            ts_goport::ast::file_versions_made(),
            ts_goport::ast::dead_file_versions(),
        );
        if now.1 >= dead || Instant::now() > deadline {
            return now;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Two script files, so each parse reads its module indicator options.
fn config(detection: &str, new_line: &str, files: &str) -> String {
    format!(
        r#"{{"compilerOptions":{{"strict":true,"outDir":"out","rootDir":"src","types":[],"lib":["es5"],"moduleDetection":"{detection}"{new_line}}},"files":[{files}]}}"#
    )
}

#[test]
fn build_watch_drops_kept_parses_whose_module_indicator_options_changed() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_drops_kept_parses_whose_module_indicator_options_changed",
        || {
            let both = r#""src/s1.ts","src/s2.ts""#;
            let first = config("auto", "", both);
            let new_line = config("auto", r#","newLine":"lf""#, both);
            let force = config("force", r#","newLine":"lf""#, r#""src/s1.ts""#);
            session(
                &[
                    ("tsconfig.json", &first),
                    ("src/s1.ts", "const one: number = 1;\n"),
                    ("src/s2.ts", "const two: number = 2;\n"),
                ],
                &["--build", "--watch", "--pretty", "false"],
                &[
                    ("src/s2.ts", "const two: number = 22;\n"),
                    ("tsconfig.json", &new_line),
                    ("tsconfig.json", &force),
                ],
                |i, out| {
                    assert!(
                        out.contains("Found 0 errors. Watching for file changes."),
                        "edit {i}: {out}"
                    );
                    // Each cycle parses lib.es5.d.ts, s1.ts and s2.ts again
                    // (Go: 3 parses), and the last one, with `force` and
                    // only s1.ts, lib.es5.d.ts and s1.ts (Go: 2). No error
                    // holds them, so each dies after its cycle.
                    let expected = [(3, 3), (6, 6), (8, 8)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}

#[test]
fn build_watch_keeps_kept_parses_whose_module_indicator_inputs_stay() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_keeps_kept_parses_whose_module_indicator_inputs_stay",
        || {
            // A script file, whose parse reads its module indicator
            // options, and a module file, whose parse does not.
            let config = |module: &str, jsx: &str| {
                format!(
                    r#"{{"compilerOptions":{{"strict":true,"outDir":"out","rootDir":"src","types":[],"lib":["es5"],"module":"{module}","jsx":"{jsx}"}},"files":["src/s1.ts","src/m1.ts"]}}"#
                )
            };
            session(
                &[
                    ("tsconfig.json", &config("es2020", "preserve")),
                    ("src/s1.ts", "const one: number = 1;\n"),
                    ("src/m1.ts", "export const m: number = 1;\n"),
                ],
                &["--build", "--watch", "--pretty", "false"],
                &[
                    ("src/s1.ts", "const one: number = 11;\n"),
                    ("tsconfig.json", &config("esnext", "preserve")),
                    ("tsconfig.json", &config("esnext", "react")),
                    ("tsconfig.json", &config("esnext", "react-jsx")),
                ],
                |i, out| {
                    assert!(
                        out.contains("Found 0 errors. Watching for file changes."),
                        "edit {i}: {out}"
                    );
                    // Each cycle parses lib.es5.d.ts, s1.ts and m1.ts again
                    // (Go: 3 parses), also when the `module` and `jsx`
                    // edits give no file other module indicator options.
                    // No error holds them, so each dies after its cycle.
                    let expected = [(3, 3), (6, 6), (9, 9), (12, 12)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}

/// A config of the files `files` with the `moduleDetection` value
/// `detection` (none for `None`).
fn detection_config(detection: Option<&str>, files: &str) -> String {
    let detection = detection.map_or(String::new(), |kind| {
        format!(r#","moduleDetection":"{kind}""#)
    });
    format!(
        r#"{{"compilerOptions":{{"strict":true,"outDir":"out","rootDir":"src","types":[],"lib":["es5"]{detection}}},"files":[{files}]}}"#
    )
}

#[test]
fn build_watch_keeps_script_parses_from_legacy_to_auto_and_back() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_keeps_script_parses_from_legacy_to_auto_and_back",
        || {
            // Two script files, s1.ts and q1.mts, and a module file.
            let files = r#""src/s1.ts","src/q1.mts","src/m1.ts""#;
            let legacy = detection_config(Some("legacy"), files);
            session(
                &[
                    ("tsconfig.json", &legacy),
                    ("src/s1.ts", "const one: number = 1;\n"),
                    ("src/q1.mts", "const q: number = 1;\n"),
                    ("src/m1.ts", "export const m: number = 1;\n"),
                ],
                &["--build", "--watch", "--pretty", "false"],
                &[
                    ("src/s1.ts", "const one: number = 11;\n"),
                    ("src/q1.mts", "const q: number = 11;\n"),
                    ("tsconfig.json", &detection_config(None, files)),
                    ("tsconfig.json", &legacy),
                ],
                |i, out| {
                    assert!(
                        out.contains("Found 0 errors. Watching for file changes."),
                        "edit {i}: {out}"
                    );
                    // Each cycle parses lib.es5.d.ts, s1.ts, q1.mts and m1.ts
                    // again (Go: 4 parses), whether the module indicator
                    // options of a file change or not. No error holds
                    // them, so each dies after its cycle.
                    let expected = [(4, 4), (8, 8), (12, 12), (16, 16)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}

#[test]
fn build_watch_keeps_mts_and_cts_script_parses_on_a_force_edit() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_keeps_mts_and_cts_script_parses_on_a_force_edit",
        || {
            // Three script files.
            let files = r#""src/s1.ts","src/q1.mts","src/c1.cts""#;
            let auto = detection_config(None, files);
            session(
                &[
                    ("tsconfig.json", &auto),
                    ("src/s1.ts", "const one: number = 1;\n"),
                    ("src/q1.mts", "const q: number = 1;\n"),
                    ("src/c1.cts", "const c: number = 1;\n"),
                ],
                &["--build", "--watch", "--pretty", "false"],
                &[
                    ("src/q1.mts", "const q: number = 11;\n"),
                    ("src/c1.cts", "const c: number = 11;\n"),
                    ("tsconfig.json", &detection_config(Some("force"), files)),
                    ("tsconfig.json", &auto),
                ],
                |i, out| {
                    assert!(
                        out.contains("Found 0 errors. Watching for file changes."),
                        "edit {i}: {out}"
                    );
                    // Each cycle parses lib.es5.d.ts, s1.ts, q1.mts and c1.cts
                    // again (Go: 4 parses), also when `force` gives q1.mts
                    // and c1.cts the options that their extension gives
                    // with `auto`. No error holds them, so each dies after
                    // its cycle.
                    let expected = [(4, 4), (8, 8), (12, 12), (16, 16)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}

#[test]
fn build_watch_keeps_the_parses_of_a_project_whose_config_stays() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_keeps_the_parses_of_a_project_whose_config_stays",
        || {
            let project = |detection: &str, files: &str| {
                format!(
                    r#"{{"compilerOptions":{{"composite":true,"strict":true,"outDir":"out","rootDir":"src","types":[],"lib":["es5"]{detection}}},"files":[{files}]}}"#
                )
            };
            // Projects `a` and `b` do not reference each other.
            session(
                &[
                    (
                        "tsconfig.json",
                        r#"{"files":[],"references":[{"path":"./a"},{"path":"./b"}]}"#,
                    ),
                    ("a/tsconfig.json", &project("", r#""src/s.ts""#)),
                    ("b/tsconfig.json", &project("", r#""src/s.ts","src/m.ts""#)),
                    ("a/src/s.ts", "const a: number = 1;\n"),
                    ("b/src/s.ts", "const b: number = 1;\n"),
                    ("b/src/m.ts", "export const m: number = 1;\n"),
                ],
                &["--build", "--watch", "--pretty", "false"],
                &[
                    ("b/src/s.ts", "const b: number = 11;\n"),
                    (
                        "a/tsconfig.json",
                        &project(r#","moduleDetection":"force""#, r#""src/s.ts""#),
                    ),
                    ("b/src/m.ts", "export const m: number = 11;\n"),
                ],
                |i, out| {
                    assert!(
                        out.contains("Found 0 errors. Watching for file changes."),
                        "edit {i}: {out}"
                    );
                    // Only the project of the edit builds, and it parses
                    // each of its files and lib.es5.d.ts again: `b` (Go: 3
                    // parses), then `a` (Go: 2), then `b` (Go: 3). No error
                    // holds them, so each dies after its cycle.
                    let expected = [(3, 3), (5, 5), (8, 8)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}

#[test]
fn build_watch_reset_status_lets_go_of_held_versions() {
    run_test_in_child(
        "tsctests::watch_build_kept_versions::build_watch_reset_status_lets_go_of_held_versions",
        || {
            let project = |references: &str| {
                format!(
                    r#"{{"compilerOptions":{{"composite":true,"strict":true,"target":"es2015","module":"esnext","lib":["es5"],"types":[],"rootDir":"src","outDir":"out"}}{references},"include":["src"]}}"#
                )
            };
            let core = project("");
            let app = project(r#","references":[{"path":"../core"}]"#);
            let b_error = "app/src/b.ts(1,14): error TS2322: Type 'string' is not assignable to type 'number'.";
            let a_error = "core/src/a.ts(1,14): error TS2322: Type 'string' is not assignable to type 'number'.";
            session(
                &[
                    (
                        "tsconfig.json",
                        r#"{"files":[],"references":[{"path":"./core"},{"path":"./app"}]}"#,
                    ),
                    ("core/tsconfig.json", &core),
                    ("app/tsconfig.json", &app),
                    ("core/src/a.ts", "export const a: number = 1;\n"),
                    ("app/src/b.ts", "export const b: number = 1;\n"),
                ],
                &[
                    "--build",
                    "--watch",
                    "--pretty",
                    "false",
                    "--stopBuildOnErrors",
                ],
                &[
                    ("app/src/b.ts", "export const b: number = \"one\";\n"),
                    ("core/src/a.ts", "export const a: number = \"one\";\n"),
                    ("app/src/b.ts", "export const b: number = \"two\";\n"),
                ],
                |i, out| {
                    // Go: b.ts v2 has the error of `app`, then `core` has
                    // one too, then `app` is reset by the b.ts edit and
                    // skipped (its dependency has errors), so its errors
                    // and b.ts are not read again.
                    let (a, b, found) = match i {
                        0 => (false, true, "Found 1 error."),
                        1 => (true, true, "Found 2 errors."),
                        _ => (true, false, "Found 1 error."),
                    };
                    assert_eq!(out.contains(a_error), a, "edit {i}: {out}");
                    assert_eq!(out.contains(b_error), b, "edit {i}: {out}");
                    assert!(out.contains(found), "edit {i}: {out}");
                    // The build of `app` parses lib.es5.d.ts and b.ts v2
                    // (Go: 2 parses), then the build of `core`
                    // lib.es5.d.ts and a.ts v2 (Go: 2), and the last cycle
                    // builds nothing (Go: 0). Each lib version dies after
                    // its cycle. The errors of `app` hold b.ts v2 and
                    // those of `core` a.ts v2. The last edit of b.ts
                    // resets the status of `app` (`reset_status`), which
                    // lets go of b.ts v2, so it dies.
                    let expected = [(2, 1), (4, 2), (4, 3)][i];
                    assert_eq!(file_versions(expected.1), expected, "edit {i}");
                },
            );
        },
    );
}
