//! Port-only tests of two `--extendedDiagnostics` counts that the port had
//! wrong (bump D int4 corrections). Each expected count is what the Go N'
//! oracle (tsgo-oracle-fed0bf24149f) prints for the same files and
//! `tsconfig.json` in a project of their own, with `--singleThreaded`. The
//! project has the real bundled libs, not the fake test lib, because both
//! counts come from lib declarations.
//!
//! - t3types1 (T3 server Types): `typeReferenceToTypeNode`
//!   (checker/nodebuilderimpl.go:3198) runs the 4 global `Iterable` lookups
//!   when `typeParams != nil`. `InterfaceType.TypeParameters()`
//!   (checker/types.go:1046) is nil only when `allTypeParameters` is empty,
//!   so a reference to a class with only its `this` type runs them too. The
//!   first lookup of each interface makes 5 types: the interface, `T`,
//!   `TReturn`, `TNext` and its `this` type. The port tested an empty list
//!   and made 20 types fewer.
//! - typeormsym1 (typeorm Symbols): `collectDiagnosticsFromFiles`
//!   (compiler/program.go:702) queues one declaration diagnostics job per
//!   file, and with `--singleThreaded` the work group runs the last queued
//!   job first (core/workgroup.go:67, pop :78). The file that first
//!   instantiates a conditional type decides whether `instantiateSymbol`
//!   makes a new symbol for its `type` property. The port ran the files in
//!   order and made 1 symbol more or fewer.

use ts_goport::frontend::bundled;

use crate::support::child::run_command_in_child;
use crate::support::runner::TscInput;
use crate::support::test_sys::{get_test_lib_path_for, new_test_sys};

const PROJECT: &str = "/home/src/workspaces/project";

/// Adds the bundled text of lib `name`, and of each lib that it references,
/// at the test lib paths. The test system writes its fake lib only where no
/// file is.
fn add_real_lib(name: &str, files: &mut Vec<(String, String)>) {
    let path = get_test_lib_path_for(name);
    if files.iter().any(|(added, _)| *added == path) {
        return;
    }
    let file = path.rsplit('/').next().expect("a lib file name");
    let text = bundled::bundled_text(&format!("{}/{file}", bundled::lib_path()))
        .unwrap_or_else(|| panic!("no bundled {file}"));
    files.push((path, text.to_string()));
    for reference in text
        .lines()
        .filter_map(|line| line.strip_prefix("/// <reference lib=\""))
    {
        let referenced = reference.split('"').next().expect("a lib name");
        add_real_lib(referenced, files);
    }
}

/// `Symbols`, `Types` and `Instantiations` of `tsc -p tsconfig.json
/// --singleThreaded --extendedDiagnostics --pretty false` on `files` (names
/// in the project) with the real lib `lib`, and the output.
fn counts(lib: &str, files: &[(&str, &str)]) -> ([u64; 3], String) {
    let mut all = Vec::new();
    add_real_lib(lib, &mut all);
    all.extend(
        files
            .iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), (*text).to_string())),
    );
    let input = TscInput {
        files: all
            .into_iter()
            .map(|(path, text)| (path, text.into()))
            .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = [
        "-p",
        "tsconfig.json",
        "--singleThreaded",
        "--extendedDiagnostics",
        "--pretty",
        "false",
    ]
    .map(String::from);
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    let output = sys.output_text();
    let counts = ["Symbols:", "Types:", "Instantiations:"].map(|name| count_of(&output, name));
    (counts, output)
}

/// The count of the `name` line (`"Symbols:"`, `"Types:"`) in a
/// `tsc --extendedDiagnostics` output.
fn count_of(output: &str, name: &str) -> u64 {
    let count = output
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .unwrap_or_else(|| panic!("no {name} line in:\n{output}"));
    count
        .trim()
        .parse()
        .unwrap_or_else(|err| panic!("{count:?}: {err}"))
}

