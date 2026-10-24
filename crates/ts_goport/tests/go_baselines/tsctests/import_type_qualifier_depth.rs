//! Port-only tests of the import type qualifier test in the emit walk of
//! `mark_linked_references` (Go `checker/checker.go:28935`, ts#64636).
//!
//! PORT: no Go counterpart. Go's tests of ts#64636
//! (`importElisionTypeofImportTypeQualifier`,
//! `noFlakyDiagnosticsTypeofImportTypeQualifier1` and `2`) have one
//! QualifiedName in the qualifier. The port asks `isImportTypeQualifierPart`
//! only at the top of a QualifiedName chain (deepqual1), so these tests use
//! qualifiers of 2 and 3 QualifiedNames. With `--noCheck` the emit resolver
//! resolves the names itself: Go before ts#64636 (pin N 673a5f17d713) keeps
//! `import { ns } from "./c"` there. Each expected output is the Go
//! 3e9236361fa9 oracle's on the same files, with the project directory
//! changed to this test's.

use ts_goport::execute::tsc::ExitStatus;

use crate::support::child::run_command_in_child;
use crate::support::runner::TscInput;
use crate::support::test_sys::new_test_sys;

const PROJECT: &str = "/home/src/workspaces/project";

/// The diagnostics, `out/a.js` and `out/a.d.ts` of `tsc -p tsconfig.json
/// --pretty false` (and `--noCheck` when `no_check`) on the files, with
/// `a.ts` as the root.
fn run(files: &[(&str, &str)], module: &str, no_check: bool) -> (String, String, String) {
    let config = format!(
        r#"{{"compilerOptions":{{"strict":true,"target":"es2022","module":"{module}","declaration":true,"types":[],"outDir":"out"}},"files":["a.ts"]}}"#
    );
    let input = TscInput {
        files: files
            .iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), (*text).into()))
            .chain([(format!("{PROJECT}/tsconfig.json"), config.into())])
            .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let mut args = vec!["-p", "tsconfig.json", "--pretty", "false"];
    if no_check {
        args.push("--noCheck");
    }
    let args = args.into_iter().map(String::from).collect::<Vec<_>>();
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    // The test system adds the list of files after the diagnostics.
    let output = sys.output_text();
    let diagnostics = output.split("!!! List files start").next().unwrap_or("");
    let want_status = if diagnostics.is_empty() {
        ExitStatus::Success
    } else {
        ExitStatus::DiagnosticsPresentOutputsGenerated
    };
    assert_eq!(result.status, want_status, "{diagnostics}");
    (
        diagnostics.to_string(),
        sys.read_file_no_error(&format!("{PROJECT}/out/a.js")),
        sys.read_file_no_error(&format!("{PROJECT}/out/a.d.ts")),
    )
}

const ELIDE_A: &str = r#"import { ns } from "./c";
export function f(x: typeof import("./b").ns.z.w.y) {}
export function g(x: typeof import("./b").ns.z.v) {}
export type T = import("./b").ns.z.w.I;
"#;
const ELIDE_B: &str = "export namespace ns { export namespace z { export const v = 0; export namespace w { export const y = 1; export interface I { a: number } } } }\n";
const ELIDE_C: &str = "export namespace ns { export namespace z { export const v = 2; export namespace w { export const y = 3; export interface I { b: string } } } }\n";
const ELIDE_D_TS: &str = r#"export declare function f(x: typeof import("./b").ns.z.w.y): void;
export declare function g(x: typeof import("./b").ns.z.v): void;
export type T = import("./b").ns.z.w.I;
"#;

/// `ns` in `import("./b").ns.z.w.y` is an export of `./b`, not the local
/// `ns`: the import of `./c` is elided at every qualifier depth.
#[test]
fn deep_import_type_qualifiers_do_not_keep_a_local_import() {
    let files = [("a.ts", ELIDE_A), ("b.ts", ELIDE_B), ("c.ts", ELIDE_C)];
    let cases = [
        (
            "commonjs",
            "\"use strict\";\nObject.defineProperty(exports, \"__esModule\", { value: true });\nexports.f = f;\nexports.g = g;\nfunction f(x) { }\nfunction g(x) { }\n",
        ),
        (
            "esnext",
            "export function f(x) { }\nexport function g(x) { }\n",
        ),
    ];
    for (module, want_js) in cases {
        for no_check in [false, true] {
            let (diagnostics, js, dts) = run(&files, module, no_check);
            assert_eq!(diagnostics, "", "{module} noCheck {no_check}");
            assert_eq!(js, want_js, "{module} noCheck {no_check}");
            assert_eq!(dts, ELIDE_D_TS, "{module} noCheck {no_check}");
        }
    }
}

