//! Port-only test of the `--explainFiles` lines that tell a file's module
//! format (Go `compiler/includeprocessor.go:123
//! explainRedirectAndImpliedFormat`).
//!
//! PORT: no Go counterpart (projfuzz1 EXPLAIN_ABS, loadparity1). Go caches
//! these lines per file, and the first caller's names win: `ExplainFiles`
//! uses names relative to the cwd (`program.go:2120`), and the include
//! processor diagnostics use absolute names (`processingDiagnostic.go:106`).
//! After a syntax error Go never builds those diagnostics (`program.go:2031`),
//! so the names are relative. The port built them when it made the program,
//! and it printed the absolute name. The lines also read the CommonJS
//! indicator that the binder sets, so the point where Go first reads the
//! include processor diagnostics, before or after the bind, shows in them.
//! In `tsc -b --watch` Go gives each program a new parse of a `.js` file, so
//! a file that the port keeps for a later program must not show the bind of
//! an earlier one before its own bind (R232 review, inputs A and B; case L
//! of the loadparity2 check: a second project in the same cycle).
//! The expected lines of the first test are Go N's, the others Go's at pin
//! 3e9236361fa9.

use ts_goport::execute::tsc::ExitStatus;
use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::context;

use crate::support::child::{
    command_line_in_process, new_in_process_test_sys, run_command_in_child, run_test_in_child,
};
use crate::support::runner::TscInput;
use crate::support::test_sys::new_test_sys;

const PROJECT: &str = "/home/src/workspaces/project";

#[test]
fn explain_files_after_a_syntax_error_uses_relative_names() {
    let input = TscInput {
        files: [
            (
                format!("{PROJECT}/tsconfig.json"),
                r#"{"compilerOptions":{"module":"nodenext","rootDir":"src/lib"}}"#.into(),
            ),
            // TS6059 (not under rootDir) explains src/a.ts.
            (format!("{PROJECT}/src/a.ts"), "let = ;\n".into()),
            (
                format!("{PROJECT}/src/lib/b.ts"),
                "export const b = 1;\n".into(),
            ),
            (
                format!("{PROJECT}/package.json"),
                r#"{"type":"module"}"#.into(),
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = ["-p", ".", "--explainFiles", "--noEmit", "--pretty", "false"].map(String::from);
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    // Go N exits 2.
    assert_eq!(
        result.status,
        ExitStatus::DiagnosticsPresentOutputsGenerated
    );
    let output = sys.output_text();
    let want = "src/a.ts\n   Matched by default include pattern '**/*'\n   File is ECMAScript module because 'package.json' has field \"type\" with value \"module\"\nsrc/lib/b.ts\n";
    assert!(output.contains(want), "no {want:?} in:\n{output}");
}

/// Runs tsc in `PROJECT` with `files` and `args`, and returns the output
/// without the TS2318 lines (`noLib` gives one per global type) and the
/// test system's "!!! List files" markers.
fn tsc_output(files: &[(&str, &str)], args: &[&str]) -> String {
    let input = TscInput {
        files: files
            .iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), (*text).into()))
            .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    sys.output_text()
        .lines()
        .filter(|line| !line.contains("TS2318") && !line.starts_with("!!! "))
        .map(|line| format!("{line}\n"))
        .collect()
}

// Go `ast.IsExternalOrCommonJSModule` reads the CommonJS indicator that the
// binder sets. With `moduleDetection: legacy` the parser gives a file with
// only CommonJS syntax no external module indicator. tsc binds before
// `ExplainFiles`, so a.js and b.js get the format line and c.js does not.
// The expected lines are Go's (pin 3e9236361fa9).
#[test]
fn explain_files_reads_the_bound_common_js_indicator() {
    let output = tsc_output(
        &[
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"allowJs":true,"module":"nodenext","moduleDetection":"legacy","noLib":true,"types":[]}}"#,
            ),
            (
                "src/a.js",
                "const b = require(\"./b\");\nmodule.exports = { b };\n",
            ),
            ("src/b.js", "exports.y = 1;\n"),
            ("src/c.js", "var c = 1;\n"),
        ],
        &["-p", ".", "--explainFiles", "--noEmit", "--pretty", "false"],
    );
    let want = concat!(
        "src/b.js\n",
        "   Imported via \"./b\" from file 'src/a.js'\n",
        "   Matched by default include pattern '**/*'\n",
        "   File is CommonJS module because 'package.json' was not found\n",
        "src/a.js\n",
        "   Matched by default include pattern '**/*'\n",
        "   File is CommonJS module because 'package.json' was not found\n",
        "src/c.js\n",
        "   Matched by default include pattern '**/*'\n",
    );
    assert!(output.contains(want), "no {want:?} in:\n{output}");
}

