//! Multi-program checks for `goport_multiprog pair` and
//! `goport_live_programs`.
//!
//! The `pair` tests copy `fixtures/multiprog/basic` to a new directory under
//! the system temp dir and run `goport_multiprog pair` there. Program A is
//! the original project. Program B is the project after one edit of
//! `src/a.ts`. Each report must be byte-identical to a fresh
//! `goport -p tsconfig.json` on the same text, with the same exit code.
//!
//! The live tests hold several projects (`basic`, `emit`, `linked` and `cut`
//! under `fixtures/multiprog`) at once, as the language server does. Each
//! report must equal a fresh `goport` or `goport_emit` run of that project
//! alone.
//!
//! The watch tests run `tsc --watch` through `goport_watch` and edit
//! `src/a.ts` (and `src/c.ts` or `tsconfig.json`) between builds.
//!
//! The build test runs `goport_build -b` on `fixtures/multiprog/build-dedup`,
//! whose projects share parsed files in one process, as Go `tsc -b` does.
//!
//! The serial bind test runs `goport_typesyms` and `tsgo` with one bind
//! thread and with four, and compares their outputs.
//!
//! `pair` writes these files to its out dir: `a.txt`, `a.status`, `b.txt`,
//! `b.status`, `b.reused` and, with `--first`, `first.txt`. A status file
//! holds the decimal exit code. `b.reused` holds `true` or `false`.
//!
//! Each test uses its own directory and processes, so the tests can run in
//! parallel. A passing test deletes its directory. A failing test keeps it
//! and names it in the message.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ts_goport::execute::tsc::EXIT_UNPORTED;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/basic"
);

const EMIT_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/multiprog/emit");

const WATCH_HELD_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/watch-held"
);

const WATCH_BUILD_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/watch-build"
);

const WATCH_CONFIG_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/watch-config"
);

const WATCH_STALE_DTS_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/watch-stale-dts"
);

const BUILD_DEDUP_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/multiprog/build-dedup"
);

/// The file that each test edits, relative to the project.
const CHANGED: &str = "src/a.ts";

/// The stdout text and exit code of one report.
#[derive(Debug, PartialEq)]
struct Report {
    stdout: String,
    status: i32,
}

/// The two reports of one `pair` run, and whether B reused A's files.
struct Pair {
    a: Report,
    b: Report,
    reused: bool,
}

#[test]
fn edit_that_keeps_imports_shares_files() {
    let pair = check_pair("keeps-imports", Some("edits/a.ts"), false);
    assert!(
        pair.reused,
        "an edit with the same imports must reuse the other files"
    );
    assert_new_c_error(&pair);
}

#[test]
fn edit_that_changes_imports_rebuilds() {
    let pair = check_pair("changes-imports", Some("edits/a-imports.ts"), false);
    assert!(
        !pair.reused,
        "an edit that adds an import must rebuild the program"
    );
    assert_new_c_error(&pair);
}

#[test]
fn no_op_edit_equals_first_program() {
    let pair = check_pair("no-op", None, false);
    assert_eq!(pair.b, pair.a, "the same text must give the same report");
}

#[test]
fn programs_after_an_unrelated_program() {
    let pair = check_pair("after-first", Some("edits/a.ts"), true);
    assert!(
        pair.reused,
        "an edit with the same imports must reuse the other files"
    );
    assert_new_c_error(&pair);
}

/// lsshells M3b. `GOPORT_FREE_FILE_VERSIONS=1` turns freeing on in
/// `goport_multiprog` (a CLI process, where it is off by default), and
/// `GOPORT_OWNED_NODES=1` makes the parse of each freed version own its
/// nodes (M3c, off by default), so the checker reads them in scopes.
const FREE_FILE_VERSIONS: &[(&str, &str)] = &[
    ("GOPORT_FREE_FILE_VERSIONS", "1"),
    ("GOPORT_OWNED_NODES", "1"),
];

/// lsshells M3b: with freeing on, each new parse of B is a freeable file
/// version (its store and `GoFile` belong to the version, not to a leaked
/// tier 1 publish), and `goport_multiprog` checks that. A and B still report
/// like fresh runs, for an edit that keeps the imports (only the changed
/// file is new) and for one that adds an import (every file is new).
// PORT: no Go counterpart.
#[test]
fn freeable_file_versions_report_like_fresh_runs() {
    let pair = check_pair_with_env(
        "free-keeps-imports",
        Some("edits/a.ts"),
        false,
        FREE_FILE_VERSIONS,
    );
    assert!(
        pair.reused,
        "an edit with the same imports must reuse the other files"
    );
    assert_new_c_error(&pair);
    let pair = check_pair_with_env(
        "free-changes-imports",
        Some("edits/a-imports.ts"),
        false,
        FREE_FILE_VERSIONS,
    );
    assert!(
        !pair.reused,
        "an edit that adds an import must rebuild the program"
    );
    assert_new_c_error(&pair);
}

/// lsshells M3b: `goport_multiprog cycles` with freeing on frees every
/// freeable file version once no program has it: after the last release,
/// each version it made is dead. Each report equals the report of the last
/// version with the same text (`cycles` checks it). A CLI process with the
/// flag unset makes no file version and frees nothing.
// PORT: no Go counterpart.
#[test]
fn cycles_free_file_versions_only_when_the_flag_is_on() {
    const CYCLES: usize = 4;
    let (made, dead) = run_cycles("cycles-default", CYCLES, &[]);
    assert_eq!((made, dead), (0, 0), "a CLI process makes no file version");
    let (made, dead) = run_cycles("cycles-free", CYCLES, FREE_FILE_VERSIONS);
    assert!(
        made >= CYCLES,
        "each cycle parses the changed file again, so it makes a file version (made {made})"
    );
    assert_eq!(
        dead, made,
        "a file version outlives every program that had it (a missed holder)"
    );
}

