//! The early emit of `tsc -p` and `tsc -b` with an incremental program
//! (`execute::incremental::Program::start_emit`). Each checker gets its emit
//! jobs right behind its check job, so it emits when its own check ends and
//! the emit pool runs during the check. The outputs, the build info, stdout
//! and the exit code must be the same as with `GOPORT_EARLY_EMIT=0`, which
//! keeps Go's barrier (the emit starts after the whole check).
//!
//! The fixture is `fixtures/emit_pool`: 4 program files and the es2020 lib
//! files, so 4 checkers get files. With declarations the JS parts of
//! `shapes.ts`, `legacy.js` and `index.ts` go to the emit pool and the d.ts
//! parts stay on the checker threads. `GOPORT_EMIT_THREADS=2` turns the pool
//! on at any core count. Each `tsgo` run writes to the same new directory
//! under the system temp dir, so the source map and build info paths are
//! the same. A passing test deletes it.
//!
//! The rule test checks `emit_can_start_with_check` on the same fixture:
//! each case that a check could see the outputs of keeps the barrier. Do not
//! set `GOPORT_EARLY_EMIT=0` for this test. Its F2 cases load
//! `tsconfig.rules.json` (the same config with `"exclude": []`): without an
//! `exclude`, the config excludes `outDir` and `declarationDir` from its
//! files, so no program file would be inside them.
//!
//! The emit-only tests make their own `tsc -b` solution in a scratch dir: a
//! task that checks nothing (every file's semantic diagnostics cached,
//! `noCheck`, or a syntax error) must emit on its checker threads too, so it
//! finishes when its own emit ends, as a Go builder does. The global
//! diagnostics test makes its own project there too. So do the k2gaps1
//! tests: `tsc -p --listFilesOnly` emits nothing, and `tsc -b` of two small
//! projects runs without a panic (also in the dev profile). An edit after a
//! build first waits until a new file gets a later mtime than every file of
//! the build (`Solution::wait_for_a_later_mtime`), so `tsc -b` sees it. The
//! barrier test loads the fixture and checks that `send_checker_barrier`
//! waits for the emit pool and the d.ts twins. The emitretry1 tests (Unix)
//! make their own project there too: a read-only output, or a `writeFile`
//! callback that gives an error, makes a write fail in `tsc -b`, `tsc -p`
//! or the API build, and the output must be Go N''s. So do the noeediag1
//! tests, for each case that turns the port's early emit off.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ts_goport::core::enter_program;
use ts_goport::emitter::program_emit::emit_can_start_with_check;
use ts_goport::flags::{ModuleKind, ModuleResolutionKind};
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::program::{release_program, try_load_version};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/emit_pool");

const CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/emit_pool/tsconfig.json"
);

/// `CONFIG` with `"exclude": []`, so program files can be in the output
/// directories.
const RULES_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/emit_pool/tsconfig.rules.json"
);

/// The tests that load a program in this process (the rule test and the
/// barrier test) take this lock: two program loads at once in one process
/// break the node store and file id checks of `ast/store.rs`.
static IN_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What one `tsgo` run wrote and printed.
#[derive(Debug, PartialEq)]
struct Run {
    /// The bytes of each file under the out dir, by relative path.
    files: BTreeMap<String, Vec<u8>>,
    stdout: String,
    status: Option<i32>,
}

#[test]
fn early_emit_writes_what_the_barrier_writes() {
    let root = scratch_dir();
    let out = root.join("out");
    let cases: [(&str, &[&str]); 3] = [
        ("js and d.ts", &[]),
        (
            "js only",
            &["--declaration", "false", "--declarationMap", "false"],
        ),
        ("d.ts only", &["--emitDeclarationOnly"]),
    ];
    for (case, extra) in cases {
        let barrier = tsgo(&out, extra, false);
        let early = tsgo(&out, extra, true);
        assert_eq!(
            barrier.status,
            Some(0),
            "{case}: the fixture must compile without diagnostics, so the check is sent: {}",
            barrier.stdout
        );
        assert!(
            barrier.files.contains_key("tsconfig.tsbuildinfo")
                && barrier.files.keys().any(|name| std::path::Path::new(name)
                    .extension()
                    .is_some_and(|e| e == "js")
                    || name.ends_with(".d.ts")),
            "{case}: the run must write outputs and build info: {:?}",
            barrier.files.keys()
        );
        assert_eq!(early, barrier, "{case}: the early emit against the barrier");
    }
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

#[test]
fn build_early_emit_writes_what_the_barrier_writes() {
    let root = scratch_dir();
    let out = root.join("out");
    // The fixture as a composite project, with its outputs and build info
    // in the scratch dir. The base config's `include` and `rootDir` stay
    // relative to the fixture.
    let config = root.join("tsconfig.json");
    let text = format!(
        r#"{{
  "extends": "{fixture}/tsconfig.json",
  "compilerOptions": {{
    "composite": true,
    "outDir": "{out}",
    "tsBuildInfoFile": "{out}/tsconfig.tsbuildinfo"
  }}
}}
"#,
        fixture = norm_str(FIXTURE),
        out = norm(&out)
    );
    fs::write(&config, text).unwrap_or_else(|error| panic!("write {}: {error}", config.display()));
    let barrier = tsgo_build(&config, &out, false);
    let early = tsgo_build(&config, &out, true);
    assert_eq!(
        barrier.status,
        Some(0),
        "the fixture must build without diagnostics, so the check is sent: {}",
        barrier.stdout
    );
    assert!(
        barrier.files.contains_key("tsconfig.tsbuildinfo")
            && barrier.files.keys().any(|name| std::path::Path::new(name)
                .extension()
                .is_some_and(|e| e == "js")
                || name.ends_with(".d.ts")),
        "the build must write outputs and build info: {:?}",
        barrier.files.keys()
    );
    assert_eq!(early, barrier, "the early emit against the barrier");
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// K2 (tscbemit1). `tsc -b --builders 2` on the solution p1 p2 p3, with no
/// references. Only the emits of p1 and p2 are pending (`--noEmit` builds
/// after an edit checked them): p1 is one root with a large emit, p2 a small
/// writer whose new output adds `v2`, and p3 imports `v2` from p2's output.
/// A Go builder writes its task's outputs when the task's emit ends, so p2
/// ends first, its builder takes p3, and p3 loads after p2 wrote: exit 0,
/// no output (Go N gives that). Before tscbemit1 a task with nothing to
/// check emitted on the loading thread when it finished, so p1 and p2
/// finished in build order and p3 loaded before p2 wrote (TS2305).
#[test]
fn build_emit_only_task_finishes_when_its_emit_ends() {
    assert_eq!(
        build_emit_only_solution("", &[], 0),
        (Some(0), String::new()),
        "p2 finishes before p1, so p3 loads after p2 wrote v2"
    );
}

