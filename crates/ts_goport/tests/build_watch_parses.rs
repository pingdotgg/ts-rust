//! How `tsc -b --watch` keeps the parses of a cycle, against Go.
//!
//! Go keeps each `.d.ts` and `.json` parse of a cycle in the build host
//! cache `sourceFiles` until the cycle ends (build/host.go `GetSourceFile`,
//! build/orchestrator.go `resetCaches`). A project that builds beside an
//! upstream project (no reference to it) can parse an upstream output
//! before that build writes it. Every later program of the cycle then gets
//! that parse. The `.d.ts` case is
//! `multi_program::build_watch_keeps_the_first_dts_parse_of_a_cycle`.
//! The port keeps a parse that Go makes again, so the `shared-ts-bind-order`
//! tests check that the symbols of such a parse get Go's new ids.
//!
//! Each test copies its fixture to a new directory and runs `tsgo` there,
//! and `goport_watch` or `tsgo -b --watch`. The directory is under the
//! system temp dir, or under `/tmp` for
//! `build_watch_parses_a_written_dts_again_in_the_next_cycle` (see there). A
//! passing test deletes the directory. A failing test keeps it and names it
//! in the message.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const STALE_JSON_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/build_watch/stale-json"
);

#[cfg(target_os = "linux")]
const COARSE_MTIME_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/build_watch/coarse-mtime"
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
/// `expected-app.tsbuildinfo.json` are from `tsgo-oracle-fed0bf24149f` for
/// the same steps (3 of 3 runs). At that pin only the `lib.dom.d.ts` and
/// `lib.es2022.array.d.ts` versions differ from `tsgo-oracle-673a5f17d713`
/// (16 of 16 runs): Go changed those two libs (#64604, #64536). At the bump
/// E pin the build info is from `tsgo-oracle-3e9236361fa9` (3 of 3 runs):
/// only the version (`7.1.0-beta`) and the versions of the changed libs
/// `lib.decorators.d.ts` and `lib.es2023.intl.d.ts` differ.
// PORT: no Go counterpart; the output is Go's.
#[test]
fn build_watch_keeps_the_first_json_parse_of_a_cycle() {
    let root = scratch_dir("stale-json");
    // The project is one level deeper, as in the Go runs of `expected.txt`.
    // Go 673a5f17d713 did not watch `/tmp/<dir>/project` (`CanWatchDirectory`
    // watched no directory 3 or fewer levels below `/`). Since #64366 Go
    // watches the directory of a config or program file at any depth.
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

/// The bwsig2 skeptic case: a written `.d.ts` whose time goes back (as on a
/// coarse clock), with no watch event that builds the project that reads
/// it. `lib` writes `a.d.ts` to `out`, beside the project. `side` imports
/// `out/a.d.ts`, has no reference to `lib` and is not incremental
/// (`"composite": false`). The steps, with the OS watcher, and the number
/// of builds that ended after each:
///
/// 1. The start: 1.
/// 2. A `side` edit: `side` builds and parses `out/a.d.ts` (`a: number`): 2.
/// 3. A `lib` edit (`a: string`): `lib` builds and writes `out/a.d.ts`.
///    `side` does not build: 3.
/// 4. The test sets both times of each `out/*.d.ts` back to the time that
///    step 2 saw (it set them before the watch started too): no build, 3.
/// 5. A `side` edit: `side` builds. Go reads the new `a.d.ts`, because
///    `resetCaches` drops every parse at the end of a cycle
///    (execute/build/orchestrator.go:506-513, called at :862): TS2322, 4.
///
/// Why `side` is not composite: a non-incremental build info has no file
/// names (execute/incremental/snapshottobuildinfo.go:37, :51). A composite
/// `side` would list `out/a.d.ts` there. Then Go watches `out` at any depth
/// (`addProgramFileWatchDir`, execute/build/orchestrator.go:746-752,
/// :775-781; #64366), and the event for `out/a.d.ts` builds `side` in step
/// 3 (:606-614). So step 5 would not show the drop below.
///
/// With no file names, an event for `out/a.d.ts` builds no task: it is no
/// root file, build info file name or package.json lookup of a task
/// (:586-638). An event for a directory under a watch would build every
/// task (:653-666), but every event here is for a file. `out` gets a watch
/// only from the package.json lookups of `side` (:761, `addWatchDir`), and
/// `CanWatchDirectory` refuses a directory 3 or fewer levels below `/`
/// (:769-773, execute/watchmanager/watchbackend.go:84-92). Lines are at Go
/// fed0bf24149f. Go 673a5f17d713 does the same for a non-composite `side`.
///
/// The port keeps a `.d.ts` parse from one cycle to the next and checks it
/// by its modification time. The end of `build_all_tasks` drops the kept
/// parse of each written file (`BuildHost::drop_written_watch_sources`).
/// Without that call the port keeps the parse of step 2 and finds no
/// error: this test checks that call. A watch event drops a kept parse too
/// (`BuildHost::evict_watch_sources`), so the check needs an `out` with no
/// watch. Why the root is `/tmp/<dir>`: there `out` is 3 levels below `/`,
/// and it gets no watch. Under the system temp dir it can be deeper and get
/// a watch. Go gives the same output at any depth, so where `/tmp` is
/// deeper the test still runs and passes, but it does not check the drop.
/// `expected.txt` and the counts are from
/// `tsgo-oracle-fed0bf24149f` and `tsgo-oracle-673a5f17d713` in place of
/// `tsgo` for this test (3 of 3 runs each).
// PORT: no Go counterpart; the output is Go's.
#[cfg(target_os = "linux")]
#[test]
fn build_watch_parses_a_written_dts_again_in_the_next_cycle() {
    use std::time::Duration;
    // `out` must be 3 or fewer levels below `/` (see above).
    let root = scratch_dir_in(Path::new("/tmp"), "coarse-mtime");
    // As in the first test, the project is one level deeper. `lib` writes
    // to `<root>/out`, outside the project.
    let project = root.join("work").join("project");
    copy_dir(Path::new(COARSE_MTIME_FIXTURE), &project);
    let out_dir = root.join("out");
    let old = SystemTime::now() - Duration::from_secs(1000);
    let set_dts_times_back = || {
        for entry in fs::read_dir(&out_dir).expect("read out") {
            let path = entry.expect("out entry").path();
            if path.to_string_lossy().ends_with(".d.ts") {
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .and_then(|file| {
                        file.set_times(fs::FileTimes::new().set_accessed(old).set_modified(old))
                    })
                    .unwrap_or_else(|error| panic!("set the time of {}: {error}", path.display()));
            }
        }
    };
    // The first build can build `side` before `lib` writes `out/a.d.ts`
    // (TS2307). The second builds each project with it.
    for _ in 0..2 {
        let build = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(["-b", "tsconfig.json", "--pretty", "false"])
            .env("GOPORT_LAUNCH", "0")
            .current_dir(&project)
            .output()
            .expect("run tsgo -b");
        if build.status.success() {
            break;
        }
    }
    set_dts_times_back();
    let watch = os_watch::Run::start(&project);
    watch.expect_builds(1, "the start", &root);
    let edit = |target: &str, source: &str| os_watch::replace(&root, &project, target, source);
    edit("side/index.ts", "edits/side-index-1.ts");
    watch.expect_builds(2, "the first side edit", &root);
    edit("lib/a.ts", "edits/lib-a.ts");
    watch.expect_builds(3, "the lib edit", &root);
    set_dts_times_back();
    std::thread::sleep(os_watch::QUIET);
    watch.expect_builds(3, "the time change", &root);
    edit("side/index.ts", "edits/side-index-2.ts");
    watch.expect_builds(4, "the second side edit", &root);
    let (stdout, stderr) = watch.stop();
    assert_eq!(
        normalize_watch_output(&stdout),
        read(&project.join("expected.txt")),
        "watch output against tsgo ({})",
        root.display()
    );
    assert_eq!(stderr, "", "stderr ({})", root.display());
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// watchreset1r round 3 (skeptic B3). p1 and p2 both have `s.ts`; p1 also
/// has a syntax error. Go parses `s.ts` in each program of each cycle
/// (build/host.go:50-56 `GetSourceFile`), and each bind gives the new class
/// `S` (with a private name) a new id. The port keeps one parse; in default
/// mode p2 binds it before p1, which binds only at its emit or its
/// statistics. Each program's bind must still give `S` new ids
/// (`program::note_go_parse`, `program::bind_all`). `goport_watch` runs the
/// default schedule (no test schedule). The edits: `s.ts`, `s.ts`,
/// `p2/b.ts` (p1 does not build), `s.ts`.
const SHARED_TS_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/build_watch/shared-ts-bind-order"
);

/// One build of a `tsc -b --watch` output: the private name ids
/// (`#<id>@#<name>`, sorted) and the `--diagnostics` "Symbols" counts (in
/// output order).
#[derive(Debug, PartialEq)]
struct BuildIds {
    ids: Vec<u64>,
    symbols: Vec<u64>,
}

/// Runs `goport_watch` with `tsc -b --watch --pretty false` plus `extra` on
/// a copy of `shared-ts-bind-order` (p1 with `noEmitOnError` when
/// `no_emit_on_error`), and gives each build's `BuildIds` with the scratch
/// dir, which the caller removes.
fn shared_ts_builds(
    test: &str,
    no_emit_on_error: bool,
    extra: &[&str],
) -> (PathBuf, Vec<BuildIds>) {
    let root = scratch_dir(test);
    let project = root.join("project");
    copy_dir(Path::new(SHARED_TS_FIXTURE), &project);
    if no_emit_on_error {
        let config = project.join("p1/tsconfig.json");
        let text = read(&config).replacen(
            r#""strict": true,"#,
            r#""strict": true, "noEmitOnError": true,"#,
            1,
        );
        fs::write(&config, text).expect("write p1/tsconfig.json");
    }
    let out = root.join("watch.txt");
    let run = Command::new(env!("CARGO_BIN_EXE_goport_watch"))
        .arg(&out)
        .args(["s.ts", "edits/s1.ts", "edits/s2.ts", "p2/b.ts=edits/b3.ts"])
        .arg("edits/s4.ts")
        .args(["--", "-b", "--watch", "tsconfig.json", "--pretty", "false"])
        .args(extra)
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
    let output = read(&out);
    let mut builds: Vec<BuildIds> = Vec::new();
    for line in output.lines() {
        if line.contains("Starting compilation") || line.contains("File change detected") {
            builds.push(BuildIds {
                ids: Vec::new(),
                symbols: Vec::new(),
            });
        }
        let Some(build) = builds.last_mut() else {
            continue;
        };
        if let Some(count) = line.strip_prefix("Symbols:") {
            build
                .symbols
                .push(count.trim().parse().expect("Symbols count"));
        }
        for (at, _) in line.match_indices("@#") {
            let digits = line[..at]
                .bytes()
                .rev()
                .take_while(u8::is_ascii_digit)
                .count();
            if digits > 0 && line[..at - digits].ends_with('#') {
                build
                    .ids
                    .push(line[at - digits..at].parse().expect("private name id"));
            }
        }
    }
    for build in &mut builds {
        build.ids.sort_unstable();
    }
    (root, builds)
}

/// The `nbemit` case: p1 emits in spite of its syntax error, so it binds at
/// its emit. Go's ids race between its builders: in 16 runs of
/// `tsgo-oracle-3e9236361fa9 -b -w --pretty false`
/// (after231/watchreset1r2/skep/logs/nbemit.log,
/// after231/watchreset1r3/logs/nbemit-go.log) each build's two ids (`B` and
/// p2's `S`) are in the ranges below, and the fourth build (p2 only) is
/// `#115@#b #116@#s` in every run. Round 2 lost one id in each cycle
/// (`#112@#b #113@#s` there), which no Go run can give.
// PORT: no Go counterpart; the ids are Go's.
#[test]
fn build_watch_gives_new_ids_to_a_shared_ts_file_in_each_program_in_any_bind_order() {
    let (root, builds) = shared_ts_builds("shared-ts-emit", false, &[]);
    let ranges = [1..=4, 39..=42, 77..=80, 115..=116, 151..=154];
    assert_eq!(builds.len(), ranges.len(), "builds ({})", root.display());
    for (build, (ids, range)) in builds
        .iter()
        .map(|build| &build.ids)
        .zip(ranges)
        .enumerate()
    {
        assert!(
            ids.len() == 2 && ids[0] != ids[1] && ids.iter().all(|id| range.contains(id)),
            "build {}: ids {ids:?}, Go's are two in {range:?} ({})",
            build + 1,
            root.display()
        );
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// The `nobind` case with `--diagnostics`: p1 has `noEmitOnError`, so it
/// binds only at its statistics, after p2 bound `s.ts`. Go's own parse of
/// `s.ts` in p1 is not bound then, so p1's "Symbols" (the first count of a
/// build that builds p1) has none of its symbols. The ids and counts are
/// those of `tsgo-oracle-3e9236361fa9 -b -w --pretty false --diagnostics`
/// in 11 of 11 runs (after231/watchreset1r2/skep/logs, `nobindD` and
/// `nobind2D`; after231/watchreset1r3/logs/nobindD-go.log; one run gives
/// the two ids of the second build to the other classes).
// PORT: no Go counterpart; the ids and counts are Go's.
#[test]
fn build_watch_counts_no_symbols_of_a_shared_ts_file_that_another_program_bound() {
    let (root, builds) = shared_ts_builds("shared-ts-diagnostics", true, &["--diagnostics"]);
    let build = |ids: [u64; 2], symbols: &[u64]| BuildIds {
        ids: ids.to_vec(),
        symbols: symbols.to_vec(),
    };
    assert_eq!(
        builds,
        [
            build([1, 2], &[6_385, 10_600]),
            build([39, 40], &[6_385, 10_601]),
            build([77, 78], &[6_385, 10_601]),
            build([115, 116], &[10_602]),
            build([151, 152], &[6_385, 10_602]),
        ],
        "builds against tsgo ({})",
        root.display()
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// A `tsgo -b --watch` run with the OS watcher, for
/// `build_watch_parses_a_written_dts_again_in_the_next_cycle`.
#[cfg(target_os = "linux")]
mod os_watch {
    use std::io::Read;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    /// The text that ends each watch build.
    const BUILD_END: &str = "Watching for file changes";

    /// How long one build may take.
    const BUILD_LIMIT: Duration = Duration::from_mins(3);

    /// How long the output stays the same before a build counts as done.
    pub const QUIET: Duration = Duration::from_millis(1500);

    /// The output so far, and the time of its last change.
    type Output = Arc<Mutex<(String, Instant)>>;

    /// Drop kills a run that still runs.
    pub struct Run {
        child: Child,
        stdout: Output,
        stderr: Output,
        readers: Vec<JoinHandle<()>>,
    }

    impl Run {
        pub fn start(project: &Path) -> Run {
            let mut child = Command::new(env!("CARGO_BIN_EXE_tsgo"))
                .args(["-b", "--watch", "tsconfig.json", "--pretty", "false"])
                .env("GOPORT_LAUNCH", "0")
                .current_dir(project)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start tsgo -b --watch");
            let stdout: Output = Arc::new(Mutex::new((String::new(), Instant::now())));
            let stderr: Output = Arc::new(Mutex::new((String::new(), Instant::now())));
            let readers = vec![
                read_into(child.stdout.take().expect("stdout"), stdout.clone()),
                read_into(child.stderr.take().expect("stderr"), stderr.clone()),
            ];
            Run {
                child,
                stdout,
                stderr,
                readers,
            }
        }

        /// Waits until `builds` builds ended, the output ends with the end
        /// of a build, and nothing more came for `QUIET`.
        pub fn settle(&self, builds: usize, root: &Path) {
            let end = Instant::now() + BUILD_LIMIT;
            loop {
                {
                    let output = self.stdout.lock().unwrap();
                    let last_line = output.0.lines().rev().find(|line| !line.trim().is_empty());
                    if output.0.matches(BUILD_END).count() >= builds
                        && last_line.is_some_and(|line| line.contains(BUILD_END))
                        && output.1.elapsed() >= QUIET
                    {
                        return;
                    }
                    assert!(
                        Instant::now() < end,
                        "build {builds} did not end ({}):\n{}",
                        root.display(),
                        output.0
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// `settle(builds)`, then checks that no more builds ended.
        pub fn expect_builds(&self, builds: usize, step: &str, root: &Path) {
            self.settle(builds, root);
            let output = self.stdout.lock().unwrap();
            assert_eq!(
                output.0.matches(BUILD_END).count(),
                builds,
                "builds after {step} ({}):\n{}",
                root.display(),
                output.0
            );
        }

        /// Kills the run and gives its whole stdout and stderr.
        pub fn stop(mut self) -> (String, String) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            for reader in self.readers.drain(..) {
                reader.join().expect("reader thread");
            }
            let stdout = self.stdout.lock().unwrap().0.clone();
            let stderr = self.stderr.lock().unwrap().0.clone();
            (stdout, stderr)
        }
    }

    impl Drop for Run {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// A thread that appends what `from` gives to `into` until its end.
    fn read_into(mut from: impl Read + Send + 'static, into: Output) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let mut buf = [0; 8192];
            while let Ok(n) = from.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut output = into.lock().unwrap();
                output.0.push_str(&String::from_utf8_lossy(&buf[..n]));
                output.1 = Instant::now();
            }
        })
    }

    /// Puts the text of the project file `source` in place of `target`, as
    /// an editor saves: a new file under `root`, renamed over the target.
    pub fn replace(root: &Path, project: &Path, target: &str, source: &str) {
        let staged = root.join("edit.tmp");
        std::fs::copy(project.join(source), &staged)
            .unwrap_or_else(|error| panic!("copy {source}: {error}"));
        std::fs::rename(&staged, project.join(target))
            .unwrap_or_else(|error| panic!("rename to {target}: {error}"));
    }
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
    scratch_dir_in(&std::env::temp_dir(), test)
}

/// `scratch_dir` under `base`.
fn scratch_dir_in(base: &Path, test: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let dir = base.join(format!(
        "goport-build-watch-{test}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
    // tsc names the watched directories by the real path, so the test must
    // too: on Windows the temp dir can be a short name (`RUNNER~1`), and an
    // edit under it then reaches no watch.
    let real = fs::canonicalize(&dir).expect("canonical scratch dir");
    // On Windows the real path is verbatim (`\\?\C:\...`), which tsc does not
    // take as a cwd.
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
