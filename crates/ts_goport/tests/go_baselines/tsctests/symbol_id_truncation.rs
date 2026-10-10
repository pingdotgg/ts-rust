//! Port-only test of the symbol ids that late-bound property names hold
//! (trunc1, from the infmemo1 skeptic fuzz case g106-36).
//!
//! Go gives a symbol its id on first use (`ast.GetSymbolId`): each read of
//! `valueSymbolLinks` (checker/links.go:33) and of a few node builder maps.
//! The checkers of a pool share one counter, and each `NewChecker` gives 4
//! checker symbols their ids before the first check
//! (checker/checker.go:1355 initializeChecker). The property name of a
//! unique symbol holds its id (`<prefix>@k4@<id>`, checker/checker.go:23402
//! getESSymbolLikeTypeForNode), and the node builder counts the length of
//! that name toward truncation (checker/nodebuilderimpl.go:2614
//! addPropertyToElementList). So the digits of the id move where
//! `... N more ...` starts (`ValueSymbolLinkStore`,
//! `program::new_pool_checker`).
//!
//! Here `k4` has id 13 with the 2 checkers of the default pool (8 ids from
//! `NewChecker`, then `a0` to `a3` and `k4`) and id 9 with `--checkers 1`.
//! `skipLibCheck` leaves `globals.d.ts` unchecked, so only the checker of
//! `a.ts` gives ids after the pool is made, and the Go ids do not race. The
//! expected texts are the output of `tsgo-oracle-673a5f17d713 -p
//! tsconfig.json --pretty false` (with and without `--checkers 1`) on the
//! same files.
//!
//! followups38 adds the ids that `NewChecker` gives to binder symbols (merge
//! errors), which Go gives once in a pool, and the programs of one process
//! with `--singleThreaded` (`tsc -b`, watch): Go's counter runs on from one
//! program to the next (`program::CheckerPool::carry_symbol_ids`).

use ts_goport::execute::tsc::ExitStatus;
use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::context;

use crate::support::child::{
    command_line_in_process, new_in_process_test_sys, run_command_in_child, run_test_in_child,
};
use crate::support::runner::TscInput;
use crate::support::test_sys::new_test_sys;

const PROJECT: &str = "/home/src/workspaces/project";

const GLOBALS: &str = "interface Array<T> { length: number; [n: number]: T }
interface Boolean {}
interface CallableFunction {}
interface Function {}
interface IArguments {}
interface NewableFunction {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
";

/// The members `w0` to `w{count - 1}`, each with a space after it.
fn members(count: usize) -> String {
    (0..count).map(|i| format!("w{i}: number; ")).collect()
}

/// The `a.ts` of every test: `k4` follows 4 other symbols with ids, and a
/// name of `pad` letters `a` pads the type text.
fn a_ts(pad: usize) -> String {
    format!(
        "declare const a0: number;
declare const a1: number;
declare const a2: number;
declare const a3: number;
declare const k4: unique symbol;
declare const x: {{ [k4]: void; {}: number; {}q: string }};
const n: number = x;
",
        "a".repeat(pad),
        members(20)
    )
}

/// The diagnostics of `tsc -p tsconfig.json --pretty false` plus `extra`
/// on `files` (name and text) and `a_ts(pad)`, in that order.
fn check_files(files: &[(&str, String)], pad: usize, extra: &[&str]) -> String {
    let names: Vec<String> = files
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .chain(["\"a.ts\"".to_string()])
        .collect();
    let tsconfig = format!(
        r#"{{"compilerOptions":{{"noLib":true,"skipLibCheck":true,"strict":true,"noEmit":true}},"files":[{}]}}"#,
        names.join(",")
    );
    let mut all = files.to_vec();
    all.push(("a.ts", a_ts(pad)));
    all.push(("tsconfig.json", tsconfig));
    tsc_p(&all, extra)
}

/// The diagnostics of `tsc -p tsconfig.json --pretty false` plus `extra`
/// on `files` (name and text; `tsconfig.json` is one of them).
fn tsc_p(files: &[(&str, String)], extra: &[&str]) -> String {
    let input = TscInput {
        files: files
            .iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), text.clone().into()))
            .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let mut args: Vec<String> = ["-p", "tsconfig.json", "--pretty", "false"]
        .map(String::from)
        .to_vec();
    args.extend(extra.iter().map(|arg| arg.to_string()));
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