/// Names that the imported module lacks, at depth 2 and 3: one error each
/// with a full check and none with `--noCheck`.
#[test]
fn deep_import_type_qualifiers_with_missing_names() {
    let a = "export function f(x: typeof import(\"./b\").nope.a.b) {}\nexport function h(x: typeof import(\"./b\").x.q.r) {}\n";
    let files = [("a.ts", a), ("b.ts", "export const x = 1;\n")];
    let want_diagnostics = format!(
        "a.ts(1,43): error TS2694: Namespace '\"{PROJECT}/b\"' has no exported member 'nope'.\na.ts(2,45): error TS2694: Namespace '\"{PROJECT}/b\".x' has no exported member 'q'.\n"
    );
    let want_js = "\"use strict\";\nObject.defineProperty(exports, \"__esModule\", { value: true });\nexports.f = f;\nexports.h = h;\nfunction f(x) { }\nfunction h(x) { }\n";
    let want_dts = "export declare function f(x: typeof import(\"./b\").nope.a.b): void;\nexport declare function h(x: typeof import(\"./b\").x.q.r): void;\n";
    for no_check in [false, true] {
        let (diagnostics, js, dts) = run(&files, "commonjs", no_check);
        let want = if no_check { "" } else { &want_diagnostics };
        assert_eq!(diagnostics, want, "noCheck {no_check}");
        assert_eq!(js, want_js, "noCheck {no_check}");
        assert_eq!(dts, want_dts, "noCheck {no_check}");
    }
}

/// A module that does not resolve, with a qualifier of depth 2.
#[test]
fn deep_import_type_qualifier_of_an_unresolved_module() {
    let a = "export function f(x: typeof import(\"foo\").sys.a.readDirectory) {}\n";
    let want_js = "\"use strict\";\nObject.defineProperty(exports, \"__esModule\", { value: true });\nexports.f = f;\nfunction f(x) { }\n";
    let want_dts =
        "export declare function f(x: typeof import(\"foo\").sys.a.readDirectory): void;\n";
    for no_check in [false, true] {
        let (diagnostics, js, dts) = run(&[("a.ts", a)], "commonjs", no_check);
        let want = if no_check {
            ""
        } else {
            "a.ts(1,36): error TS2307: Cannot find module 'foo' or its corresponding type declarations.\n"
        };
        assert_eq!(diagnostics, want, "noCheck {no_check}");
        assert_eq!(js, want_js, "noCheck {no_check}");
        assert_eq!(dts, want_dts, "noCheck {no_check}");
    }
}

/// Local qualified names of depth 3 that are not in an import type: the
/// alias of an import-equals and of a value access are kept, and a
/// type-only one is elided.
#[test]
fn deep_local_qualified_names_keep_their_aliases() {
    let a = r#"import { n1 } from "./b";
import { n1 as k1 } from "./b";
import { n1 as m1 } from "./b";
import alias = k1.n2.n3;
export let q: typeof n1.n2.n3.v = 1;
export let r: n1.n2.n3.I | undefined;
export const s = alias.v;
export const t = m1.n2.n3.v;
"#;
    let b = "export namespace n1 { export namespace n2 { export namespace n3 { export const v = 1; export interface I {} } } }\n";
    let want_js = r#""use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.t = exports.s = exports.r = exports.q = void 0;
const b_1 = require("./b");
const b_2 = require("./b");
var alias = b_1.n1.n2.n3;
exports.q = 1;
exports.s = alias.v;
exports.t = b_2.n1.n2.n3.v;
"#;
    let want_dts = r#"import { n1 } from "./b";
export declare let q: typeof n1.n2.n3.v;
export declare let r: n1.n2.n3.I | undefined;
export declare const s = 1;
export declare const t = 1;
"#;
    for no_check in [false, true] {
        let (diagnostics, js, dts) = run(&[("a.ts", a), ("b.ts", b)], "commonjs", no_check);
        assert_eq!(diagnostics, "", "noCheck {no_check}");
        assert_eq!(js, want_js, "noCheck {no_check}");
        assert_eq!(dts, want_dts, "noCheck {no_check}");
    }
}