/// The TS6059 diagnostic and the `--explainFiles` lines of src/c.js, which
/// has only CommonJS syntax. Both share Go's per-file cache of the format
/// line, which the include processor diagnostics fill at their first read.
fn root_dir_output(extra_args: &[&str]) -> String {
    let mut args = vec!["-p", ".", "--explainFiles", "--noEmit", "--pretty", "false"];
    args.extend_from_slice(extra_args);
    tsc_output(
        &[
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"allowJs":true,"module":"nodenext","moduleDetection":"legacy","rootDir":"src/sub","noLib":true,"types":[]}}"#,
            ),
            ("src/sub/a.ts", "export const a = 1;\n"),
            ("src/c.js", "module.exports = 1;\n"),
        ],
        &args,
    )
}

// tsc reads the include processor diagnostics in `GetProgramDiagnostics`,
// before the bind (program.go:2078), so neither TS6059 nor `ExplainFiles`
// has the CommonJS line of c.js. The expected lines are Go's.
#[test]
fn root_dir_diagnostic_before_the_bind_has_no_common_js_line() {
    let output = root_dir_output(&[]);
    let want = format!(
        concat!(
            "error TS6059: File '{p}/src/c.js' is not under 'rootDir' '{p}/src/sub'. 'rootDir' is expected to contain all source files.\n",
            "  The file is in the program because:\n",
            "    Matched by default include pattern '**/*'\n",
            "src/c.js\n",
            "   Matched by default include pattern '**/*'\n",
            "src/sub/a.ts\n",
            "   Matched by default include pattern '**/*'\n",
            "   File is CommonJS module because 'package.json' was not found\n",
        ),
        p = PROJECT
    );
    assert!(output.contains(&want), "no {want:?} in:\n{output}");
}

// An incremental program binds first (`programToSnapshot`), so Go's
// diagnostics and `ExplainFiles` both have the CommonJS line of c.js. The
// expected lines are Go's.
#[test]
fn root_dir_diagnostic_of_an_incremental_program_has_the_common_js_line() {
    let output = root_dir_output(&["--incremental", "--tsBuildInfoFile", "tbi/x.tsbuildinfo"]);
    let want = format!(
        concat!(
            "error TS6059: File '{p}/src/c.js' is not under 'rootDir' '{p}/src/sub'. 'rootDir' is expected to contain all source files.\n",
            "  The file is in the program because:\n",
            "    Matched by default include pattern '**/*'\n",
            "  File is CommonJS module because 'package.json' was not found\n",
            "src/c.js\n",
            "   Matched by default include pattern '**/*'\n",
            "   File is CommonJS module because 'package.json' was not found\n",
            "src/sub/a.ts\n",
            "   Matched by default include pattern '**/*'\n",
            "   File is CommonJS module because 'package.json' was not found\n",
        ),
        p = PROJECT
    );
    assert!(output.contains(&want), "no {want:?} in:\n{output}");
}

/// A `tsc -b --watch` session in `PROJECT` with `files` and `args`: the
/// output of the first build and of the cycle after the edit of `edit.0`
/// to `edit.1`, each without blank lines, the time stamp lines and the
/// test system's "!!!" markers.
fn build_watch_cycles(files: &[(&str, &str)], args: &[&str], edit: (&str, &str)) -> [String; 2] {
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
    let lines = |text: String| -> String {
        text.lines()
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("!!! ")
                    && !line.contains(" - Starting compilation in watch mode...")
                    && !line.contains(" - File change detected.")
                    && !line.contains("Watching for file changes.")
            })
            .map(|line| format!("{line}\n"))
            .collect()
    };
    let first = lines(sys.output_text());
    sys.set_output_bytes(Vec::new());
    let path = format!("{PROJECT}/{}", edit.0);
    let _ = sys.fs_from_file_map().write_file(&path, edit.1);
    sys.mock_watch_backend().send_events(vec![Event {
        kind: EventKind::Update,
        path,
    }]);
    w.do_cycle();
    [first, lines(sys.output_text())]
}