/// The diagnostics of `tsc -p tsconfig.json --pretty false` plus `extra`.
fn check(extra: &[&str]) -> String {
    check_files(&[("globals.d.ts", GLOBALS.into())], 9, extra)
}

/// Go's error with the members up to `w{shown - 1}` and `more` hidden.
fn expected(shown: usize, more: usize) -> String {
    expected_with_pad(9, shown, more)
}

/// `expected` for `a_ts(pad)`.
fn expected_with_pad(pad: usize, shown: usize, more: usize) -> String {
    format!(
        "a.ts(7,7): error TS2322: Type '{{ [k4]: void; {}: number; {}... {more} more ...; q: string; }}' is not assignable to type 'number'.\n",
        "a".repeat(pad),
        members(shown)
    )
}

#[test]
fn unique_symbol_ids_count_as_go_in_the_default_pool() {
    // Id 13: the name is one byte longer than with id 9, so w14 goes.
    assert_eq!(check(&[]), expected(14, 6));
}

#[test]
fn unique_symbol_ids_count_as_go_with_one_checker() {
    assert_eq!(check(&["--checkers", "1"]), expected(15, 5));
}

/// `declare let d0: number;` to `d{count - 1}`, one per line.
fn lets(count: usize) -> String {
    (0..count)
        .map(|i| format!("declare let d{i}: number;\n"))
        .collect()
}

#[test]
fn pool_checkers_skip_only_the_ids_of_their_own_symbols() {
    // Each `NewChecker` merges the globals, and its merge errors (TS2451,
    // hidden by skipLibCheck) give `d0` to `d25` their ids. They are binder
    // symbols, so Go gives each id once in the pool of 4 checkers: k4 gets
    // id 47 (4 * 4 + 26 + 5), not 125 ((4 + 26) * 4 + 5), and with a pad
    // of 8 w14 stays. Go's merge errors race in the pool, so a few ids can
    // burn (k4 49), but k4 keeps 2 digits.
    let files = [
        ("globals.d.ts", format!("{GLOBALS}{}", lets(26))),
        ("dup1.d.ts", lets(26)),
        ("dup2.d.ts", "declare const z: number;\n".into()),
    ];
    assert_eq!(check_files(&files, 8, &[]), expected_with_pad(8, 15, 5));
}

#[test]
fn merge_error_texts_give_ids_with_one_checker() {
    // The node builder gives `d0` to `d25` their ids as it writes their
    // names in the merge errors (nodebuilderimpl.go:974
    // getNameOfSymbolAsWritten), before `NewChecker` gives its 4 ids. So
    // with one checker k4 gets id 35, not 9, and w14 goes.
    let files = [
        ("globals.d.ts", format!("{GLOBALS}{}", lets(26))),
        ("dup1.d.ts", lets(26)),
        ("dup2.d.ts", "declare const z: number;\n".into()),
    ];
    assert_eq!(
        check_files(&files, 9, &["--checkers", "1"]),
        expected(14, 6)
    );
}

/// Go's error for `a_ts(9)` in `file`, with the members up to
/// `w{shown - 1}` and `more` hidden.
fn expected_in(file: &str, shown: usize, more: usize) -> String {
    expected(shown, more).replacen("a.ts", file, 1)
}

