//! How `tsc -b --watch` keeps the parses of a cycle, against Go.
//!
//! Go keeps each `.d.ts` and `.json` parse of a cycle in the build host
//! cache `sourceFiles` until the cycle ends (build/host.go `GetSourceFile`,
//! build/orchestrator.go `resetCaches`). A project that builds beside an
//! upstream project (no reference to it) can parse an upstream output
//! before that build writes it. Every later program of the cycle then gets
//! that parse. The `.d.ts` case is
//! `multi_program::build_watch_keeps_the_first_dts_parse_of_a_cycle`.
//!
//! Each test copies its fixture to a new directory under the system temp
//! dir and runs `tsgo` and `goport_watch` there. A passing test deletes the
//! directory. A failing test keeps it and names it in the message.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const STALE_JSON_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/build_watch/stale-json"
);

/// bwsig2: the `.json` half of bwsig1. `lib` is composite with
/// `resolveJsonModule` and writes `data.json` to its `outDir`. `side` has no
/// reference to `lib` but imports `lib/dist/data.json`, so it builds beside
/// `lib` and reads that file before `lib` writes it. The edit sets
/// `"newLine": "crlf"` in the base config: every project builds again, and
/// `lib` writes `data.json` with CRLF line ends. `app` references `lib`,
/// imports the same file, builds after `lib` and gets the parse that `side`
/// made: its build info keeps the LF version of `data.json`. `expected.txt`
/// (the output up to the end of the second build) and
/// `expected-app.tsbuildinfo.json` are from `tsgo-oracle-673a5f17d713` for
/// the same steps (11 of 11 runs).
// PORT: no Go counterpart; the output is Go's.
#[test]
fn build_watch_keeps_the_first_json_parse_of_a_cycle() {
    let root = scratch_dir("stale-json");
    // Go `CanWatchDirectory` does not watch `/tmp/<dir>/project`, so the
    // project is one level deeper.
    let project = root.join("work").join("project");
    copy_dir(Path::new(STALE_JSON_FIXTURE), &project);
    // The first build builds `side` and `app` with no `lib/dist/data.json`
    // (TS2307, as in Go: `side` looks it up first, and the cached lookup
    // stays for the build); the second builds them with it.
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
        .arg(&out)
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
    assert!(
        read(&project.join("lib/dist/data.json")).contains("\r\n"),
        "lib wrote data.json with CRLF ({})",
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

/// A `--pretty false` watch output without the clear-screen codes, and with
/// each status time as `<time>`: `03:41:35 PM - ` becomes `<time> - `.
fn normalize_watch_output(output: &str) -> String {
    let is_time = |text: &str| {
        text.strip_suffix(" AM")
            .or_else(|| text.strip_suffix(" PM"))
            .is_some_and(|time| {
                !time.is_empty() && time.bytes().all(|b| b.is_ascii_digit() || b == b':')
            })
    };
    output
        .replace("\x1b[2J\x1b[3J\x1b[H", "")
        .split_inclusive('\n')
        .map(|line| match line.split_once(" - ") {
            Some((time, rest)) if is_time(time) => format!("<time> - {rest}"),
            _ => line.to_string(),
        })
        .collect()
}

/// A new directory under the system temp dir, as its real path: `tsgo`
/// sees the real current directory, and `goport_watch` sends its events
/// for paths under it. A Windows real path is verbatim (`\\?\C:\...`),
/// which a program does not take as a cwd, so it stays as made there.
fn scratch_dir(test: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "goport-build-watch-{test}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
    if cfg!(windows) {
        return dir;
    }
    fs::canonicalize(&dir).expect("canonical scratch dir")
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