/// Input A of the R232 review: src/c.js has only CommonJS syntax and is
/// outside `rootDir`, so TS6059 explains it. `tsc -b` without
/// `incremental` builds the TS6059 chain before any bind. Go parses c.js
/// again for the program of each cycle, so the chain of cycle 2 has no
/// CommonJS line either. The port kept the parse of c.js, bound by cycle 1,
/// and printed the line in cycle 2. The expected lines are Go's
/// (`tsgo-oracle-3e9236361fa9 -b tsconfig.json --watch`, edit of a.ts).
fn build_watch_root_dir_chain(extra_args: &[&str]) {
    let mut args = vec!["-b", "tsconfig.json", "--watch", "--pretty", "false"];
    args.extend_from_slice(extra_args);
    let [first, second] = build_watch_cycles(
        &[
            ("package.json", "{}\n"),
            ("src/sub/a.ts", "export const a = 1;\n"),
            ("src/c.js", "module.exports = 1;\n"),
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"allowJs":true,"moduleDetection":"legacy","module":"node16","rootDir":"src/sub","outDir":"out","types":[]},"include":["src"]}"#,
            ),
        ],
        &args,
        ("src/sub/a.ts", "export const a = 2;\n"),
    );
    let ts5055 = format!(
        "error TS5055: Cannot write file '{PROJECT}/src/c.js' because it would overwrite input file.\n"
    );
    let ts6059 = format!(
        "error TS6059: File '{PROJECT}/src/c.js' is not under 'rootDir' '{PROJECT}/src/sub'. 'rootDir' is expected to contain all source files.\n"
    );
    let chain = format!(
        "  The file is in the program because:\n    Matched by include pattern 'src' in '{PROJECT}/tsconfig.json'\n"
    );
    assert_eq!(first, format!("{ts5055}{ts6059}{chain}"), "cycle 1");
    assert_eq!(
        second,
        format!("{ts5055}{ts6059}{chain}{ts6059}"),
        "cycle 2"
    );
}

#[test]
fn build_watch_root_dir_chain_of_a_kept_common_js_file() {
    run_test_in_child(
        "tsctests::explain_files_cache::build_watch_root_dir_chain_of_a_kept_common_js_file",
        || build_watch_root_dir_chain(&[]),
    );
}

#[test]
fn build_watch_root_dir_chain_of_a_kept_common_js_file_single_threaded() {
    run_test_in_child(
        "tsctests::explain_files_cache::build_watch_root_dir_chain_of_a_kept_common_js_file_single_threaded",
        || build_watch_root_dir_chain(&["--singleThreaded"]),
    );
}

/// The `--explainFiles` lines of src/a.ts and src/c.js in `output`.
fn src_explain_lines(output: &str) -> String {
    let mut lines = output.lines().skip_while(|line| *line != "src/a.ts");
    let mut block = String::new();
    let mut in_c = false;
    for line in lines.by_ref() {
        if in_c && !line.starts_with("   ") {
            break;
        }
        in_c |= line == "src/c.js";
        block.push_str(line);
        block.push('\n');
    }
    block
}

/// Input B of the R232 review: `--explainFiles` and `noEmit`. Cycle 1
/// binds before `ExplainFiles`, so both files get the format line. Cycle 2
/// gives a.ts a syntax error, so Go binds nothing, and its new parse of
/// c.js has no CommonJS indicator. The port kept the parse of c.js, bound
/// by cycle 1, and printed the line in cycle 2. The expected lines are
/// Go's (`tsgo-oracle-3e9236361fa9 -b tsconfig.json --watch --explainFiles`).
fn build_watch_explain_files_after_a_syntax_error(extra_args: &[&str]) {
    let mut args = vec![
        "-b",
        "tsconfig.json",
        "--watch",
        "--explainFiles",
        "--pretty",
        "false",
    ];
    args.extend_from_slice(extra_args);
    let [first, second] = build_watch_cycles(
        &[
            ("package.json", "{}\n"),
            ("src/a.ts", "export const a = 1;\n"),
            ("src/c.js", "module.exports = 1;\n"),
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"allowJs":true,"moduleDetection":"legacy","module":"node16","noEmit":true,"types":[]},"include":["src"]}"#,
            ),
        ],
        &args,
        ("src/a.ts", "let q = ;\n"),
    );
    let format = "   File is CommonJS module because 'package.json' does not have field \"type\"\n";
    let matched = "   Matched by include pattern 'src' in 'tsconfig.json'\n";
    assert_eq!(
        src_explain_lines(&first),
        format!("src/a.ts\n{matched}{format}src/c.js\n{matched}{format}"),
        "cycle 1:\n{first}"
    );
    assert!(
        second.starts_with("src/a.ts(1,9): error TS1109: Expression expected.\n"),
        "cycle 2:\n{second}"
    );
    assert_eq!(
        src_explain_lines(&second),
        format!("src/a.ts\n{matched}src/c.js\n{matched}"),
        "cycle 2:\n{second}"
    );
}