#[test]
fn ids_count_on_across_the_projects_of_tsc_b_single_threaded() {
    // Go: k4 gets id 9 in p1 and 44 in p2 (p1 gave 35 ids), so p2 shows one
    // member less. Each program counted from the start before.
    let file = |name: &str, text: String| (format!("{PROJECT}/{name}"), text.into());
    let config = r#"{"compilerOptions":{"composite":true,"noLib":true,"skipLibCheck":true,"strict":true,"outDir":"out"},"files":["globals.d.ts","a.ts"]}"#;
    let input = TscInput {
        files: [
            file(
                "tsconfig.json",
                r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#.into(),
            ),
            file("p1/globals.d.ts", GLOBALS.into()),
            file("p1/a.ts", a_ts(9)),
            file("p1/tsconfig.json", config.into()),
            file("p2/globals.d.ts", GLOBALS.into()),
            file("p2/a.ts", a_ts(9)),
            file("p2/tsconfig.json", config.into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    let sys = new_test_sys(&input, false);
    let args = ["-b", "--pretty", "false", "--singleThreaded"].map(String::from);
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    // The test system adds the list of files of each project.
    let diagnostics: String = sys
        .output_text()
        .lines()
        .filter(|line| !line.starts_with("!!! List files"))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_eq!(
        diagnostics,
        expected_in("p1/a.ts", 15, 5) + &expected_in("p2/a.ts", 14, 6)
    );
}

#[test]
fn ids_count_on_across_watch_cycles_single_threaded() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::ids_count_on_across_watch_cycles_single_threaded",
        || {
            // Go: k4 gets id 9 in the first build, which gives 35 ids. The
            // edit adds e1, and the next build counts on from there: after
            // the 4 ids of `NewChecker`, the d.ts signature of the changed
            // a.ts gives k4 its id before the check (the declaration
            // transform asks `EmitResolver.IsLateBound`). k4 gets 40, so
            // one member less shows.
            let file = |name: &str, text: String| (format!("{PROJECT}/{name}"), text.into());
            let input = TscInput {
                files: [
                    file("globals.d.ts", GLOBALS.into()),
                    file("a.ts", a_ts(9)),
                    file(
                        "tsconfig.json",
                        r#"{"compilerOptions":{"noLib":true,"skipLibCheck":true,"strict":true,"noEmit":true},"files":["globals.d.ts","a.ts"]}"#.into(),
                    ),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            };
            let sys = new_in_process_test_sys(&input);
            let args = ["-w", "--pretty", "false", "--singleThreaded"].map(String::from);
            let result = command_line_in_process(&context::background(), &sys, &args);
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            assert!(
                sys.output_text().contains(&expected(15, 5)),
                "first build: {}",
                sys.output_text()
            );
            sys.set_output_bytes(Vec::new());
            let a = format!("{PROJECT}/a.ts");
            let _ = sys
                .fs_from_file_map()
                .write_file(&a, &format!("declare const e1: number;\n{}", a_ts(9)));
            sys.mock_watch_backend().send_events(vec![Event {
                kind: EventKind::Update,
                path: a,
            }]);
            w.do_cycle();
            let expected = expected(14, 6).replacen("(7,7)", "(8,7)", 1);
            assert!(
                sys.output_text().contains(&expected),
                "second build: {}",
                sys.output_text()
            );
        },
    );
}

// followups38 items 2 and 4 (R186 reviewer item 1). In
// `c.valueSymbolLinks.Get(s).f = <call>` Go reads the links of the new
// symbol `s` (and gives its id) before the call on the right side, and a
// `links := c.valueSymbolLinks.Get(s)` comes right after `s` is made.
// `binder.GetSymbolNameForPrivateIdentifier` gives an id to the symbol that
// it gets: the class, or the symbol of the type that the site looks in. Each
// test below puts the first id of `u` on the right side of one such site
// (or after the private name site), with Go's id of `u` at a digit boundary
// (10 or 100, `--checkers 1`). With the old order `u` gets one id less, so
// its late-bound name is one byte shorter and one more member shows. The
// expected texts are the output of `tsgo-oracle-673a5f17d713 -p
// tsconfig.json --pretty false --checkers 1` on the same files, and a debug
// build of Go at the same pin gave the id of `u`.
//
// The other sites have no case that changes the output:
// - padObjectLiteralType: the implied type of the binding pattern
//   (getTypeFromObjectBindingPattern) reads the type of each element first.
// - the `args` symbol of combineUnionOrIntersectionParameters: the loop
//   before it reads the type at each position of the shorter signature, so
//   the right side can only give ids to tuple element symbols.
// - createSymbolWithType: each caller reads the links of the source first,
//   except getUndefinedProperty and createCombinedSymbolForOverloadFailure.
//   Their sources (object literal properties, parameters) are never the
//   symbol of a unique symbol.
// - the JSX children symbol: the right side can give ids, but only to the
//   element and `length` symbols of a new tuple target (createTupleType).
//   No name holds their ids.
// - reportUnmatchedProperty: the error then prints the source class, which
//   gives it its id in the old order too, and nothing between gives an id.

/// The diagnostics of `--checkers 1` for one site. `decl.d.ts` holds `u`
/// and `decl` (not checked: skipLibCheck). `a.ts` holds `fill` declarations
/// that take one id each, `site`, and `xq`: a type that holds `[key]` and a
/// pad of `pad` letters `p`. `z.ts` holds `after`, checked after `a.ts`.
fn check_site(
    decl: &str,
    site: &str,
    after: Option<&str>,
    key: &str,
    fill: usize,
    pad: usize,
) -> String {
    let fillers: String = (0..fill)
        .map(|i| format!("declare const zf{i}: number;\n"))
        .collect();
    let mut files = vec![
        ("globals.d.ts", GLOBALS.to_string()),
        (
            "decl.d.ts",
            format!("declare const u: unique symbol;\n{decl}"),
        ),
        (
            "a.ts",
            format!(
                "{fillers}{site}declare const xq: {{ [{key}]: void; {}: number; {}q: string }};\nconst nq: number = xq;\n",
                "p".repeat(pad),
                members(20)
            ),
        ),
    ];
    if let Some(after) = after {
        files.push(("z.ts", after.to_string()));
    }
    let names: Vec<String> = files
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect();
    let tsconfig = format!(
        r#"{{"compilerOptions":{{"noLib":true,"skipLibCheck":true,"strict":true,"noEmit":true,"target":"es2020"}},"files":[{}]}}"#,
        names.join(",")
    );
    files.push(("tsconfig.json", tsconfig));
    tsc_p(&files, &["--checkers", "1"])
}

/// Go's error on `xq` at `line` of `a.ts` (`check_site`): with the id of
/// the key at the boundary, the members up to `w13` show.
fn site_error(line: usize, key: &str, pad: usize) -> String {
    format!(
        "a.ts({line},7): error TS2322: Type '{{ [{key}]: void; {}: number; {}... 6 more ...; q: string; }}' is not assignable to type 'number'.\n",
        "p".repeat(pad),
        members(14)
    )
}

#[test]
fn binding_pattern_symbols_get_ids_before_their_types() {
    // getTypeFromObjectBindingPattern: `b` gets id 9, then its default `u`
    // gets 10.
    let site = "function f({ a, b = u }) { return [a, b]; }\n";
    assert_eq!(
        check_site("", site, None, "u", 2, 11),
        "a.ts(3,14): error TS7031: Binding element 'a' implicitly has an 'any' type.\n".to_string()
            + &site_error(5, "u", 11)
    );
}

#[test]
fn spread_symbols_get_ids_before_their_union_type() {
    // getSpreadType: the merged `a` gets its id before the union of
    // `typeof u` and `string`.
    let decl = "declare const l: { a: typeof u };\ndeclare const r: { a?: string };\n";
    assert_eq!(
        check_site(decl, "const s = { ...l, ...r };\n", None, "u", 89, 10),
        site_error(92, "u", 10)
    );
}

#[test]
fn partial_spread_symbols_get_ids_before_their_types() {
    // tryMergeUnionOfObjectTypeAndEmptyObject: the optional `a` gets its id
    // before the type of `o.a`.
    let decl = "declare const o: { a: typeof u } | undefined;\n";
    assert_eq!(
        check_site(decl, "const s = { ...o };\n", None, "u", 1, 11),
        site_error(4, "u", 11)
    );
}

#[test]
fn spread_symbol_copies_get_ids_before_their_types() {
    // getSpreadSymbol: the copy of the readonly `a` gets its id before the
    // type of `o.a`.
    let decl = "declare const o: { readonly a: typeof u };\n";
    assert_eq!(
        check_site(decl, "const s = { ...o };\n", None, "u", 1, 11),
        site_error(4, "u", 11)
    );
}

#[test]
fn object_literal_properties_get_ids_before_the_pattern_error() {
    // checkObjectLiteral: `zz` gets its id before the error text prints the
    // implied type of the pattern, which gives `u` its id.
    let decl = "declare const v: { k: typeof u };\n";
    assert_eq!(
        check_site(decl, "const { a = v } = { a: 1, zz: 2 };\n", None, "u", 87, 10),
        "a.ts(88,27): error TS2353: Object literal may only specify known properties, and 'zz' does not exist in type '{ a?: { k: unique symbol; } | undefined; }'.\n".to_string()
            + &site_error(90, "u", 10)
    );
}

#[test]
fn reverse_mapped_properties_get_ids_before_their_source() {
    // resolveReverseMappedTypeMembers: the inferred `s` gets its id before
    // `C.s`, the unique symbol whose id the key `[C.s]` holds.
    let decl = "declare class C { static readonly s: unique symbol; }
type Box<T> = { v: T };
declare function un<T>(x: { [K in keyof T]: Box<T[K]> }): T;
";
    assert_eq!(
        check_site(decl, "const r = un(C);\n", None, "C.s", 89, 8),
        "a.ts(90,14): error TS2345: Argument of type 'typeof C' is not assignable to parameter of type '{ readonly s: Box<unknown>; prototype: Box<unknown>; }'.
  Types of property 's' are incompatible.
    Type 'typeof C.s' is not assignable to type 'Box<unknown>'.
"
        .to_string()
            + &site_error(92, "C.s", 8)
    );
}

#[test]
fn import_attribute_symbols_get_ids_before_their_values() {
    // checkImportAttributesExpression: `type` gets its id before its value
    // `u` (an error, but Go checks it).
    let decl = "interface ImportAttributes { type?: unknown }\n";
    let site = "import { k } from \"./z\" with { type: u };\n";
    assert_eq!(
        check_site(decl, site, Some("export const k = 1;\n"), "u", 3, 11),
        "a.ts(4,25): error TS2823: Import attributes are only supported when the '--module' option is set to 'esnext', 'node18', 'node20', 'nodenext', or 'preserve'.
a.ts(4,38): error TS2858: Import attribute values must be string literal expressions.
"
        .to_string()
            + &site_error(6, "u", 11)
    );
}

#[test]
fn private_name_lookups_give_the_class_its_id() {
    // The return type of `m` checks `this.#x`
    // (lookupSymbolForPrivateIdentifierDeclaration), which gives `C` its id
    // before `u`. Go gives `C` its id when it binds `z.ts`; the port does
    // not, so the class must get it at this site. `C` is checked after `u`.
    let after = "class C {\n  #x = 1;\n  m() { return this.#x; }\n}\n";
    assert_eq!(
        check_site(
            "declare const i: C;\n",
            "const r = i.m();\n",
            Some(after),
            "u",
            88,
            10
        ),
        site_error(91, "u", 10)
    );
}

#[test]
fn private_name_assertion_calls_give_the_class_its_id() {
    // The return type of `m` narrows `v` through the assertion call
    // `this.#a(v)` (getTypeOfDottedName), which gives `C` its id before `u`.
    let after = "class C {
  #a(x: unknown): asserts x is number {}
  m(v: unknown) { this.#a(v); return v; }
}
";
    assert_eq!(
        check_site(
            "declare const i: C;\n",
            "const r = i.m(0);\n",
            Some(after),
            "u",
            87,
            10
        ),
        site_error(90, "u", 10)
    );
}

#[test]
fn this_private_name_contextual_types_give_the_this_type_its_id() {
    // getContextualTypeForAssignmentExpression: the contextual type of the
    // right side of `this.#x = ...` looks up `#x` in the `this` type
    // `Other`, which gives `Other` its id before `u`. The left side
    // `this.#x` and its error do not give `Other` an id.
    let site = "class K { #x = () => 0; m() { function g(this: Other) { this.#x = () => 1; } return g; } }\n";
    assert_eq!(
        check_site("interface Other { a: number }\n", site, None, "u", 85, 10),
        "a.ts(86,62): error TS2339: Property '#x' does not exist on type 'Other'.\n".to_string()
            + &site_error(88, "u", 10)
    );
}
