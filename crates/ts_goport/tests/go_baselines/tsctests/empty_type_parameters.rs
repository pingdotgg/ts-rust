//! Port-only test of an empty type parameter list `<>`.
//!
//! PORT: no Go counterpart. Go `(*Node).TypeParameters()` (ast/ast.go:522)
//! gives the Nodes of the list, and the Go parser stores nil Nodes for an
//! empty list (`Arena.Clone` of an empty slice is nil, core/arena.go:55). So
//! `TypeParameters() == nil` holds for `f<>()`. The reparser
//! (parser/reparser.go:459) read the list instead: a JSDoc `@template` did
//! not replace the empty list of a function, so the file got TS1098 and the
//! template names were not found.
//!
//! Every expected output is the output of `tsgo-oracle-3e9236361fa9 -p
//! tsconfig.json --pretty false` on the same files.

use ts_goport::execute::tsc::ExitStatus;

use crate::support::child::run_command_in_child;
use crate::support::runner::TscInput;
use crate::support::test_sys::{TestSys, new_test_sys};

const PROJECT: &str = "/home/src/workspaces/project";

const CHECK_JS: &str =
    r#"{"compilerOptions":{"noEmit":true,"allowJs":true,"checkJs":true},"files":["a.js"]}"#;

/// Runs `tsgo -p tsconfig.json --pretty false` on `file` (`a.js` or `a.ts`)
/// with `tsconfig` and gives the system, the status and the diagnostics text.
fn tsc(file: &str, text: &str, tsconfig: &str) -> (TestSys, ExitStatus, String) {
    let input = TscInput {
        files: [
            (format!("{PROJECT}/{file}"), text.into()),
            (format!("{PROJECT}/tsconfig.json"), tsconfig.into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = ["-p", "tsconfig.json", "--pretty", "false"].map(String::from);
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    // The test system adds the list of files after the diagnostics.
    let output = sys.output_text();
    let diagnostics = output
        .split("!!! List files start")
        .next()
        .unwrap_or_default()
        .to_string();
    (sys, result.status, diagnostics)
}

/// The fuzzer's repro, and `<>` with a space or a comment inside.
#[test]
fn template_tag_replaces_an_empty_type_parameter_list() {
    for text in [
        "/** @template T */\nfunction f<>() {}\n",
        "/** @template T */\nfunction f< >() {}\n/** @template T */\nfunction g</* c */>() {}\n",
    ] {
        let (_, status, diagnostics) = tsc("a.js", text, CHECK_JS);
        assert_eq!(diagnostics, "", "{text}");
        assert_eq!(status, ExitStatus::Success, "{text}");
    }
}

/// The template name is found (no TS2304) in a return tag, a method, a
/// function expression and a parameter tag.
#[test]
fn template_tag_names_are_found_with_an_empty_list() {
    let cases = [
        (
            "/**\n * @template T\n * @returns {T}\n */\nfunction f<>() {}\n",
            "a.js(3,14): error TS2355: A function whose declared type is neither 'undefined', 'void', nor 'any' must return a value.\n",
        ),
        (
            r#"class C {
  /** @template T */
  m<>() {}
}
const g = /** @template T */ function <>() {};
/** @template T @param {T} x @returns {T} */
function h<>(x) { return x; }
/** @type {number} */
const n = h("s");
"#,
            "a.js(9,7): error TS2322: Type 'string' is not assignable to type 'number'.\n",
        ),
    ];
    for (text, want) in cases {
        let (_, status, diagnostics) = tsc("a.js", text, CHECK_JS);
        assert_eq!(diagnostics, want, "{text}");
        assert_eq!(
            status,
            ExitStatus::DiagnosticsPresentOutputsGenerated,
            "{text}"
        );
    }
}

/// The declaration emit has the template type parameters.
#[test]
fn template_tag_type_parameters_are_emitted_for_an_empty_list() {
    let (sys, status, diagnostics) = tsc(
        "a.js",
        "/** @template T */\nfunction f<>() {}\n/** @template T @param {T} x */\nfunction g< >(x) {}\n",
        r#"{"compilerOptions":{"allowJs":true,"declaration":true,"emitDeclarationOnly":true,"outDir":"out"},"files":["a.js"]}"#,
    );
    assert_eq!(diagnostics, "");
    assert_eq!(status, ExitStatus::Success);
    assert_eq!(
        sys.read_file_no_error(&format!("{PROJECT}/out/a.d.ts")),
        "/** @template T */\ndeclare function f<T>(): void;\n/** @template T @param {T} x */\ndeclare function g<T>(x: T): void;\n"
    );
}