/// tscbemit2: as `build_emit_only_task_finishes_when_its_emit_ends`, with
/// `noCheck` in p1. Go's p1 does no semantic check either
/// (`GetSemanticDiagnostics` returns nil), so it only emits: exit 0, no
/// output (Go N gives that). Before tscbemit2 the port started no early
/// emit for it, so it finished first, in build order (TS2305).
#[test]
fn build_no_check_task_finishes_when_its_emit_ends() {
    assert_eq!(
        build_emit_only_solution(r#", "noCheck": true"#, &[], 0),
        (Some(0), String::new()),
        "p2 finishes before p1, so p3 loads after p2 wrote v2"
    );
}

/// tscbemit2: as `build_emit_only_task_finishes_when_its_emit_ends`, with a
/// syntax error in p1. Go's `GetDiagnosticsOfAnyProgram` stops at the
/// syntactic diagnostics, so p1 only emits: only p1's TS1109 (Go N gives
/// that). Before tscbemit2 the port started no early emit for it, so it
/// finished first, in build order, and p3 also gave TS2305.
#[test]
fn build_task_with_syntax_errors_finishes_when_its_emit_ends() {
    assert_eq!(
        build_emit_only_solution("", &[("bad.ts", "export const bad = ;\n")], 2),
        (
            Some(2),
            "p1/src/bad.ts(1,20): error TS1109: Expression expected.\n".to_owned()
        ),
        "p2 finishes before p1, so p3 loads after p2 wrote v2"
    );
}

/// tscbemit2: Go reads the global diagnostics before the emit, also with
/// `noCheck` or program diagnostics (here TS6053, a missing file), where
/// `start_check` does not read them, so the early emit reads them first.
/// With `lib` es5 the d.ts emit of a generator asks for the missing global
/// type `IterableIterator`, which adds TS2318 to the checker's global
/// diagnostics. Go never reports it (it read them before), so `tsc -p` and
/// `tsc -b` give only the program diagnostics (Go N gives that). A read after
/// the early emit adds TS2318.
#[test]
fn early_emit_reads_the_global_diagnostics_before_the_emit() {
    let missing = concat!(
        "error TS6053: File '{root}/src/missing.ts' not found.\n",
        "  The file is in the program because:\n",
        "    Part of 'files' list in tsconfig.json\n"
    );
    for (options, files, expected) in [
        (r#", "noCheck": true"#, r#""include": ["src"]"#, None),
        (
            "",
            r#""files": ["src/a.ts", "src/missing.ts"]"#,
            Some(missing),
        ),
    ] {
        let root = scratch_dir();
        let config = format!(
            r#"{{"compilerOptions": {{"composite": true, "strict": true, "target": "es2022",
  "module": "esnext", "moduleResolution": "bundler", "outDir": "dist", "rootDir": "src",
  "skipLibCheck": true, "lib": ["es5"]{options}}}, {files}}}"#
        );
        fs::write(root.join("tsconfig.json"), config).expect("write the config");
        fs::create_dir(root.join("src")).expect("create src");
        fs::write(root.join("src/a.ts"), "export function* g() { yield 1; }\n")
            .expect("write a.ts");
        let expected =
            expected.map_or_else(String::new, |text| text.replace("{root}", &norm(&root)));
        let status = if expected.is_empty() { 0 } else { 2 };
        for args in [
            &["-p", "tsconfig.json"][..],
            &["-b", "tsconfig.json", "--force"],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
                .current_dir(&root)
                .args(args)
                .args(["--pretty", "false"])
                .env("GOPORT_EARLY_EMIT", "1")
                .output()
                .expect("run tsgo");
            assert_eq!(
                (
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout).into_owned()
                ),
                (Some(status), expected.clone()),
                "tsgo {args:?} with{options} and {files}: no TS2318 from the emit"
            );
        }
        fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
    }
}