/// editfast1: with `GOPORT_CHECK_VERSION_TABLES=1`, `goport_multiprog`
/// builds the tables of each version that replaces files of the old one in
/// place (`go_frontend::reused_tables`) again from its files alone, and
/// panics when they differ. A pair passes with it, and so do cycles, where
/// each version starts from tables made that way and the file versions are
/// freeable. The cycles still free every file version.
// PORT: no Go counterpart.
#[test]
fn reused_version_tables_equal_a_full_build() {
    const CHECK: (&str, &str) = ("GOPORT_CHECK_VERSION_TABLES", "1");
    const CYCLES: usize = 4;
    let pair = check_pair_with_env("tables-check", Some("edits/a.ts"), false, &[CHECK]);
    assert!(
        pair.reused,
        "an edit with the same imports must reuse the other files"
    );
    assert_new_c_error(&pair);
    let env = [FREE_FILE_VERSIONS[0], FREE_FILE_VERSIONS[1], CHECK];
    let (made, dead) = run_cycles("tables-check-cycles", CYCLES, &env);
    assert!(
        made >= CYCLES,
        "each cycle parses the changed file again, so it makes a file version (made {made})"
    );
    assert_eq!(
        dead, made,
        "a file version outlives every program that had it (a missed holder)"
    );
}

/// Runs `goport_multiprog cycles` `count` times on a new copy of the fixture
/// with `env` set, and gives the numbers of its last line,
/// `file_versions made=<n> dead=<m>`. `GOPORT_FREE_FILE_VERSIONS` is unset
/// unless `env` sets it, so the test environment does not change the
/// default.
fn run_cycles(test: &str, count: usize, env: &[(&str, &str)]) -> (usize, usize) {
    let root = scratch_dir(test);
    let project = root.join("project");
    copy_dir(Path::new(FIXTURE), &project);
    let changed = project.join(CHANGED);
    let original = read(&changed);
    let run = Command::new(env!("CARGO_BIN_EXE_goport_multiprog"))
        .arg("cycles")
        .arg("tsconfig.json")
        .arg(norm(&changed))
        .arg(count.to_string())
        .env_remove("GOPORT_FREE_FILE_VERSIONS")
        .envs(env.iter().copied())
        .current_dir(&project)
        .output()
        .expect("run goport_multiprog");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "goport_multiprog cycles failed ({}) in {}:\n{stdout}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(read(&changed), original, "cycles must restore {CHANGED}");
    let counts = stdout
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("file_versions made="))
        .and_then(|rest| rest.split_once(" dead="))
        .and_then(|(made, dead)| Some((made.parse().ok()?, dead.parse().ok()?)))
        .unwrap_or_else(|| panic!("no file_versions line in the cycles output:\n{stdout}"));
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
    counts
}

/// The projects of the live tests: the fixture dir, the
/// `goport_live_programs` mode and a line that its `checker.txt` must hold.
/// The `emit` and `linked` lines are `import(...)` types, so the checkers
/// on the shared thread make module specifiers. The `linked` one needs the
/// symlink cache of its own program (see `link_shelf`).
// Only the Unix-only test `three_live_projects_report_like_fresh_runs` uses this.
#[cfg(unix)]
const LIVE_PROJECTS: [(&str, &str, &str); 3] = [
    ("basic", "check", "side: \"left\" | \"right\""),
    ("emit", "emit", "b: import(\"./shapes\").Box"),
    ("linked", "emit", "second: import(\"shelf\").Book"),
];