#[test]
fn build_watch_explain_files_of_a_kept_common_js_file_after_a_syntax_error() {
    run_test_in_child(
        "tsctests::explain_files_cache::build_watch_explain_files_of_a_kept_common_js_file_after_a_syntax_error",
        || build_watch_explain_files_after_a_syntax_error(&[]),
    );
}

#[test]
fn build_watch_explain_files_of_a_kept_common_js_file_after_a_syntax_error_single_threaded() {
    run_test_in_child(
        "tsctests::explain_files_cache::build_watch_explain_files_of_a_kept_common_js_file_after_a_syntax_error_single_threaded",
        || build_watch_explain_files_after_a_syntax_error(&["--singleThreaded"]),
    );
}

/// `output` without the `--explainFiles` lines of the test system's lib
/// files.
fn without_lib_lines(output: &str) -> String {
    let mut in_lib = false;
    output
        .lines()
        .filter(|line| {
            if !line.starts_with("   ") {
                in_lib = line.contains("/tslibs/");
            }
            !in_lib
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Case L of the loadparity2 Go note check: shared/s.js has only CommonJS
/// syntax and is in projects p1 and p2, outside both `rootDir`s, so TS6059
/// explains it before any bind. With `--singleThreaded` p2 builds after p1
/// in one cycle. Go parses s.js again for p2, so the p2 chain has no
/// CommonJS line. The port gives p2 the parse that p1 bound
/// (`BuildHost::watch_source_file`), and before this program's bind it
/// must not read that bind (`IncludeProcessor::program_bound`). The
/// expected lines are Go's (`tsgo-oracle-3e9236361fa9 -b tsconfig.json
/// --watch --explainFiles --singleThreaded`, edit of p1/src/a.ts).
#[test]
fn build_watch_root_dir_chains_of_a_common_js_file_in_two_projects() {
    run_test_in_child(
        "tsctests::explain_files_cache::build_watch_root_dir_chains_of_a_common_js_file_in_two_projects",
        || {
            let project = r#"{"compilerOptions":{"allowJs":true,"moduleDetection":"legacy","module":"node16","types":[],"rootDir":"src","noEmit":true},"include":["src","../shared"]}"#;
            let [first, second] = build_watch_cycles(
                &[
                    ("package.json", "{}\n"),
                    ("p1/src/a.ts", "export const a = 1;\n"),
                    ("p2/src/b.ts", "export const b = 1;\n"),
                    ("shared/s.js", "module.exports = 1;\n"),
                    ("p1/tsconfig.json", project),
                    ("p2/tsconfig.json", project),
                    (
                        "tsconfig.json",
                        r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#,
                    ),
                ],
                &[
                    "-b",
                    "tsconfig.json",
                    "--watch",
                    "--explainFiles",
                    "--singleThreaded",
                    "--pretty",
                    "false",
                ],
                ("p1/src/a.ts", "export const a = 2;\n"),
            );
            let chain = |p: &str| {
                format!(
                    concat!(
                        "error TS6059: File '{root}/shared/s.js' is not under 'rootDir' '{root}/{p}/src'. 'rootDir' is expected to contain all source files.\n",
                        "  The file is in the program because:\n",
                        "    Matched by include pattern '../shared' in '{root}/{p}/tsconfig.json'\n",
                    ),
                    root = PROJECT,
                    p = p
                )
            };
            let files = |p: &str, ts: &str| {
                format!(
                    concat!(
                        "{p}/src/{ts}\n",
                        "   Matched by include pattern 'src' in '{p}/tsconfig.json'\n",
                        "   File is CommonJS module because 'package.json' does not have field \"type\"\n",
                        "shared/s.js\n",
                        "   Matched by include pattern '../shared' in '{p}/tsconfig.json'\n",
                    ),
                    p = p,
                    ts = ts
                )
            };
            let (p1, p2) = (
                format!("{}{}", chain("p1"), files("p1", "a.ts")),
                format!("{}{}", chain("p2"), files("p2", "b.ts")),
            );
            assert_eq!(without_lib_lines(&first), format!("{p1}{p2}"), "cycle 1");
            assert_eq!(
                without_lib_lines(&second),
                format!("{p1}{}", chain("p2")),
                "cycle 2"
            );
        },
    );
}