/// k2gaps1 (M6): `tsc -p --listFilesOnly` on an incremental project lists
/// the program files, exits 0 and writes nothing (Go N gives that). The
/// early emit rules refuse `--listFilesOnly` (`early_emit_options_allow`;
/// `rules_keep_the_barrier_when_a_check_could_see_the_outputs` checks the
/// rule). An emit that started anyway would write only when it ended
/// before the process exits.
#[test]
fn tsc_p_list_files_only_writes_no_output() {
    let root = scratch_dir();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions": {"incremental": true, "strict": true, "target": "es2022",
  "module": "esnext", "moduleResolution": "bundler", "outDir": "dist", "rootDir": "src",
  "declaration": true, "skipLibCheck": true, "lib": ["es5"]}, "include": ["src"]}"#,
    )
    .expect("write the config");
    fs::create_dir(root.join("src")).expect("create src");
    fs::write(root.join("src/a.ts"), "export const a = 1;\n").expect("write a.ts");
    let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .current_dir(&root)
        .args([
            "-p",
            "tsconfig.json",
            "--listFilesOnly",
            "--pretty",
            "false",
        ])
        .env("GOPORT_EARLY_EMIT", "1")
        .output()
        .expect("run tsgo");
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned()
        ),
        (
            Some(0),
            format!(
                "bundled:///libs/lib.es5.d.ts\nbundled:///libs/lib.decorators.d.ts\n\
                 bundled:///libs/lib.decorators.legacy.d.ts\n{}/src/a.ts\n",
                norm(&root)
            )
        ),
        "tsc -p --listFilesOnly lists the files"
    );
    let mut files = BTreeMap::new();
    read_files(&root, &root, &mut files);
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        ["src/a.ts", "tsconfig.json"],
        "tsc -p --listFilesOnly writes nothing"
    );
    fs::remove_dir_all(&root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// k2gaps1 (C1): `program::send_checker_barrier` makes one value per
/// checker thread and one for the emit pool, and each drops only when the
/// jobs sent before have ended: the checker thread's own jobs, the jobs of
/// its d.ts twin (the d.ts prints, which wait for their JS parts on the
/// pool) and the emit pool jobs. `tsc -b` finishes a task when all its
/// values have dropped (`BuildTask::notify_when_compiled`), so a task with
/// a long emit on the pool or a twin finishes when that emit ends, as its
/// Go builder does. Here one twin job and one pool job wait on a gate, so
/// two values must stay until their gates open. `send_emit_pool_jobs` and
/// `send_dts_twin_job` make the pool and the twin at any core count.
#[test]
fn checker_barrier_waits_for_the_emit_pool_and_the_twins() {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Condvar, Mutex};
    use ts_goport::program::{
        run_on_checker_threads_for_files, send_checker_barrier, send_dts_twin_job,
        send_emit_pool_jobs, source_files,
    };

    /// A job waits in `pass` until `open` runs.
    #[derive(Clone, Default)]
    struct Gate(Arc<(Mutex<bool>, Condvar)>);
    impl Gate {
        fn pass(&self) {
            let (open, changed) = &*self.0;
            let mut open = open.lock().expect("gate lock");
            while !*open {
                open = changed.wait(open).expect("gate lock");
            }
        }
        fn open(&self) {
            *self.0.0.lock().expect("gate lock") = true;
            self.0.1.notify_all();
        }
    }
    /// Sends one `()` when the barrier drops it.
    struct Dropped(Sender<()>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let _in_process = IN_PROCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let config = norm_str(CONFIG);
    let program = try_load_version(&config, |_| {})
        .unwrap_or_else(|error| panic!("cannot load {config}: {error}"));
    {
        let _scope = enter_program(Some(program));
        let twin_gate = Gate::default();
        let pool_gate = Gate::default();
        let file = *source_files().last().expect("a program file");
        let gate = twin_gate.clone();
        let twin_jobs = run_on_checker_threads_for_files(&[file], move |_| {
            let gate = gate.clone();
            send_dts_twin_job(move || gate.pass())
        });
        let gate = pool_gate.clone();
        let pool_jobs = send_emit_pool_jobs(vec![move || gate.pass()]);
        let (sender, receiver) = channel();
        let values = send_checker_barrier(|| Dropped(sender.clone()));
        drop(sender);
        assert!(values >= 2, "one value per checker and one for the pool");

        // The values of the other checkers drop. The two values that wait
        // cannot drop before their gates open, so a short wait only times a
        // value that must not come, and a slow thread cannot fail the test.
        let long = Duration::from_secs(30);
        for _ in 2..values {
            receiver
                .recv_timeout(long)
                .expect("the value of a checker with no waiting job drops");
        }
        let short = Duration::from_millis(300);
        assert!(
            receiver.recv_timeout(short).is_err(),
            "the values of the waiting twin and the waiting pool job stay"
        );
        twin_gate.open();
        receiver
            .recv_timeout(long)
            .expect("the twin's value drops when its job ends");
        assert!(
            receiver.recv_timeout(short).is_err(),
            "the pool's value stays while its job waits"
        );
        pool_gate.open();
        receiver
            .recv_timeout(long)
            .expect("the pool's value drops when its job ends");
        drop((twin_jobs, pool_jobs));
    }
    release_program(program);
}

/// k2gaps1: `tsc -b` of two small composite projects (a cold build, then
/// an edit) exits 0 and prints nothing. Run it in the dev profile too
/// (`cargo test -p ts_goport --test early_emit`): there the
/// `debug_assert!`s of the early emit run, and the release build of the
/// protected tests leaves them out. In round 1 of k2gaps1 one of them
/// fired in every dev-profile `tsc -b`.
#[test]
fn build_two_composite_projects_without_a_panic() {
    let solution = Solution::new();
    solution.write(
        "tsconfig.json",
        r#"{"files": [], "references": [{"path": "./p1"}, {"path": "./p2"}]}"#,
    );
    solution.write("p1/tsconfig.json", &project_config(""));
    solution.write(
        "p2/tsconfig.json",
        r#"{"compilerOptions": {"composite": true, "strict": true, "target": "es2022",
  "module": "esnext", "moduleResolution": "bundler", "outDir": "dist", "rootDir": "src",
  "skipLibCheck": true}, "include": ["src"], "references": [{"path": "../p1"}]}"#,
    );
    solution.write("p1/src/index.ts", "export const v1 = 1;\n");
    solution.write(
        "p2/src/a.ts",
        "import { v1 } from \"../../p1/src/index\";\nexport const a = v1;\n",
    );
    assert_eq!(
        solution.build(&["tsconfig.json"]),
        (Some(0), String::new()),
        "the cold build"
    );
    solution.wait_for_a_later_mtime();
    solution.write("p1/src/index.ts", "export const v1 = 2;\n");
    assert_eq!(
        solution.build(&["tsconfig.json"]),
        (Some(0), String::new()),
        "the build after an edit"
    );
    assert_eq!(
        solution.read("p1/dist/index.js"),
        "export const v1 = 2;\n",
        "the build after an edit emits p1 again"
    );
    solution.remove();
}

