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
//! The expected lines of the first test are Go N's, the others Go's at pin
//! 3e9236361fa9.

use ts_goport::execute::tsc::ExitStatus;

use crate::support::child::run_command_in_child;
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