/// Three live projects, loaded in both orders: `basic` checked like
/// `goport`, `emit` and `linked` compiled like `goport_emit`. Each report
/// and output equals a fresh run of that project alone. The checkers of all
/// programs share the loading thread (`checker.txt`, see
/// `goport_live_programs`), and what they write does not depend on the load
/// order or the other programs.
#[cfg(unix)]
#[test]
fn three_live_projects_report_like_fresh_runs() {
    let root = scratch_dir("live");
    let fixtures = Path::new(FIXTURE).parent().expect("fixture root");
    for (dir, _, _) in LIVE_PROJECTS {
        copy_dir(&fixtures.join(dir), &root.join(dir));
    }
    link_shelf(&root.join("linked"));
    let projects: Vec<String> = LIVE_PROJECTS
        .iter()
        .map(|(dir, mode, _)| format!("{mode}:{dir}/tsconfig.json"))
        .collect();
    let mut args = vec!["live", "out-1"];
    args.extend(projects.iter().map(String::as_str));
    run_live_programs(&root, &args);
    let mut args = vec!["live", "out-2"];
    args.extend(projects.iter().rev().map(String::as_str));
    run_live_programs(&root, &args);

    let last = LIVE_PROJECTS.len() - 1;
    for (i, (dir, mode, line)) in LIVE_PROJECTS.into_iter().enumerate() {
        let config = format!("{dir}/tsconfig.json");
        let fresh_dir = format!("fresh-{dir}");
        let (fresh, fresh_outputs) = if mode == "check" {
            (goport(&root, Path::new(&config)), None)
        } else {
            let fresh = goport_emit(&root, &config, &fresh_dir);
            (fresh, Some(read_tree(&root.join(&fresh_dir))))
        };
        let mut texts = Vec::new();
        for (out, index) in [("out-1", i), ("out-2", last - i)] {
            let program = root.join(out).join(index.to_string());
            assert_eq!(
                read_live_report(&program),
                fresh,
                "{out} {dir}: report against a fresh run ({})",
                root.display()
            );
            if let Some(fresh_outputs) = &fresh_outputs {
                assert_eq!(
                    read_tree(&program.join("out")),
                    *fresh_outputs,
                    "{out} {dir}: outputs against goport_emit ({})",
                    root.display()
                );
            }
            texts.push(read(&program.join("checker.txt")));
        }
        assert_eq!(
            texts[0],
            texts[1],
            "{dir}: checker text depends on the load order ({})",
            root.display()
        );
        assert!(
            texts[0].lines().any(|text| text == line),
            "{dir}: checker text has no line {line:?}:\n{}",
            texts[0]
        );
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// A type longer than Go's limit is cut inside a 2-byte char (the `cut`
/// fixture), in a live program next to another one. Go `typeToStringEx`
/// keeps the first 317 bytes and adds "...", so the type line in
/// `checker.txt` ends with the first byte of the char. Go
/// `diagnostics.Format` turns that byte into U+FFFD in the error message.
/// Pinned tsgo prints the same message.
#[test]
fn type_cut_inside_a_char_keeps_go_bytes() {
    let root = scratch_dir("cut");
    let fixtures = Path::new(FIXTURE).parent().expect("fixture root");
    for dir in ["basic", "cut"] {
        copy_dir(&fixtures.join(dir), &root.join(dir));
    }
    run_live_programs(
        &root,
        &[
            "live",
            "out",
            "check:basic/tsconfig.json",
            "check:cut/tsconfig.json",
        ],
    );
    let program = root.join("out").join("1");

    let source = read(&root.join("cut/src/word.ts"));
    let word = source
        .split('"')
        .find(|text| text.starts_with('x') && text.len() > 320)
        .expect("the fixture declares a long word");
    let literal = format!("\"{word}\"");
    let cut = &literal.as_bytes()[..317];
    assert!(
        std::str::from_utf8(cut).is_err(),
        "byte 317 of the fixture type must be inside a char"
    );

    let line = [b"word: ", cut, b"..."].concat();
    let checker = fs::read(program.join("checker.txt")).expect("read checker.txt");
    assert!(
        checker.split(|&b| b == b'\n').any(|text| text == line),
        "checker.txt has no line {:?} ({})",
        String::from_utf8_lossy(&line),
        root.display()
    );

    let report = read_live_report(&program);
    assert_eq!(
        report,
        goport(&root, Path::new("cut/tsconfig.json")),
        "report against a fresh run ({})",
        root.display()
    );
    let message = format!(
        "Type '{}...' is not assignable to type '\"x\"'.",
        String::from_utf8_lossy(cut)
    );
    assert!(
        report.stdout.contains(&message),
        "the report has no message {message:?}:\n{}",
        report.stdout
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// Links `node_modules/shelf` to `packages/shelf` in a copy of the `linked`
/// fixture, as a workspace package manager does. The program resolves
/// `shelf` through the link and keeps it in its symlink cache.
#[cfg(unix)]
fn link_shelf(project: &Path) {
    let modules = project.join("node_modules");
    fs::create_dir(&modules)
        .unwrap_or_else(|error| panic!("create {}: {error}", modules.display()));
    std::os::unix::fs::symlink("../packages/shelf", modules.join("shelf"))
        .unwrap_or_else(|error| panic!("link shelf in {}: {error}", project.display()));
}

/// A program is released (its checker workers joined) while a job of another
/// program waits on that program's pool. Both still report like fresh runs.
#[test]
fn release_while_another_program_checks_on_the_pool() {
    let root = scratch_dir("release");
    copy_dir(Path::new(FIXTURE), &root.join("p"));
    copy_dir(Path::new(EMIT_FIXTURE), &root.join("q"));
    run_live_programs(
        &root,
        &["release", "out", "p/tsconfig.json", "q/tsconfig.json"],
    );
    let out = root.join("out");
    for name in ["p", "q"] {
        let config = format!("{name}/tsconfig.json");
        assert_eq!(
            read_report(&out, name),
            goport(&root, Path::new(&config)),
            "{name} against goport ({})",
            root.display()
        );
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// `tsc --watch` makes a program version per build (`goport_watch`). After
/// each edit of `src/a.ts`, the errors of the build equal a fresh `goport`
/// run on the same text. The first edit gives the unchanged `c.ts` an error,
/// so the build sees the edit through a file it shares with the last build.
#[test]
fn watch_builds_report_like_fresh_runs() {
    let root = scratch_dir("watch");
    let project = root.join("project");
    copy_dir(Path::new(FIXTURE), &project);
    let changed = project.join(CHANGED);
    let original = read(&changed);
    let original_file = root.join("original.ts");
    write(&original_file, &original);
    let edits = [
        project.join("edits/a.ts"),
        project.join("edits/a-imports.ts"),
        original_file,
    ];
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg(CHANGED)
        .args(edits.iter().map(|edit| norm(edit)))
        .args(["--", "--watch", "-p", "tsconfig.json", "--noEmit"])
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        read(&changed),
        original,
        "goport_watch must restore {CHANGED}"
    );

    let watch = read(&out);
    let builds: Vec<&str> = watch
        .split_inclusive("Watching for file changes.")
        .filter(|build| build.contains("Watching for file changes."))
        .collect();
    assert_eq!(
        builds.len(),
        4,
        "one build per edit after the first:\n{watch}"
    );
    let texts = [original.clone(), read(&edits[0]), read(&edits[1]), original];
    for (i, (build, text)) in builds.iter().zip(&texts).enumerate() {
        write(&changed, text);
        let fresh = goport(&project, Path::new("tsconfig.json"));
        assert_eq!(
            error_lines(build),
            error_lines(&fresh.stdout),
            "build {i} against goport ({})",
            root.display()
        );
        assert_eq!(
            build.contains("src/c.ts("),
            i == 1 || i == 2,
            "build {i}: c.ts error:\n{build}"
        );
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// watchfree1: from its second build on, `tsc --watch` frees the old file
/// versions (`ast::set_watch_process`). The builds: a body edit of
/// `src/a.ts`, a touch (the same text again), an edit that changes its
/// imports (a full build), a `tsconfig.json` edit and the original text.
/// The config edit is a full build that keeps the parse of each file with
/// the same parse options and text and no errors in the last build
/// (watchcfg1, `WatchCompilerHost::reuse_parse`): it parses again only
/// `src/b.ts`, `src/c.ts` and `src/d.js`, which have errors. The errors of
/// each build equal a fresh `goport` run on the same files. At exit each
/// file version that the last program does not have is dead: the three
/// replaced versions of `src/a.ts`. The first version of each file is
/// static, so no other version dies.
// PORT: no Go counterpart.
#[test]
fn watch_frees_file_versions() {
    let root = scratch_dir("watch-free");
    // Go `CanWatchDirectory` does not watch `/tmp/<dir>/project`, so the
    // config file is one level deeper than in the other tests.
    let project = root.join("work").join("project");
    copy_dir(Path::new(FIXTURE), &project);
    let changed = project.join(CHANGED);
    let config = project.join("tsconfig.json");
    let original = read(&changed);
    let original_config = read(&config);
    let original_file = root.join("original.ts");
    write(&original_file, &original);
    // `newLine` changes the parsed config, so the build is a full build,
    // and changes no error.
    let new_config = original_config.replace(
        "\"declaration\": true",
        "\"declaration\": true,\n    \"newLine\": \"lf\"",
    );
    assert_ne!(new_config, original_config);
    let new_config_file = root.join("tsconfig.json");
    write(&new_config_file, &new_config);
    let config_edit = format!("{}={}", norm(&config), norm(&new_config_file));
    let edits = [
        norm(&project.join("edits/a.ts")),
        norm(&project.join("edits/a.ts")),
        norm(&project.join("edits/a-imports.ts")),
        config_edit,
        norm(&original_file),
    ];
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg(CHANGED)
        .args(&edits)
        .args(["--", "--watch", "-p", "tsconfig.json", "--noEmit"])
        .env_remove("GOPORT_FREE_FILE_VERSIONS")
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{stdout}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        read(&changed),
        original,
        "goport_watch must restore {CHANGED}"
    );
    assert_eq!(
        read(&config),
        original_config,
        "goport_watch must restore tsconfig.json"
    );

    let watch = read(&out);
    let builds: Vec<&str> = watch
        .split_inclusive("Watching for file changes.")
        .filter(|build| build.contains("Watching for file changes."))
        .collect();
    assert_eq!(
        builds.len(),
        6,
        "one build per edit after the first:\n{watch}"
    );
    let edited = read(&project.join("edits/a.ts"));
    let states = [
        (&original, &original_config),
        (&edited, &original_config),
        (&edited, &original_config),
        (&read(&project.join("edits/a-imports.ts")), &original_config),
        (&read(&project.join("edits/a-imports.ts")), &new_config),
        (&original, &new_config),
    ];
    for (i, (build, (text, config_text))) in builds.iter().zip(states).enumerate() {
        write(&changed, text);
        write(&config, config_text);
        let fresh = goport(&project, Path::new("tsconfig.json"));
        assert_eq!(
            error_lines(build),
            error_lines(&fresh.stdout),
            "build {i} against goport ({})",
            root.display()
        );
    }
    let (made, dead) = file_version_counts(&stdout);
    assert_eq!(
        dead, 3,
        "each replaced version of {CHANGED} dies, and no other (made {made})"
    );
    assert_eq!(
        made, 7,
        "three edits of {CHANGED}, the three files with errors that the config edit parses again, and the original text"
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// watchfree1: a diagnostic that a watch build copies from the last build
/// (Go `repopulateDiagnosticsOfFile`, an unchanged file) points at the file
/// versions of the build that made it, and Go prints it from them. `c.ts`
/// has TS2741, whose related information points into `a.ts`. An edit that
/// changes the signature of `a.ts` checks `c.ts` again; two edits that move
/// its lines and keep its signature copy the error, which still points at
/// the first edited version. A comment in `c.ts` checks it again, and a
/// touch of `c.ts` copies the error, which points at the version before the
/// touch. `--pretty` prints the lines of each version. The output (times
/// and clear-screen codes removed) equals `expected.txt`, the output of
/// `tsgo-oracle-673a5f17d713 -w -p tsconfig.json --pretty` for the same
/// edits. The emitted files equal a fresh `goport_emit` of the last texts.
/// At exit the four versions that no copied error holds are dead.
// PORT: no Go counterpart for the file versions; the output is Go's.
#[test]
fn watch_copied_errors_read_their_file_versions() {
    let root = scratch_dir("watch-held");
    let project = root.join("project");
    copy_dir(Path::new(WATCH_HELD_FIXTURE), &project);
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg(CHANGED)
        .args([
            "edits/a-y.ts",
            "edits/a-y-2.ts",
            "edits/a-y-3.ts",
            "src/c.ts=edits/c-note.ts",
            "src/c.ts=edits/c-note.ts",
            "edits/a-y-4.ts",
        ])
        .args(["--", "--watch", "-p", "tsconfig.json", "--pretty"])
        .env_remove("GOPORT_FREE_FILE_VERSIONS")
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{stdout}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        normalize_watch_output(&read(&out)),
        read(&project.join("expected.txt")),
        "watch output against tsgo ({})",
        root.display()
    );

    let watched = read_tree(&project.join("out"));
    // The last texts, as the edits left them.
    write(
        &project.join(CHANGED),
        &read(&project.join("edits/a-y-4.ts")),
    );
    write(
        &project.join("src/c.ts"),
        &read(&project.join("edits/c-note.ts")),
    );
    fs::remove_dir_all(project.join("out")).expect("remove the watch out dir");
    // `--writeRoot`: the config's `outDir` is inside the project.
    let emit = Command::new(env!("CARGO_BIN_EXE_goport_emit"))
        .args([
            "-p",
            "tsconfig.json",
            "--outDir",
            "out",
            "--writeRoot",
            "out",
        ])
        .current_dir(&project)
        .output()
        .expect("run goport_emit");
    assert_eq!(
        emit.status.code(),
        Some(2),
        "goport_emit reports the c.ts error and writes the outputs ({}):\n{}",
        root.display(),
        String::from_utf8_lossy(&emit.stdout)
    );
    let fresh = read_tree(&project.join("out"));
    let outputs = |tree: BTreeMap<String, String>| -> BTreeMap<String, String> {
        tree.into_iter()
            .filter(|(name, _)| !name.ends_with(".tsbuildinfo"))
            .collect()
    };
    assert_eq!(
        outputs(watched),
        outputs(fresh),
        "watch outputs against goport_emit ({})",
        root.display()
    );

    let (made, dead) = file_version_counts(&stdout);
    assert_eq!(
        (made, dead),
        (6, 4),
        "a.ts v2 to v5 and c.ts v2 and v3 are versions; the last a.ts and c.ts live"
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// watchfree1: Go `tsc -b --watch` parses every file of a project that it
/// builds again in each cycle (`resetCaches`). Here a file keeps its parse
/// while it does not change (`BuildHost::watch_source_file`), and from the
/// second cycle on a new parse is a freeable file version. Four edits of
/// `core/src/a.ts` build `core` again each time, and parse only `a.ts`
/// again. The output (times removed) equals `expected.txt`, the output of
/// `tsgo-oracle-673a5f17d713 -b -w tsconfig.json --pretty false` for the
/// same edits, and the outputs of `core` equal its outputs
/// (`expected-out`). Each version of `a.ts` dies but the last: its parse is
/// kept, and the `c.ts` error of the last build points at it
/// (`BuildTask::held_file_versions`).
// PORT: no Go counterpart for the file versions; the output is Go's.
#[test]
fn watch_build_frees_file_versions() {
    let root = scratch_dir("watch-build");
    let project = root.join("project");
    copy_dir(Path::new(WATCH_BUILD_FIXTURE), &project);
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg("core/src/a.ts")
        .args([
            "edits/a-y.ts",
            "edits/a-2.ts",
            "edits/a-3.ts",
            "edits/a-4.ts",
        ])
        .args(["--", "-b", "--watch", "tsconfig.json", "--pretty", "false"])
        .env_remove("GOPORT_FREE_FILE_VERSIONS")
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{stdout}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        normalize_watch_output(&read(&out)),
        read(&project.join("expected.txt")),
        "watch output against tsgo ({})",
        root.display()
    );
    assert_eq!(
        read_tree(&project.join("core/out")),
        read_tree(&project.join("expected-out")),
        "core outputs against tsgo ({})",
        root.display()
    );
    let (made, dead) = file_version_counts(&stdout);
    assert_eq!(
        (made, dead),
        (4, 3),
        "each build after the first parses a.ts again; the last a.ts version lives"
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// watchcfg1: config edits that change only emit options, with
/// `--pretty`. `src/b.ts` (TS2741, with related information into
/// `src/a.ts`) and `src/c.ts` have errors; `src/a.ts` and `src/d.ts` have
/// none. The edits: `removeComments` on, a comment line added to `a.ts`
/// (the related information moves), `newLine: crlf` and `sourceMap` on, a
/// fix of `b.ts`, and the first config again. Go parses every file again
/// after a config edit (execute/watcher.go doBuild,
/// build/orchestrator.go resetCaches); the port keeps the parses of the
/// files with no errors (`WatchCompilerHost::reuse_parse`,
/// `BuildHost::keep_watch_sources_for_config_change`). The output (times
/// and clear-screen codes removed) equals `expected-w.txt`, the output of
/// `tsgo-oracle-673a5f17d713 -w -p tsconfig.json --pretty` for the same
/// edits. The outputs equal Go's (`expected-out`): the last edit emits
/// each file again, with its comments and LF line ends, and the source
/// maps of the crlf build stay. The `expected-out` source maps are from
/// `tsgo-oracle-fed0bf24149f` (pin N'), which omits an empty `sourceRoot`
/// (ts#64544); its watch output and other outputs are the same as pin N's.
// PORT: no Go counterpart; the output is Go's.
#[test]
fn watch_config_edits_of_emit_options_print_like_go() {
    watch_config_edits(
        "watch-config",
        &["--watch", "-p", "tsconfig.json", "--pretty"],
        "expected-w.txt",
    );
}

/// `watch_config_edits_of_emit_options_print_like_go` in `tsc -b --watch`:
/// the output equals `expected-bw.txt`, the output of
/// `tsgo-oracle-673a5f17d713 -b -w tsconfig.json --pretty` (a build prints
/// no error summary), and the outputs equal Go's.
// PORT: no Go counterpart; the output is Go's.
#[test]
fn build_watch_config_edits_of_emit_options_print_like_go() {
    watch_config_edits(
        "build-watch-config",
        &["-b", "--watch", "tsconfig.json", "--pretty"],
        "expected-bw.txt",
    );
}

/// bwsig1: Go `tsc -b --watch` keeps the `.d.ts` and `.json` parses of a
/// cycle until the cycle ends (build/host.go `GetSourceFile`,
/// build/orchestrator.go `resetCaches`). `side` has no reference to `lib`
/// but imports its output `lib/dist/a.d.ts`, so it builds beside `lib` and
/// reads that file before `lib` writes it. The edit turns on
/// `removeComments` in the base config: every project builds again, and
/// `lib` writes `a.d.ts` without its comment. `app` references `lib`,
/// builds after it and gets the parse that `side` made: its build info
/// keeps the old version of `a.d.ts` and no signature for `index.ts`.
/// `expected.txt` (the output up to the end of the second build) and
/// `expected-app.tsbuildinfo.json` are from `tsgo-oracle-673a5f17d713` for
/// the same steps (10 of 10 runs). At the bump D pin the build info is from
/// `tsgo-oracle-fed0bf24149f` (3 of 3 runs): only the versions of the changed
/// libs `lib.dom.d.ts` and `lib.es2022.array.d.ts` differ.
// PORT: no Go counterpart; the output is Go's.
#[test]
fn build_watch_keeps_the_first_dts_parse_of_a_cycle() {
    let root = scratch_dir("watch-stale-dts");
    // Go `CanWatchDirectory` does not watch `/tmp/<dir>/project`
    // (`watch_frees_file_versions`).
    let project = root.join("work").join("project");
    copy_dir(Path::new(WATCH_STALE_DTS_FIXTURE), &project);
    // The first build builds `side` before `lib` writes `a.d.ts` (TS2307,
    // as in Go); the second builds `side` again with it.
    for expect_success in [false, true] {
        let build = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(["-b", "tsconfig.json", "--pretty", "false"])
            .current_dir(&project)
            .output()
            .expect("run tsgo -b");
        assert_eq!(
            build.status.success(),
            expect_success,
            "tsgo -b in {}:\n{}",
            root.display(),
            String::from_utf8_lossy(&build.stdout)
        );
    }
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg("tsconfig.base.json")
        .arg("edits/tsconfig.base.json")
        .args(["--", "-b", "--watch", "tsconfig.json", "--pretty", "false"])
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        normalize_watch_output(&read(&out)),
        read(&project.join("expected.txt")),
        "watch output against tsgo ({})",
        root.display()
    );
    assert_eq!(
        read(&project.join("app/dist/tsconfig.tsbuildinfo")),
        read(&project.join("expected-app.tsbuildinfo.json")),
        "app build info against tsgo ({})",
        root.display()
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// Runs `goport_watch` with `tsc_args` on a copy of the `watch-config`
/// fixture through its edits, and checks the output against `expected` and
/// the outputs against `expected-out`.
fn watch_config_edits(test: &str, tsc_args: &[&str], expected: &str) {
    let root = scratch_dir(test);
    // Go `CanWatchDirectory` does not watch `/tmp/<dir>/project`, so a
    // config edit would start no build there (`watch_frees_file_versions`).
    let project = root.join("work").join("project");
    copy_dir(Path::new(WATCH_CONFIG_FIXTURE), &project);
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(norm(&out))
        .arg("src/a.ts")
        .args([
            "tsconfig.json=edits/tsconfig-comments.json",
            "src/a.ts=edits/a-2.ts",
            "tsconfig.json=edits/tsconfig-crlf.json",
            "src/b.ts=edits/b-2.ts",
            "tsconfig.json=edits/tsconfig.json",
        ])
        .arg("--")
        .args(tsc_args)
        .env_remove("GOPORT_FREE_FILE_VERSIONS")
        .current_dir(&project)
        .output()
        .expect("run goport_watch");
    assert!(
        run.status.success(),
        "goport_watch failed ({}) in {}:\n{}\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        normalize_watch_output(&read(&out)),
        read(&project.join(expected)),
        "watch output against tsgo ({})",
        root.display()
    );
    assert_eq!(
        read_tree(&project.join("out")),
        read_tree(&project.join("expected-out")),
        "outputs against tsgo ({})",
        root.display()
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// The counts of the last stdout line of `goport_watch`,
/// `file_versions made=<n> dead=<m>`.
fn file_version_counts(stdout: &str) -> (usize, usize) {
    stdout
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("file_versions made="))
        .and_then(|rest| rest.split_once(" dead="))
        .and_then(|(made, dead)| Some((made.parse().ok()?, dead.parse().ok()?)))
        .unwrap_or_else(|| panic!("no file_versions line in the goport_watch output:\n{stdout}"))
}

/// A watch output without the clear-screen codes, and with each status
/// time as `<time>`: `[<esc>[90m3:41:35 PM<esc>[0m] ` (`--pretty`) as
/// `[<time>] `, and `03:41:35 PM - ` as `<time> - `.
fn normalize_watch_output(output: &str) -> String {
    const TIME_START: &str = "[\x1b[90m";
    const TIME_END: &str = "\x1b[0m] ";
    let is_time = |text: &str| {
        text.strip_suffix(" AM")
            .or_else(|| text.strip_suffix(" PM"))
            .is_some_and(|time| {
                !time.is_empty() && time.bytes().all(|b| b.is_ascii_digit() || b == b':')
            })
    };
    let output = output.replace("\x1b[2J\x1b[3J\x1b[H", "");
    let mut normalized = String::with_capacity(output.len());
    for line in output.split_inclusive('\n') {
        if let Some((_, rest)) = line
            .strip_prefix(TIME_START)
            .and_then(|rest| rest.split_once(TIME_END))
        {
            normalized.push_str("[<time>] ");
            normalized.push_str(rest);
        } else if let Some((_, rest)) = line.split_once(" - ").filter(|(time, _)| is_time(time)) {
            normalized.push_str("<time> - ");
            normalized.push_str(rest);
        } else {
            normalized.push_str(line);
        }
    }
    normalized
}

/// `goport_build -b` makes the program of each project in one process, and
/// the build host shares parsed `.d.ts` files between them. `p1` imports
/// `a` and `b`, so the copy of `a` under `b/node_modules` is a duplicate
/// package: `p1` parses it and its `dep.d.ts` and leaves both out. `p2`
/// imports only `b`, so both are program files of `p2`. `expected.txt` is
/// the pinned tsgo output of the same command.
#[test]
fn build_includes_files_that_an_earlier_project_left_out() {
    let root = scratch_dir("build-dedup");
    copy_dir(Path::new(BUILD_DEDUP_FIXTURE), &root);
    let run = Command::new(env!("CARGO_BIN_EXE_goport_build"))
        .args(["-b", "tsconfig.json", "--explainFiles", "--pretty", "false"])
        .current_dir(&root)
        .output()
        .expect("run goport_build");
    let report = Report {
        stdout: String::from_utf8(run.stdout).expect("goport_build stdout is UTF-8"),
        status: run.status.code().expect("goport_build exited with a code"),
    };
    let expected = Report {
        stdout: read(&root.join("expected.txt")),
        status: 0,
    };
    assert_eq!(
        report,
        expected,
        "goport_build against tsgo ({}):\n{}",
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    for project in ["p1", "p2"] {
        let build_info = root.join(project).join("tsconfig.tsbuildinfo");
        assert!(build_info.is_file(), "no {}", build_info.display());
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// A script that uses many `lib.es5.d.ts` and `lib.dom.d.ts` symbols and
/// merges with two lib interfaces. Go N (`tsgo-oracle-673a5f17d713`) gives
/// the two errors that the test checks.
const SERIAL_BIND_SCRIPT: &str = r#"interface Array<T> { lastItem(): T | undefined; }
interface Window { appName: string; }
const div: HTMLDivElement = document.createElement("div");
const tag: number = div.tagName;
const doubled = [1, 2, 3].map((n) => n * 2).filter((n) => n > 2);
const last = doubled.lastItem();
const joined = ["a", "b"].join("-").toUpperCase().split("-");
const parsed: { a: number } = JSON.parse('{"a": 1}');
const keys = Object.keys(parsed).concat(Object.getOwnPropertyNames(Math));
const when = new Date(Date.now()).toISOString();
const found = /x+/g.exec("xxy");
const failure = new RangeError("bad");
const rounded = Math.round(Math.PI * Number.MAX_VALUE);
const bound = Function.prototype.bind.call(parseInt, null, "10");
window.addEventListener("click", (e) => e.clientX + window.appName.length);
let wrong: string = rounded;
"#;

/// followups31 (R177 reviewer item 2): the remap path of apisym1c. A
/// serial bind (`GOPORT_BIND_THREADS=1`) binds each file into the binder
/// lineage. Its first file, `lib.es5.d.ts`, loads its lib bind snapshot into
/// the new lineage arena, so the snapshot joins as a file arena and its ids
/// move (`lib_snapshot::load`, `BoundFile::remap`). The types, symbols and
/// diagnostics must equal those of the parallel bind (the default on more
/// than one core), where each file binds into an arena of its own, and
/// those of a serial live bind (`GOPORT_LIB_SNAPSHOT=0`).
#[test]
fn a_serial_bind_with_a_lib_snapshot_in_the_new_lineage_equals_the_parallel_bind() {
    let dir = scratch_dir("serial-bind");
    write(
        &dir.join("tsconfig.json"),
        r#"{ "compilerOptions": { "lib": ["dom", "es5"], "types": [], "strict": true, "noEmit": true }, "files": ["a.ts"] }"#,
    );
    write(&dir.join("a.ts"), SERIAL_BIND_SCRIPT);
    // (bind threads, GOPORT_LIB_SNAPSHOT): the serial bind traces its
    // snapshot loads.
    let sides = [
        ("serial", "1", "trace"),
        ("parallel", "4", "1"),
        ("live", "1", "0"),
    ];
    let runs: Vec<(BTreeMap<String, String>, Report, String)> = sides
        .iter()
        .map(|&(side, threads, snapshot)| {
            let env = [
                ("GOPORT_BIND_THREADS", threads),
                ("GOPORT_LIB_SNAPSHOT", snapshot),
            ];
            let out = dir.join(side);
            let typesyms = Command::new(env!("CARGO_BIN_EXE_goport_typesyms"))
                .args(["-p", "tsconfig.json", "-o"])
                .arg(norm(&out))
                .envs(env)
                .current_dir(&dir)
                .output()
                .expect("run goport_typesyms");
            let stderr = String::from_utf8_lossy(&typesyms.stderr).into_owned();
            assert!(
                typesyms.status.success(),
                "{side} goport_typesyms: {stderr}"
            );
            let check = Command::new(env!("CARGO_BIN_EXE_tsgo"))
                .args(["-p", "tsconfig.json", "--listFiles", "--pretty", "false"])
                .envs(env)
                .current_dir(&dir)
                .output()
                .expect("run tsgo");
            let check = Report {
                stdout: String::from_utf8(check.stdout).expect("tsgo stdout is UTF-8"),
                status: check.status.code().expect("tsgo exited with a code"),
            };
            (read_tree(&out), check, stderr)
        })
        .collect();
    let (types, check, stderr) = &runs[0];
    assert!(
        stderr.contains("goport lib snapshot: lib.es5.d.ts loaded"),
        "the serial bind loads the snapshot of its first file: {stderr}"
    );
    // `--listFiles` lists the files after the errors.
    assert!(
        check
            .stdout
            .lines()
            .find(|line| !line.contains("): error TS"))
            .is_some_and(|line| line.ends_with("/lib.es5.d.ts")),
        "lib.es5.d.ts is the first file: {}",
        check.stdout
    );
    assert_eq!(
        error_lines(&check.stdout),
        [
            "a.ts(4,7): error TS2322: Type 'string' is not assignable to type 'number'.",
            "a.ts(16,5): error TS2322: Type 'number' is not assignable to type 'string'.",
        ]
    );
    assert_eq!(check.status, 2);
    for (side, (other_types, other_check, _)) in sides.iter().zip(&runs).skip(1) {
        assert_eq!(other_types, types, "{} types and symbols", side.0);
        assert_eq!(other_check, check, "{} tsgo", side.0);
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The diagnostic lines of a tsc report.
fn error_lines(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter(|line| line.contains("): error TS"))
        .collect()
}

/// Runs `goport_live_programs` with `cwd` as the current directory and
/// fails the test when it fails.
fn run_live_programs(cwd: &Path, args: &[&str]) {
    let run = Command::new(env!("CARGO_BIN_EXE_goport_live_programs"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run goport_live_programs");
    assert!(
        run.status.success(),
        "goport_live_programs {args:?} failed ({}) in {}:\n{}",
        run.status,
        cwd.display(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Runs a fresh `goport_emit -p <config> --outDir <out_dir>` in `cwd`.
// Only the Unix-only test `three_live_projects_report_like_fresh_runs` uses this.
#[cfg(unix)]
fn goport_emit(cwd: &Path, config: &str, out_dir: &str) -> Report {
    let run = Command::new(env!("CARGO_BIN_EXE_goport_emit"))
        .args(["-p", config, "--outDir", out_dir])
        .current_dir(cwd)
        .output()
        .expect("run goport_emit");
    let status = run.status.code().expect("goport_emit exited with a code");
    assert_ne!(
        status,
        EXIT_UNPORTED,
        "goport_emit hit unported code in {}:\n{}",
        cwd.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    Report {
        stdout: String::from_utf8(run.stdout).expect("goport_emit stdout is UTF-8"),
        status,
    }
}

/// Reads `report.txt` and `status` of one `live` program dir.
fn read_live_report(dir: &Path) -> Report {
    let status = read(&dir.join("status"));
    Report {
        stdout: read(&dir.join("report.txt")),
        status: status
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{} must hold an exit code", dir.display())),
    }
}

/// The files under `dir` by path relative to `dir`, with their text.
fn read_tree(dir: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).unwrap_or_else(|error| {
            panic!("read {}: {error}", current.display());
        }) {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .expect("a path under the dir")
                    .to_string_lossy()
                    .into_owned();
                files.insert(name, read(&path));
            }
        }
    }
    files
}

/// Runs `pair` on a new copy of the fixture and checks each report against a
/// fresh `goport`. `edit` is the fixture file whose text replaces `src/a.ts`.
/// `None` writes the original text again. With `with_first`, `pair` first
/// loads, reports and releases a second copy, so A and B are not the first
/// program of the process.
fn check_pair(test: &str, edit: Option<&str>, with_first: bool) -> Pair {
    check_pair_with_env(test, edit, with_first, &[])
}

/// `check_pair` with `env` set for `goport_multiprog` (not for the fresh
/// `goport` runs).
fn check_pair_with_env(
    test: &str,
    edit: Option<&str>,
    with_first: bool,
    env: &[(&str, &str)],
) -> Pair {
    let root = scratch_dir(test);
    let project = root.join("project");
    copy_dir(Path::new(FIXTURE), &project);
    let changed = project.join(CHANGED);
    let original = read(&changed);
    let new_text = edit.map_or_else(|| original.clone(), |edit| read(&project.join(edit)));
    let new_text_file = root.join("new-text.ts");
    write(&new_text_file, &new_text);
    let out = root.join("out");
    fs::create_dir(&out).unwrap_or_else(|error| panic!("create {}: {error}", out.display()));

    let fresh_a = goport(&project, Path::new("tsconfig.json"));

    let mut args: Vec<String> = vec![
        "pair".into(),
        "tsconfig.json".into(),
        norm(&changed),
        norm(&new_text_file),
        norm(&out),
    ];
    let first_config = with_first.then(|| {
        let other = root.join("other");
        copy_dir(Path::new(FIXTURE), &other);
        other.join("tsconfig.json")
    });
    if let Some(config) = &first_config {
        args.push("--first".into());
        args.push(norm(config));
    }
    let run = Command::new(env!("CARGO_BIN_EXE_goport_multiprog"))
        .args(&args)
        .envs(env.iter().copied())
        .current_dir(&project)
        .output()
        .expect("run goport_multiprog");
    assert!(
        run.status.success(),
        "goport_multiprog pair failed ({}) in {}:\n{}",
        run.status,
        root.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(read(&changed), original, "pair must restore {CHANGED}");

    let a = read_report(&out, "a");
    assert_eq!(
        a,
        fresh_a,
        "a.txt against goport on the original ({})",
        root.display()
    );

    write(&changed, &new_text);
    let fresh_b = goport(&project, Path::new("tsconfig.json"));
    let b = read_report(&out, "b");
    assert_eq!(
        b,
        fresh_b,
        "b.txt against goport on the edited text ({})",
        root.display()
    );

    if let Some(config) = &first_config {
        assert_eq!(
            read(&out.join("first.txt")),
            goport(&project, config).stdout,
            "first.txt against goport on the other copy ({})",
            root.display()
        );
    }

    let reused = match read(&out.join("b.reused")).trim() {
        "true" => true,
        "false" => false,
        other => panic!("b.reused must be true or false, not {other:?}"),
    };
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
    Pair { a, b, reused }
}

/// Both edits make `Point.y` a string, so the unchanged `c.ts` has an error
/// in B only. This makes sure the edit is visible in the reports.
fn assert_new_c_error(pair: &Pair) {
    assert!(
        !pair.a.stdout.contains("src/c.ts("),
        "c.ts must have no error before the edit:\n{}",
        pair.a.stdout
    );
    assert!(
        pair.b.stdout.contains("src/c.ts("),
        "c.ts must have an error after the edit:\n{}",
        pair.b.stdout
    );
}

/// Runs a fresh `goport -p <config>` with `cwd` as the current directory.
fn goport(cwd: &Path, config: &Path) -> Report {
    let run = Command::new(env!("CARGO_BIN_EXE_goport"))
        .arg("-p")
        .arg(norm(config))
        .current_dir(cwd)
        .output()
        .expect("run goport");
    let status = run.status.code().expect("goport exited with a code");
    assert_ne!(
        status,
        EXIT_UNPORTED,
        "goport hit unported code in {}:\n{}",
        cwd.display(),
        String::from_utf8_lossy(&run.stderr)
    );
    Report {
        stdout: String::from_utf8(run.stdout).expect("goport stdout is UTF-8"),
        status,
    }
}

/// Reads `<name>.txt` and `<name>.status` from the `pair` out dir.
fn read_report(out: &Path, name: &str) -> Report {
    let status = read(&out.join(format!("{name}.status")));
    Report {
        stdout: read(&out.join(format!("{name}.txt"))),
        status: status
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}.status must hold an exit code, not {status:?}")),
    }
}

/// Makes a new empty directory for one test run.
fn scratch_dir(test: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "goport-multiprog-{test}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
    // The programs see the real current directory, so the paths given to
    // `pair` must use the real path too.
    let real = fs::canonicalize(&dir).expect("canonical scratch dir");
    // On Windows the real path is verbatim (`\\?\C:\...`), which the programs
    // do not take as a cwd or in an argument.
    #[cfg(windows)]
    let real = PathBuf::from(unverbatim(&real.to_string_lossy()));
    real
}

/// `text` without the Windows verbatim prefix: `\\?\C:\x` is `C:\x`, and
/// `\\?\UNC\server\share\x` is `\\server\share\x` (the root stays).
#[cfg(windows)]
fn unverbatim(text: &str) -> String {
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(text).to_owned()
    }
}

/// `path` as the programs name it: absolute with `/` separators
/// (`C:/Users/x/y` on Windows). The identity on Unix.
fn norm(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let text = unverbatim(&text).replace('\\', "/");
    text
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap_or_else(|error| panic!("create {}: {error}", to.display()));
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target)
                .unwrap_or_else(|error| panic!("copy to {}: {error}", target.display()));
        }
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}