/// emitretry1. `tsc -b` of an incremental project whose `out/a.js` is
/// read-only. The early emit keeps its writes until the task finishes, so
/// the write fails in the flush, and the files emit again with the kept
/// results (`emit_files::Writes::Replay`). Go N' (fed0bf24149f) reports the
/// TS5033 once, leaves `a.js` out of `EmittedFiles`, writes `b.js` and the
/// build info, and keeps the TS5033 in the build info's
/// `emitDiagnosticsPerFile`.
#[cfg(unix)]
#[test]
fn build_reports_a_failed_write_as_go() {
    if rustix::process::geteuid().is_root() {
        return; // root writes a read-only file
    }
    let project = write_failure_project(r#""incremental": true"#);
    let root = norm(&project.root);
    let (status, stdout) = project.build(&["tsconfig.json", "--listEmittedFiles"]);
    assert_eq!(
        (status, stdout),
        (
            Some(2),
            format!(
                "error TS5033: Could not write file '{root}/out/a.js': open {root}/out/a.js: permission denied.\n\
                 TSFILE: {root}/out/b.js\nTSFILE: {root}/tsconfig.tsbuildinfo\n"
            )
        ),
        "Go N' output"
    );
    assert_eq!(project.read("out/a.js"), "old\n", "a.js is not written");
    assert_eq!(project.read("out/b.js"), "export const b = 1;\n");
    let diagnostic = format!(
        r#"{{"noFile":true,"pos":-1,"end":-1,"code":5033,"category":1,"messageKey":"Could_not_write_file_0_Colon_1_5033","messageArgs":["{root}/out/a.js","open {root}/out/a.js: permission denied"]}}"#
    );
    assert_eq!(
        project
            .read("tsconfig.tsbuildinfo")
            .matches(&diagnostic)
            .count(),
        1,
        "the build info keeps the TS5033 once"
    );
    project.remove();
}

/// emitretry1. A composite project whose `out/a.js` is read-only. Go's d.ts
/// callback computes the signature of `a.ts` from `data.Diagnostics`, which
/// holds the TS5033 of the failed JS write, and dereferences its nil file
/// (incremental/snapshot.go:411): tsgo panics before it writes `a.d.ts`,
/// with exit 2 and nothing on stdout (Go N', `tsc -b` and `tsc -p`). Go's
/// runtime line ends with ` [recovered, repanicked]` in both modes. PORT:
/// `tsc -p` prints the line without it (the emit jobs do not mark a Go
/// panic as a work group's), so only `tsc -b` checks the whole line.
#[cfg(unix)]
#[test]
fn failed_write_before_a_dts_signature_panics_as_go() {
    const LINE: &str = "panic: runtime error: invalid memory address or nil pointer dereference";
    if rustix::process::geteuid().is_root() {
        return; // root writes a read-only file
    }
    for mode in ["-b", "-p"] {
        let project = write_failure_project(r#""composite": true"#);
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .current_dir(&project.root)
            .args([mode, "tsconfig.json", "--pretty", "false"])
            .env("GOPORT_EARLY_EMIT", "1")
            .output()
            .expect("run tsgo");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or_default();
        assert_eq!(
            (output.status.code(), output.stdout.is_empty()),
            (Some(2), true),
            "tsc {mode}: Go N' exit code and stdout; stderr: {stderr}"
        );
        if mode == "-b" {
            assert_eq!(first, format!("{LINE} [recovered, repanicked]"), "tsc -b");
        } else {
            assert!(first.starts_with(LINE), "tsc -p: {stderr}");
        }
        assert_eq!(project.read("out/a.js"), "old\n", "tsc {mode}: a.js");
        assert!(
            !project.root.join("out/a.d.ts").exists(),
            "tsc {mode}: Go panics before it writes a.d.ts"
        );
        project.remove();
    }
}

/// emitretry1. The API build with the `writeFile` callback: the client
/// answers the first `writeFile` with `first` and every later one with
/// `rest` (an error, or `{"kind":"noop"}`). Go N' sends one request per
/// output, in a varying order (its emit goroutines write in parallel), and
/// its answer has one TS5033 for each output whose request got an error,
/// sorted by file name. Before emitretry1 the port wrote the outputs again
/// after a failed write: `ok, err` gave 4 requests and 2 TS5033, `err, ok`
/// 3 requests and none.
#[cfg(unix)]
#[test]
fn api_build_writes_each_output_once_after_a_failed_write() {
    for (first, rest) in [(false, true), (true, false), (false, false), (true, true)] {
        let project = write_failure_project("");
        let root = norm(&project.root);
        let (requests, answer) = api_build(&project.root, first, rest);
        let mut files = requests.clone();
        files.sort();
        assert_eq!(files, ["a.js", "b.js"], "one writeFile request per output");
        let mut failed: Vec<&String> = requests
            .iter()
            .enumerate()
            .filter(|&(i, _)| !if i == 0 { first } else { rest })
            .map(|(_, file)| file)
            .collect();
        failed.sort();
        let diagnostics: Vec<String> = failed
            .iter()
            .map(|file| {
                format!(
                    r#"{{"pos":-1,"end":-1,"code":5033,"category":1,"text":"Could not write file '{root}/out/{file}': ipc: remote error [-32000]: client says no."}}"#
                )
            })
            .collect();
        let statistics = r#""statistics":{"Projects":1,"ProjectsBuilt":1,"TimestampUpdates":0}"#;
        let expected = if diagnostics.is_empty() {
            format!(r#"{{"jsonrpc":"2.0","id":7,"result":{{"status":0,{statistics}}}}}"#)
        } else {
            format!(
                r#"{{"jsonrpc":"2.0","id":7,"result":{{"status":2,"diagnostics":[{}],{statistics}}}}}"#,
                diagnostics.join(",")
            )
        };
        assert_eq!(
            answer, expected,
            "first ok {first}, rest ok {rest}: Go N' answer"
        );
        project.remove();
    }
}

/// noeediag1. The cases that turn the port's early emit off: a name, the
/// compiler options (with `incremental`), the output dir and the build info
/// path. Go has no early emit, so its output is the same in each.
#[cfg(unix)]
const NO_EARLY_EMIT: [(&str, &str, &str, &str); 5] = [
    (
        "noEmitOnError",
        r#""incremental": true, "noEmitOnError": true"#,
        "out",
        "tsconfig.tsbuildinfo",
    ),
    (
        "F1 nodenext, extensionless relative imports",
        r#""incremental": true, "module": "nodenext""#,
        "out",
        "tsconfig.tsbuildinfo",
    ),
    (
        "F3 outDir under node_modules",
        r#""incremental": true, "outDir": "node_modules/out""#,
        "node_modules/out",
        "node_modules/tsconfig.tsbuildinfo",
    ),
    (
        "F4 preserveSymlinks",
        r#""incremental": true, "preserveSymlinks": true"#,
        "out",
        "tsconfig.tsbuildinfo",
    ),
    (
        "GOPORT_EARLY_EMIT=0",
        r#""incremental": true"#,
        "out",
        "tsconfig.tsbuildinfo",
    ),
];

/// The `GOPORT_EARLY_EMIT` value of a `NO_EARLY_EMIT` case.
#[cfg(unix)]
fn early_emit_env(case: &str) -> &'static str {
    if case == "GOPORT_EARLY_EMIT=0" {
        "0"
    } else {
        "1"
    }
}

/// noeediag1. `tsc -b` without the port's early emit (`NO_EARLY_EMIT`),
/// with read-only `a.js` and `c.js`: the program emits them in its order
/// (b, c, a), and Go N' (fed0bf24149f) reports the TS5033s sorted (a, then
/// c), and its build info keeps them in `emitDiagnosticsPerFile`. The port
/// writes directly here, so this passed before noeediag1 too.
#[cfg(unix)]
#[test]
fn build_without_the_early_emit_reports_failed_writes_as_go() {
    if rustix::process::geteuid().is_root() {
        return; // root writes a read-only file
    }
    for (case, options, out_dir, build_info) in NO_EARLY_EMIT {
        let project = sort_failure_project(options, out_dir, true);
        let root = norm(&project.root);
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .current_dir(&project.root)
            .args([
                "-b",
                "tsconfig.json",
                "--listEmittedFiles",
                "--pretty",
                "false",
            ])
            .env("GOPORT_EARLY_EMIT", early_emit_env(case))
            .output()
            .expect("run tsgo -b");
        let error = |file: &str| format!("open {root}/{out_dir}/{file}: permission denied");
        assert_eq!(
            (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned()
            ),
            (
                Some(2),
                format!(
                    "error TS5033: Could not write file '{root}/{out_dir}/a.js': {}.\n\
                     error TS5033: Could not write file '{root}/{out_dir}/c.js': {}.\n\
                     TSFILE: {root}/{out_dir}/b.js\nTSFILE: {root}/{build_info}\n",
                    error("a.js"),
                    error("c.js")
                )
            ),
            "{case}: Go N' output"
        );
        let build_info = project.read(build_info);
        let expected = emit_diagnostics_per_file(&build_info, &root, out_dir, error);
        assert!(
            build_info.contains(&expected),
            "{case}: Go N' build info has {expected}; the port's: {build_info}"
        );
        project.remove();
    }
}

/// noeediag1. The API build without the port's early emit (`NO_EARLY_EMIT`):
/// the client answers the `writeFile` requests of `a.js` and `c.js` with an
/// error and the others with `useOS`. Go N''s emitter gets each error at
/// the write and puts the TS5033 into the file's result, so its answer has
/// them sorted (a, then c) and its build info has `emitDiagnosticsPerFile`.
/// Before noeediag1 the port's checker threads only kept these writes
/// (`build_task.rs` `DeferredWrites`), and their TS5033s came after the emit:
/// c before a, and no `emitDiagnosticsPerFile`. The last case is the API
/// build with the client's `build` option and no `incremental`: Go emits
/// without the incremental state, and its build info has `errors`.
#[cfg(unix)]
#[test]
fn api_build_without_the_early_emit_keeps_failed_writes_in_their_files() {
    let answer = |root: &str, out_dir: &str| {
        let diagnostic = |file: &str| {
            format!(
                r#"{{"pos":-1,"end":-1,"code":5033,"category":1,"text":"Could not write file '{root}/{out_dir}/{file}': ipc: remote error [-32000]: client says no."}}"#
            )
        };
        format!(
            r#"{{"jsonrpc":"2.0","id":7,"result":{{"status":2,"diagnostics":[{},{}],"statistics":{{"Projects":1,"ProjectsBuilt":1,"TimestampUpdates":0}}}}}}"#,
            diagnostic("a.js"),
            diagnostic("c.js")
        )
    };
    let ok = |_: usize, file: &str| !matches!(file, "a.js" | "c.js");
    for (case, options, out_dir, build_info) in NO_EARLY_EMIT {
        let project = sort_failure_project(options, out_dir, false);
        let root = norm(&project.root);
        let env = [("GOPORT_EARLY_EMIT", early_emit_env(case))];
        let (_, got) = api_build_with(&project.root, &env, "", "useOS", &ok);
        assert_eq!(got, answer(&root, out_dir), "{case}: Go N' answer");
        let build_info = project.read(build_info);
        let expected = emit_diagnostics_per_file(&build_info, &root, out_dir, |_| {
            "ipc: remote error [-32000]: client says no".to_string()
        });
        assert!(
            build_info.contains(&expected),
            "{case}: Go N' build info has {expected}; the port's: {build_info}"
        );
        project.remove();
    }
    let project = sort_failure_project("", "out", false);
    let root = norm(&project.root);
    let params = r#","compilerOptions":{"build":true}"#;
    let (_, got) = api_build_with(&project.root, &[], params, "useOS", &ok);
    assert_eq!(got, answer(&root, "out"), "build option: Go N' answer");
    let build_info = project.read("tsconfig.tsbuildinfo");
    assert!(
        build_info.ends_with(r#","errors":true,"root":["./src/a.ts","./src/b.ts","./src/c.ts"]}"#),
        "build option: Go N' build info has errors; the port's: {build_info}"
    );
    project.remove();
}

/// The project of the noeediag1 tests: `src/a.ts` imports `src/b.ts` and
/// `src/c.ts` (so the program order is b, c, a), with `options` added to
/// `strict`, `outDir` (`out_dir`) and `rootDir`. With `read_only`,
/// `{out_dir}/a.js` and `{out_dir}/c.js` exist, hold `old` and are
/// read-only.
#[cfg(unix)]
fn sort_failure_project(options: &str, out_dir: &str, read_only: bool) -> Solution {
    let project = Solution::new();
    let options = if options.is_empty() {
        String::new()
    } else {
        format!(", {options}")
    };
    // `options` can name the `outDir`: a JSON object takes a key once.
    let out_dir_option = if options.contains("\"outDir\"") {
        String::new()
    } else {
        format!(r#", "outDir": "{out_dir}""#)
    };
    project.write(
        "tsconfig.json",
        &format!(
            r#"{{"compilerOptions": {{"strict": true, "rootDir": "src"{out_dir_option}{options}}}, "include": ["src"]}}"#
        ),
    );
    project.write(
        "src/a.ts",
        "import { b } from \"./b\";\nimport { c } from \"./c\";\nexport const a: number = b + c;\n",
    );
    project.write("src/b.ts", "export const b: number = 1;\n");
    project.write("src/c.ts", "export const c: number = 2;\n");
    if read_only {
        for file in ["a.js", "c.js"] {
            let file = format!("{out_dir}/{file}");
            project.write(&file, "old\n");
            let path = project.root.join(file);
            fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o444))
                .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
        }
    }
    project
}

/// Go N''s `emitDiagnosticsPerFile` for a `sort_failure_project` build whose
/// writes of `{out_dir}/a.js` and `{out_dir}/c.js` failed with `error(file)`.
/// Go sorts the entries by path (incremental/snapshottobuildinfo.go
/// `setEmitDiagnostics`), so a's comes first. The file ids are the places of
/// `src/a.ts` and `src/c.ts` in the build info's `fileNames`.
#[cfg(unix)]
fn emit_diagnostics_per_file(
    build_info: &str,
    root: &str,
    out_dir: &str,
    error: impl Fn(&str) -> String,
) -> String {
    let names = build_info
        .split(r#""fileNames":["#)
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| panic!("no fileNames in {build_info}"));
    let entry = |file: &str| {
        let source = format!("src/{}.ts\"", &file[..1]);
        let id = names
            .split(',')
            .position(|name| name.ends_with(&source))
            .unwrap_or_else(|| panic!("no {source} in the fileNames {names}"))
            + 1;
        format!(
            r#"[{id},[{{"noFile":true,"pos":-1,"end":-1,"code":5033,"category":1,"messageKey":"Could_not_write_file_0_Colon_1_5033","messageArgs":["{root}/{out_dir}/{file}","{}"]}}]]"#,
            error(file)
        )
    };
    format!(
        r#""emitDiagnosticsPerFile":[{},{}]"#,
        entry("a.js"),
        entry("c.js")
    )
}

/// The project of the write failure tests: `src/a.ts` imports `src/b.ts`,
/// with `options` added to `strict`, `outDir` and `rootDir`. `out/a.js`
/// exists, holds `old` and is read-only.
#[cfg(unix)]
fn write_failure_project(options: &str) -> Solution {
    let project = Solution::new();
    let options = if options.is_empty() {
        String::new()
    } else {
        format!(", {options}")
    };
    project.write(
        "tsconfig.json",
        &format!(
            r#"{{"compilerOptions": {{"strict": true, "outDir": "out", "rootDir": "src"{options}}}, "include": ["src"]}}"#
        ),
    );
    project.write(
        "src/a.ts",
        "import { b } from \"./b\";\nexport const a: number = b;\n",
    );
    project.write("src/b.ts", "export const b: number = 1;\n");
    project.write("out/a.js", "old\n");
    let path = project.root.join("out/a.js");
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o444))
        .unwrap_or_else(|error| panic!("chmod {}: {error}", path.display()));
    project
}

