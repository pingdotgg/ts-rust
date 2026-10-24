//! Port-only test of the slots of identity matching in inference
//! (identmatch1, `Checker::normalize_for_matching` in
//! checker/inference_p2.rs).
//!
//! PORT: no Go counterpart. Go's inferFromMatchingTypes
//! (checker/inference.go:381) normalizes both members of a pair again at
//! every pair (getNormalizedType, relater.go:2654). The port keeps a member's
//! normalized type between the pairs of one call. getIntersectionType reads
//! IsEmptyAnonymousObjectType (checker.go:27077), which reads MembersResolved
//! and does not resolve. So with `X = typeof import("./types")` (a module
//! with only type exports), `getIntersectionType([X, T])` gives `X & T`
//! before the members of `X` are resolved and `T` after. In each case, `pre`
//! makes `X & T` while `X` is not resolved. The first normalization of the
//! source member `T` (substitute `X`) finds `X & T` and resolves `X` (empty)
//! in getReducedType. Go's next normalizations of that member give `T`, and
//! the pair with the target member `T` matches. A slot that kept `X & T` did
//! not match there (R230, identmatch1 rounds 1 and 2), and the inferred `U`
//! got one more `T`.
//!
//! Cases c1 (union site, isTypeOrBaseIdenticalTo), c6 (intersection site,
//! isTypeIdenticalTo) and c7 (union site, the call succeeds) of the R230
//! reviewer. Every expected output is the output of `tsgo-oracle-3e9236361fa9
//! -p tsconfig.json --pretty false` on the same files in
//! /home/src/workspaces/project, the same with and without
//! `--singleThreaded`.

use ts_goport::execute::tsc::ExitStatus;

use crate::support::child::run_command_in_child;
use crate::support::runner::TscInput;
use crate::support::test_sys::new_test_sys;

const PROJECT: &str = "/home/src/workspaces/project";

const TYPES: &str = "export type Nothing = 1;\n";

const TSCONFIG: &str = r#"{ "compilerOptions": { "strict": true, "noEmit": true, "module": "esnext", "target": "esnext", "moduleResolution": "bundler" }, "files": ["main.ts", "types.ts"] }
"#;

/// Runs `tsgo -p tsconfig.json --pretty false` (and `--singleThreaded` when
/// `single`) on `main` and `types.ts` in the project dir and gives the
/// diagnostics text.
fn check(main: &str, single: bool) -> String {
    let input = TscInput {
        files: [
            (format!("{PROJECT}/main.ts"), main.into()),
            (format!("{PROJECT}/types.ts"), TYPES.into()),
            (format!("{PROJECT}/tsconfig.json"), TSCONFIG.into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let mut args = vec!["-p", "tsconfig.json", "--pretty", "false"];
    if single {
        args.push("--singleThreaded");
    }
    let args: Vec<String> = args.into_iter().map(String::from).collect();
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    assert_eq!(
        result.status,
        ExitStatus::DiagnosticsPresentOutputsGenerated
    );
    // The test system adds the list of files after the diagnostics.
    let output = sys.output_text();
    output
        .split("!!! List files start")
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Checks `main` in both modes against Go's text.
fn assert_go_text(main: &str, want: &str) {
    for single in [false, true] {
        assert_eq!(check(main, single), want, "singleThreaded {single}");
    }
}

#[test]
fn identity_matching_sees_an_empty_resolve_at_the_union_site() {
    let main = r#"type X = typeof import("./types");

function g<T extends {}, V>() {
    let pre!: X & T;
    let src!: [T] extends [X] ? T | number : 0;
    let h!: <U>(x: V extends 0 ? U | T | string : 1) => U;
    const r: boolean = h(src);
}
"#;
    assert_go_text(
        main,
        r#"main.ts(7,11): error TS2322: Type 'number' is not assignable to type 'boolean'.
main.ts(7,26): error TS2345: Argument of type '[T] extends [typeof import("/home/src/workspaces/project/types")] ? number | T : 0' is not assignable to parameter of type 'V extends 0 ? string | number | T : 1'.
  Type 'number | T' is not assignable to type 'V extends 0 ? string | number | T : 1'.
    Type 'number' is not assignable to type 'V extends 0 ? string | number | T : 1'.
"#,
    );
}

#[test]
fn identity_matching_sees_an_empty_resolve_at_the_intersection_site() {
    let main = r#"type X = typeof import("./types");
interface Z { z: 1 }
interface W { w: 1 }
declare const z: Z;
declare const w: W;

function g<T extends {}, V>() {
    let pre!: X & T;
    let src!: [T] extends [X] ? Z & W & T : 0;
    let h!: <U>(x: V extends 0 ? Z & T & U : 1) => U;
    const r = h(src);
    const r2: boolean = r;
}
"#;
    assert_go_text(
        main,
        r#"main.ts(11,17): error TS2345: Argument of type '[T] extends [typeof import("/home/src/workspaces/project/types")] ? Z & W & T : 0' is not assignable to parameter of type 'V extends 0 ? Z & T & W : 1'.
  Type '0 | (Z & W & T)' is not assignable to type 'V extends 0 ? Z & T & W : 1'.
    Type '0' is not assignable to type 'V extends 0 ? Z & T & W : 1'.
main.ts(12,11): error TS2322: Type 'W' is not assignable to type 'boolean'.
"#,
    );
}

#[test]
fn identity_matching_sees_an_empty_resolve_in_the_inferred_type() {
    let main = r#"type X = typeof import("./types");

function g<T extends {}, V extends T>() {
    let pre!: X & T;
    let src!: [T] extends [X] ? T | number : 0;
    let h!: <U>(x: [V] extends [X] ? U | T | string : 0) => U;
    const r = h(src);
    const r2: boolean = r;
}
"#;
    assert_go_text(
        main,
        "main.ts(8,11): error TS2322: Type 'number' is not assignable to type 'boolean'.\n",
    );
}
