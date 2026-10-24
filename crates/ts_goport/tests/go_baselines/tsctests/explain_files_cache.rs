//! Port-only test of the `--explainFiles` lines that tell a file's module
//! format (Go `compiler/includeprocessor.go:123
//! explainRedirectAndImpliedFormat`).
//!
//! PORT: no Go counterpart (projfuzz1 EXPLAIN_ABS). Go caches these lines
//! per file, and the first caller's names win: `ExplainFiles` uses names
//! relative to the cwd (`program.go:2120`), and the include processor
//! diagnostics use absolute names (`processingDiagnostic.go:106`). After a
//! syntax error Go never builds those diagnostics (`program.go:2031`), so
//! the names are relative. The port builds them when it makes the program,
//! and it printed the absolute name. The expected lines are Go N's.

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