/// Builds the project in `root` through `tsgo --api --async --callbacks
/// writeFile` (createBuildOrchestrator, then build 7). The client answers
/// the first `writeFile` request with success (`noop`) when `first` and
/// with an error else, every later one as `rest`. Returns the file name of
/// each request in order, and the build answer.
#[cfg(unix)]
fn api_build(root: &Path, first: bool, rest: bool) -> (Vec<String>, String) {
    let ok = |index: usize, _: &str| if index == 0 { first } else { rest };
    api_build_with(root, &[], "", "noop", &ok)
}

/// `api_build` with tsgo's `env`, `params` added to the
/// createBuildOrchestrator params (`,"name":value` or empty), and the
/// client's answers: `ok(index, file name)` of each `writeFile` request
/// gives success (`{"kind": ok_kind}`) or an error.
#[cfg(unix)]
fn api_build_with(
    root: &Path,
    env: &[(&str, &str)],
    params: &str,
    ok_kind: &str,
    ok: &dyn Fn(usize, &str) -> bool,
) -> (Vec<String>, String) {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["--api", "--async", "--callbacks", "writeFile", "--cwd"])
        .arg(norm(root))
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run tsgo --api");
    let mut stdin = child.stdin.take().expect("tsgo stdin");
    let (sender, messages) = std::sync::mpsc::channel::<String>();
    let mut stdout = BufReader::new(child.stdout.take().expect("tsgo stdout"));
    std::thread::spawn(move || {
        loop {
            let mut length = None;
            loop {
                let mut line = String::new();
                if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = value.trim().parse::<usize>().ok();
                }
            }
            let mut body = vec![0; length.expect("a Content-Length header")];
            if stdout.read_exact(&mut body).is_err()
                || sender
                    .send(String::from_utf8_lossy(&body).into_owned())
                    .is_err()
            {
                return;
            }
        }
    });
    let mut send = |message: String| {
        write!(stdin, "Content-Length: {}\r\n\r\n{message}", message.len())
            .expect("write a request");
        stdin.flush().expect("flush a request");
    };
    let next = || {
        messages
            .recv_timeout(Duration::from_secs(60))
            .expect("a message from tsgo")
    };
    let config = norm(&root.join("tsconfig.json"));
    send(format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"createBuildOrchestrator","params":{{"rootNames":["{config}"],"cwd":"{}"{params}}}}}"#,
        norm(root)
    ));
    let created = next();
    let id = created
        .split(r#""buildOrchestratorID":"#)
        .nth(1)
        .and_then(|rest| rest.split(['}', ',']).next())
        .unwrap_or_else(|| panic!("no orchestrator id in {created}"))
        .to_owned();
    send(format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"build","params":{{"buildOrchestratorID":{id}}}}}"#
    ));
    let mut requests = Vec::new();
    let answer = loop {
        let message = next();
        if message.starts_with(r#"{"jsonrpc":"2.0","id":7,"#) {
            break message;
        }
        let call = message
            .strip_prefix(r#"{"jsonrpc":"2.0","id":""#)
            .and_then(|rest| rest.split_once(r#"","method":"writeFile","params":{"path":""#))
            .unwrap_or_else(|| panic!("not a writeFile request: {message}"));
        let (call_id, path) = (
            call.0.to_owned(),
            call.1.split('"').next().unwrap_or_default(),
        );
        let file = path.rsplit('/').next().unwrap_or_default().to_owned();
        let answer_ok = ok(requests.len(), &file);
        requests.push(file);
        send(if answer_ok {
            format!(r#"{{"jsonrpc":"2.0","id":"{call_id}","result":{{"kind":"{ok_kind}"}}}}"#)
        } else {
            format!(
                r#"{{"jsonrpc":"2.0","id":"{call_id}","error":{{"code":-32000,"message":"client says no"}}}}"#
            )
        });
    };
    drop(stdin);
    child
        .wait()
        .expect("tsgo --api exits after the end of stdin");
    (requests, answer)
}

/// The solution config of p1, p2 and p3.
const SOLUTION: &str =
    r#"{"files": [], "references": [{"path": "./p1"}, {"path": "./p2"}, {"path": "./p3"}]}"#;

/// A `tsc -b` solution in a scratch dir.
struct Solution {
    root: PathBuf,
}

impl Solution {
    fn new() -> Self {
        Solution {
            root: scratch_dir(),
        }
    }

    /// Writes `text` to `path` under the root, and makes its directories.
    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("a file in a project"))
            .unwrap_or_else(|error| panic!("create the dir of {}: {error}", path.display()));
        fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    /// Runs `tsgo -b` with `args` and `--pretty false`, and returns its exit
    /// code and stdout.
    fn build(&self, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .current_dir(&self.root)
            .arg("-b")
            .args(args)
            .args(["--pretty", "false"])
            .env("GOPORT_EARLY_EMIT", "1")
            .output()
            .expect("run tsgo -b");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    /// The text of `path` under the root.
    fn read(&self, path: &str) -> String {
        let path = self.root.join(path);
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    /// Waits until a file written now gets a later mtime than every file
    /// under the root, so `tsc -b` sees the next edit as newer than the
    /// outputs of the last build. A file system clock can move in steps:
    /// Linux stamps files with its tick clock (4 ms at 250 Hz) unless the
    /// file system has fine-grained timestamps (Linux 6.13). An edit right
    /// after a build can then get the mtime of the project's build info,
    /// and `tsc -b` sees the project as up to date (Go `getUpToDateStatus`
    /// rebuilds only for an input newer than the build info). On main CI
    /// (the Ubuntu 24.04 runner) that once made p1 of
    /// `build_emit_only_solution` up to date: it finished first, and p3 read
    /// p2's old output (TS2305).
    fn wait_for_a_later_mtime(&self) {
        let newest = newest_mtime(&self.root);
        let probe = self.root.join("mtime-probe");
        loop {
            fs::write(&probe, "")
                .unwrap_or_else(|error| panic!("write {}: {error}", probe.display()));
            let probed = fs::metadata(&probe)
                .and_then(|metadata| metadata.modified())
                .unwrap_or_else(|error| panic!("mtime of {}: {error}", probe.display()));
            if probed > newest {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        fs::remove_file(&probe)
            .unwrap_or_else(|error| panic!("remove {}: {error}", probe.display()));
    }

    /// Removes the scratch dir. A failed test keeps it.
    fn remove(self) {
        fs::remove_dir_all(&self.root)
            .unwrap_or_else(|error| panic!("remove {}: {error}", self.root.display()));
    }
}

/// The latest mtime of a file under `dir`.
fn newest_mtime(dir: &Path) -> SystemTime {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
    entries
        .map(|entry| {
            let path = entry.expect("a dir entry").path();
            if path.is_dir() {
                newest_mtime(&path)
            } else {
                fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or_else(|error| panic!("mtime of {}: {error}", path.display()))
            }
        })
        .max()
        .unwrap_or(UNIX_EPOCH)
}

/// The config of a solution project, with `extra` added to its compiler
/// options.
fn project_config(extra: &str) -> String {
    format!(
        r#"{{"compilerOptions": {{"composite": true, "strict": true, "target": "es2022",
  "module": "esnext", "moduleResolution": "bundler", "outDir": "dist", "rootDir": "src",
  "skipLibCheck": true{extra}}}, "include": ["src"]}}"#
    )
}

/// The code of 1,500 modules in one file, with `tag` in each comment: its
/// load and emit take far longer than those of a small project.
fn big_module(tag: &str) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for i in 0..1500 {
        write!(
            text,
            "export interface I{i} {{ a: number; b: string; c{i}: boolean }}\n\
             export function f{i}(x: I{i}): I{i} {{ return {{ ...x }}; }}\n\
             export class C{i} {{ constructor(public v: I{i}) {{}} get(): I{i} {{ return f{i}(this.v); }} }}\n\
             export const k{i}: number = {i}; // {tag}\n"
        )
        .expect("write to a String");
    }
    text
}

/// Makes the solution of `build_emit_only_task_finishes_when_its_emit_ends`
/// in a scratch dir, with `p1_options` added to p1's compiler options and
/// `p1_files` added to p1's `src`, runs its builds and returns the exit code
/// and stdout of the last one (`--builders 2`). `p1_no_emit` is Go N's exit
/// code of `tsc -b p1 --noEmit` on it.
fn build_emit_only_solution(
    p1_options: &str,
    p1_files: &[(&str, &str)],
    p1_no_emit: i32,
) -> (Option<i32>, String) {
    let solution = Solution::new();
    solution.write("tsconfig.json", SOLUTION);
    solution.write("p1/tsconfig.json", &project_config(p1_options));
    for project in ["p2", "p3"] {
        solution.write(&format!("{project}/tsconfig.json"), &project_config(""));
    }
    // Its emit takes far longer than the load and emit of p2.
    solution.write("p1/src/index.ts", &big_module("v1"));
    for (name, text) in p1_files {
        solution.write(&format!("p1/src/{name}"), text);
    }
    solution.write("p2/src/s0.ts", "export const s0 = 0;\n");
    solution.write("p2/src/s1.ts", "export const s1 = 1;\n");
    solution.write("p2/src/index.ts", "export const v1 = 1;\n");
    solution.write(
        "p3/src/a.ts",
        "import { v1, v2 } from \"../../p2/dist/index\";\nexport const a = v1 + v2;\n",
    );
    // The cold build: p3 cannot see `v2` yet.
    solution.build(&["tsconfig.json"]);
    solution.wait_for_a_later_mtime();
    solution.write("p1/src/index.ts", &big_module("v2"));
    solution.write(
        "p2/src/index.ts",
        "export const v1 = 1;\nexport const v2 = 2;\n",
    );
    // p1's errors are part of the case; p2 must check clean. `--verbose`
    // shows that each build sees the edit, so only its emit is pending.
    for (project, no_emit) in [("p1", p1_no_emit), ("p2", 0)] {
        let (status, stdout) = solution.build(&[project, "--noEmit", "--verbose"]);
        assert!(
            status == Some(no_emit)
                && stdout.contains(&format!("Building project '{project}/tsconfig.json'")),
            "tsc -b {project} --noEmit checks the edit (exit {status:?}): {stdout}"
        );
    }
    let result = solution.build(&["tsconfig.json", "--builders", "2"]);
    solution.remove();
    result
}

#[test]
fn rules_keep_the_barrier_when_a_check_could_see_the_outputs() {
    let _in_process = IN_PROCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let out_dir = std::env::temp_dir()
        .join("goport-early-emit-rules")
        .to_string_lossy()
        .into_owned();
    let out_dir = norm_str(&out_dir);
    // As in `early_emit_writes_what_the_barrier_writes`.
    let out = out_dir.clone();
    assert!(
        can_start(move |options| options.out_dir = out),
        "the fixture with a temp outDir must start its emit with the check"
    );

    // F1: `index.ts` imports "./shapes" without an extension.
    assert!(!can_start(|options| {
        options.module = ModuleKind::NODE_NEXT;
        options.module_resolution = ModuleResolutionKind::NODE_NEXT;
    }));
    // F2: the program files are inside the outDir or declarationDir.
    assert!(!can_start_with(RULES_CONFIG, |options| {
        options.out_dir = format!("{}/src", norm_str(FIXTURE));
    }));
    assert!(!can_start_with(RULES_CONFIG, |options| {
        options.declaration_dir = norm_str(FIXTURE);
    }));
    // F3: an output directory under `node_modules`.
    let under_node_modules = format!("{out_dir}/node_modules/out");
    assert!(!can_start(move |options| {
        options.out_dir = under_node_modules;
    }));
    // F4 and the option rules.
    assert!(!can_start(|options| {
        options.preserve_symlinks = Tristate::True;
    }));
    assert!(!can_start(|options| {
        options.no_emit_on_error = Tristate::True;
    }));
    assert!(!can_start(|options| {
        options.single_threaded = Tristate::True;
    }));
    // k2gaps1 (M6): Go emits nothing with `--listFilesOnly`.
    assert!(!can_start(|options| {
        options.list_files_only = Tristate::True;
    }));
}

/// Loads the fixture with `edit` applied to its options and returns
/// `emit_can_start_with_check` for it.
fn can_start(edit: impl FnOnce(&mut CompilerOptions)) -> bool {
    can_start_with(CONFIG, edit)
}

/// `can_start` with the fixture config `config`.
fn can_start_with(config: &str, edit: impl FnOnce(&mut CompilerOptions)) -> bool {
    let config = norm_str(config);
    let program = try_load_version(&config, edit)
        .unwrap_or_else(|error| panic!("cannot load {config}: {error}"));
    let can_start = {
        let _scope = enter_program(Some(program));
        emit_can_start_with_check()
    };
    release_program(program);
    can_start
}

/// Runs `tsgo -p` on the fixture as an incremental program, with `out` as
/// the out dir and the build info in it, and `extra` arguments. `early`
/// false sets `GOPORT_EARLY_EMIT=0`. It removes `out` first and returns what
/// the run wrote there and printed.
fn tsgo(out: &Path, extra: &[&str], early: bool) -> Run {
    if out.exists() {
        fs::remove_dir_all(out).unwrap_or_else(|error| panic!("remove {}: {error}", out.display()));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["-p", &norm_str(CONFIG), "--incremental", "--outDir"])
        .arg(norm(out))
        .arg("--tsBuildInfoFile")
        .arg(norm(&out.join("tsconfig.tsbuildinfo")))
        .args(["--listEmittedFiles", "--pretty", "false"])
        .args(extra)
        .env("GOPORT_EMIT_THREADS", "2")
        .env("GOPORT_EARLY_EMIT", if early { "1" } else { "0" })
        .output()
        .expect("run tsgo");
    let mut files = BTreeMap::new();
    if out.exists() {
        read_files(out, out, &mut files);
    }
    Run {
        files,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        status: output.status.code(),
    }
}

/// Runs `tsgo -b` on `config`, whose outputs and build info are in `out`.
/// `early` false sets `GOPORT_EARLY_EMIT=0`. It removes `out` first, so the
/// project is out of date, and returns what the run wrote there and printed.
fn tsgo_build(config: &Path, out: &Path, early: bool) -> Run {
    if out.exists() {
        fs::remove_dir_all(out).unwrap_or_else(|error| panic!("remove {}: {error}", out.display()));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .arg("-b")
        .arg(norm(config))
        .args(["--listEmittedFiles", "--pretty", "false"])
        .env("GOPORT_EMIT_THREADS", "2")
        .env("GOPORT_EARLY_EMIT", if early { "1" } else { "0" })
        .output()
        .expect("run tsgo -b");
    let mut files = BTreeMap::new();
    if out.exists() {
        read_files(out, out, &mut files);
    }
    Run {
        files,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        status: output.status.code(),
    }
}

/// Reads every file under `dir` into `files`, by path relative to `root`
/// with `/` separators (on Windows too).
fn read_files(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
    {
        let path = entry.expect("out dir entry").path();
        if path.is_dir() {
            read_files(root, &path, files);
        } else {
            let name = path
                .strip_prefix(root)
                .expect("a path under the out dir")
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let bytes =
                fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            files.insert(name, bytes);
        }
    }
}

/// A new directory under the system temp dir, by its real path (the
/// program sees real paths). The name holds the process id, the time and a
/// count of this process: two tests that start at once can read the same
/// time (3 of 20 runs on 2 CPUs failed so).
fn scratch_dir() -> PathBuf {
    static MADE: AtomicUsize = AtomicUsize::new(0);
    let count = MADE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "goport-early-emit-{}-{nanos}-{count}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
    let real = fs::canonicalize(&dir).expect("canonical scratch dir");
    // On Windows the real path is verbatim (`\\?\C:\...`), which the program
    // does not take as a cwd or in an argument.
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

/// `text` as the program names a path: with `/` separators and no verbatim
/// prefix (`C:/Users/x/y` on Windows). The identity on Unix.
fn norm_str(text: &str) -> String {
    #[cfg(windows)]
    let text = unverbatim(text).replace('\\', "/");
    #[cfg(not(windows))]
    let text = text.to_owned();
    text
}

/// `norm_str` of a path.
fn norm(path: &Path) -> String {
    norm_str(&path.to_string_lossy())
}