#[test]
fn a_constraint_error_on_a_this_only_class_resolves_the_iterable_globals() {
    // t3types1 repro r1: the error prints the constraint `C`, a reference
    // to `C` with its `this` type as the one type argument.
    let (counts, output) = counts(
        "es2018",
        &[
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"strict":true,"noEmit":true,"skipLibCheck":true,"types":[],"lib":["es2018"]},"files":["a.ts"]}"#,
            ),
            (
                "a.ts",
                "class C { x = 1; }\ndeclare function f<T extends C>(): void;\nf<unknown>();\n",
            ),
        ],
    );
    assert!(
        output.contains(
            "a.ts(3,3): error TS2344: Type 'unknown' does not satisfy the constraint 'C'.\n"
        ),
        "{output}"
    );
    // Go N': Types 114. The port made 94 before the correction.
    assert_eq!(counts, [4628, 114, 0], "Symbols, Types, Instantiations");
}

#[test]
fn an_unreported_constraint_error_resolves_the_iterable_globals() {
    // t3types1 repro r3, the T3 server path: the inference of `h()` gets the
    // constraint of `U`, and `typeof g<unknown>` fails the constraint of
    // `g`. The relation error in `types.d.ts` is not reported
    // (skipLibCheck), but it prints `C`.
    let (counts, output) = counts(
        "es2018",
        &[
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"strict":true,"noEmit":true,"skipLibCheck":true,"types":[],"lib":["es2018"],"module":"esnext"},"files":["types.d.ts","a.ts"]}"#,
            ),
            (
                "types.d.ts",
                "declare class C { x: number }\n\
                 declare function g<T extends C>(): void;\n\
                 export declare function h<U extends typeof g<unknown>>(u?: U): void;\n",
            ),
            ("a.ts", "import { h } from \"./types\";\nh();\n"),
        ],
    );
    assert!(!output.contains("error TS"), "{output}");
    // Go N': Types 112. The port made 92 before the correction.
    assert_eq!(counts, [4637, 112, 1], "Symbols, Types, Instantiations");
}

#[test]
fn single_threaded_declaration_diagnostics_run_the_last_file_first() {
    // typeormsym1 repro r1 and r1-swap. `declaration` with `noEmit` and no
    // errors runs declaration diagnostics. When `b.ts` runs first, its
    // object's `type` property makes inference resolve the `type` property
    // of the conditional type's object type, and the later instantiation
    // from `a.ts` reuses it. When `a.ts` runs first, `instantiateSymbol`
    // makes a new symbol for it.
    let config = |files: &str| {
        format!(
            r#"{{"compilerOptions":{{"declaration":true,"noEmit":true,"strict":true,"target":"es2022","module":"commonjs","types":[],"skipLibCheck":true,"lib":["es5"]}},"files":[{files}]}}"#
        )
    };
    let symbols = |files: &str| {
        let config = config(files);
        let (counts, output) = counts(
            "es5",
            &[
                ("tsconfig.json", &config),
                (
                    "types.d.ts",
                    "export type T<O> = O extends { type: \"count\"; default: infer D } ? D : 0;\n\
                     export declare function opt<O>(o: O): { [K in \"x\"]: T<O> };\n",
                ),
                (
                    "a.ts",
                    "import { opt } from \"./types\";\nexport const a = opt({ alias: \"d\" });\n",
                ),
                (
                    "b.ts",
                    "import { opt } from \"./types\";\nexport const b = opt({ type: \"string\" });\n",
                ),
            ],
        );
        assert!(!output.contains("error TS"), "{output}");
        counts
    };
    // Go N': with the files in order types.d.ts, a.ts, b.ts Go runs b.ts
    // first (3119 Symbols), and with b.ts before a.ts it runs a.ts first
    // (3120). The port gave 3120 and 3119 before the correction.
    assert_eq!(
        [
            symbols(r#""types.d.ts","a.ts","b.ts""#),
            symbols(r#""types.d.ts","b.ts","a.ts""#),
        ],
        [[3119, 114, 24], [3120, 114, 24]],
        "Symbols, Types, Instantiations with a.ts first, then with b.ts first"
    );
}
