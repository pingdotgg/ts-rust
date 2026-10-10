//! Port-only test of the name order of `some_property_reduces_to_never`
//! (Go `checker/checker.go:22216 somePropertyReducesToNever`).
//!
//! PORT: no Go counterpart. Since ts#64521 (Go N' checker.go:22283) Go
//! tests the shared names of an intersection in first-seen order and stops
//! at the first one that reduces to never. Each name that it tests before
//! that one makes a synthetic property, and `--extendedDiagnostics` counts
//! it. Here `A & B` reduces to never by `kind`, and the 30 methods of `A`
//! and `B` have different types, so each method tested before `kind` makes
//! one symbol, and the methods' types are made. With `kind` after the
//! methods, the Go N' oracle (tsgo-oracle-fed0bf24149f, this text in a
//! project of its own) prints 30 more symbols and 90 more types: Symbols
//! 59,957 against 59,927, Types 34,981 against 34,891.
//!
//! A conflicting private method declared first is found before the
//! properties are tested, and one declared last after them. That holds for
//! a private static method that shares its name with a public instance
//! method too; that test also checks Go's error text, which names the
//! private `m`. The Go N' oracle gives the same differences (30 symbols).

use crate::support::child::run_command_in_child;
use crate::support::runner::TscInput;
use crate::support::test_sys::new_test_sys;

const PROJECT: &str = "/home/src/workspaces/project";

/// The `Symbols` and `Types` counts of `tsc --extendedDiagnostics` on
/// `A & B`, with `kind` declared before or after the methods.
fn counts(kind_first: bool) -> (u64, u64) {
    let methods =
        |ret: &str| -> String { (0..30).map(|i| format!("    m{i}(): {ret};\n")).collect() };
    let interface = |name: &str, ret: &str, kind: &str| {
        let kind = format!("    kind: \"{kind}\";\n");
        if kind_first {
            format!("interface {name} {{\n{kind}{}}}\n", methods(ret))
        } else {
            format!("interface {name} {{\n{}{kind}}}\n", methods(ret))
        }
    };
    let text = format!(
        "{}{}declare const ab: A & B;\nexport const n: number = ab;\n",
        interface("A", "string", "a"),
        interface("B", "number", "b")
    );
    let output = output_of(text);
    (count_of(&output, "Symbols:"), count_of(&output, "Types:"))
}

/// The output of `tsc --extendedDiagnostics --pretty false` on the project
/// with the one file `a.ts`.
fn output_of(text: String) -> String {
    let input = TscInput {
        files: [
            (format!("{PROJECT}/a.ts"), text.into()),
            (
                format!("{PROJECT}/tsconfig.json"),
                r#"{"compilerOptions":{"noEmit":true},"files":["a.ts"]}"#.into(),
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = [
        "-p",
        "tsconfig.json",
        "--extendedDiagnostics",
        "--pretty",
        "false",
    ]
    .map(String::from);
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    sys.output_text()
}

#[test]
fn never_intersection_checks_names_in_first_seen_order() {
    let (first_symbols, first_types) = counts(true);
    let (last_symbols, last_types) = counts(false);
    assert_eq!(
        (last_symbols - first_symbols, last_types - first_types),
        (30, 90),
        "kind declared last, then first"
    );
}

/// The `Symbols` count on `A & B` of two classes whose `private m()` is
/// declared before or after 30 properties of different types.
fn private_symbols(method_first: bool) -> u64 {
    let class = |name: &str, ty: &str| {
        let method = "    private m(): void {}\n";
        let fields: String = (0..30).map(|i| format!("    f{i}!: {ty};\n")).collect();
        if method_first {
            format!("class {name} {{\n{method}{fields}}}\n")
        } else {
            format!("class {name} {{\n{fields}{method}}}\n")
        }
    };
    let text = format!(
        "{}{}declare const ab: A & B;\nexport const n: number = ab;\n",
        class("A", "string"),
        class("B", "number")
    );
    count_of(&output_of(text), "Symbols:")
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
fn never_intersection_checks_private_methods_with_properties() {
    // Each property checked before `m` makes one synthetic property.
    assert_eq!(private_symbols(false) - private_symbols(true), 30);
}

/// The `Symbols` count of `tsc --extendedDiagnostics` on `(A & typeof A).m`
/// of a class with a public `m()`, a `private static m()` and 30 names that
/// are an instance and a static property of different types, with the
/// methods declared before or after the properties. The output must have
/// Go's error (tsgo-oracle-673a5f17d713 and tsgo-oracle-fed0bf24149f, the
/// same for both orders): the intersection is never because of `m`.
fn private_static_symbols(methods_first: bool) -> u64 {
    let methods = "    m(): void {}\n    private static m(): void {}\n";
    let fields: String = (0..30)
        .map(|i| format!("    f{i}!: string;\n    static f{i}: number = 0;\n"))
        .collect();
    let body = if methods_first {
        format!("{methods}{fields}")
    } else {
        format!("{fields}{methods}")
    };
    let output = output_of(format!(
        "class A {{\n{body}}}\ndeclare const a: A & typeof A;\nexport const n: number = a.m;\n"
    ));
    let error = "a.ts(66,28): error TS2339: Property 'm' does not exist on type 'never'.\n  \
                 The intersection 'A & typeof A' was reduced to 'never' because property 'm' \
                 exists in multiple constituents and is private in some.\n";
    assert!(
        output.contains(error),
        "methods first {methods_first}:\n{output}"
    );
    count_of(&output, "Symbols:")
}

#[test]
fn never_intersection_checks_a_private_static_method_with_properties() {
    // The instance `m` and the static `m` have different declarations, and
    // the static `m` is private, so `m` is a conflicting private member.
    // Each property checked before it makes one synthetic property.
    assert_eq!(
        private_static_symbols(false) - private_static_symbols(true),
        30
    );
}
