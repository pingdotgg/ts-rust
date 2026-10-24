//! Port-only test of the symbol ids that late-bound property names hold
//! (trunc1, from the infmemo1 skeptic fuzz case g106-36).
//!
//! The property name of a unique symbol holds its id (`<prefix>@k4@<id>`,
//! checker/checker.go:23519 getESSymbolLikeTypeForNode), and the node
//! builder counts the length of that name toward truncation
//! (checker/nodebuilderimpl.go:2630 addPropertyToElementList). So the
//! digits of the id move where `... N more ...` starts.
//!
//! At the Go pin (ts#64711) a `valueSymbolLinks` read gives a symbol that
//! has no id a block id (2^48 + n, 15 digits); only explicit
//! `ast.GetSymbolId` calls give global ids (`ValueSymbolLinkStore`,
//! `ast::SymbolIdGenerator`). The first id of `k4` (and of `u` and `C.s`
//! below) comes from a link read, so its name always has 15 digits, and
//! every case here shows the members up to `w12`. Before ts#64711 the key
//! had a global id of 1 to 3 digits, and its digits (which the order of
//! other id sites set) showed 1 or 2 more members. The expected texts are
//! the output of `tsgo-oracle-3e9236361fa9 --pretty false` on the same
//! files (linkids1, `bunsteal3/linkids1/lane/symtrunc`). A key that got a
//! global id first (an explicit site before its first link read, or a link
//! read that gives a global id) would show more members.
//!
//! followups38 added the ids that `NewChecker` gives to binder symbols
//! (merge errors) and the programs of one process with `--singleThreaded`
//! (`tsc -b`, watch), whose counters run on from one program to the next
//! (`program::CheckerPool::carry_symbol_ids`). idcarry1 added the same for
//! the default checkers and `--checkers N`.

use ts_goport::execute::tsc::ExitStatus;
use ts_goport::fswatch::{Event, EventKind};
use ts_goport::gostd::context;

use crate::support::child::{
    command_line_in_process, new_in_process_test_sys, run_command_in_child, run_test_in_child,
};
use crate::support::runner::TscInput;
use crate::support::test_sys::{TSC_LIB_PATH, new_test_sys};

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
/// At the Go pin the key always has a block id: `expected(13, 7)`.
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
    // The type of `k4` reads its links first: a block id. Go 673a5f17d713
    // gave it the global id 13 and showed w13.
    assert_eq!(check(&[]), expected(13, 7));
}

#[test]
fn unique_symbol_ids_count_as_go_with_one_checker() {
    // Go 673a5f17d713: global id 9, w14 showed.
    assert_eq!(check(&["--checkers", "1"]), expected(13, 7));
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
    // hidden by skipLibCheck) give `d0` to `d25` global ids. They are
    // binder symbols, so Go gives each id once in the pool of 4 checkers.
    // `k4` gets a block id. Go 673a5f17d713 gave it the global id 47 (4 * 4
    // + 26 + 5), not 125, and w14 showed with a pad of 8.
    let files = [
        ("globals.d.ts", format!("{GLOBALS}{}", lets(26))),
        ("dup1.d.ts", lets(26)),
        ("dup2.d.ts", "declare const z: number;\n".into()),
    ];
    assert_eq!(check_files(&files, 8, &[]), expected_with_pad(8, 13, 7));
}

#[test]
fn merge_error_texts_give_ids_with_one_checker() {
    // The node builder gives `d0` to `d25` global ids as it writes their
    // names in the merge errors (nodebuilderimpl.go:975
    // getNameOfSymbolAsWritten). `k4` gets a block id. Go 673a5f17d713 gave
    // it the global id 35 and showed w13.
    let files = [
        ("globals.d.ts", format!("{GLOBALS}{}", lets(26))),
        ("dup1.d.ts", lets(26)),
        ("dup2.d.ts", "declare const z: number;\n".into()),
    ];
    assert_eq!(
        check_files(&files, 9, &["--checkers", "1"]),
        expected(13, 7)
    );
}

/// Go's error for `a_ts(9)` in `file`, with the members up to
/// `w{shown - 1}` and `more` hidden.
fn expected_in(file: &str, shown: usize, more: usize) -> String {
    expected(shown, more).replacen("a.ts", file, 1)
}

#[test]
fn ids_count_on_across_the_projects_of_tsc_b_single_threaded() {
    // Go: k4 gets a block id in p1 and in p2. Go 673a5f17d713 gave it the
    // global ids 9 and 44 (p1 gave 35 ids), and p2 showed one member less.
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
        expected_in("p1/a.ts", 13, 7) + &expected_in("p2/a.ts", 13, 7)
    );
}

/// `class <name> { static readonly #<member>: unique symbol; ... }` with an
/// error that prints the private name, one line.
fn private_class(name: &str, member: &str) -> String {
    format!(
        "class {name} {{ static readonly #{member}: unique symbol; static t() {{ const n: number = {name}.#{member}; }} }}\n"
    )
}

#[test]
fn private_name_ids_count_on_across_the_projects_of_tsc_b_single_threaded() {
    // linkids1 (privname1 open case L/c/build). A private name holds the
    // symbol id of its class, which Go's bind gives (binder.go:375). p1's
    // binds give A and B ids 1 and 2. Its check gives no global id: link
    // reads give block ids (ts#64711), and the check of `t` reads no links
    // (ts#64739). Its d.ts emit gives the two `t` methods ids 3 and 4
    // (serializeReturnTypeForSignature). So p2's binds give C and D ids 5
    // and 6. The texts are the output of `tsgo-oracle-3e9236361fa9 -b
    // --pretty false --singleThreaded` on the same files.
    let file = |name: &str, text: String| (format!("{PROJECT}/{name}"), text.into());
    let config = r#"{"compilerOptions":{"composite":true,"noLib":true,"skipLibCheck":true,"strict":true,"outDir":"out"},"files":["globals.d.ts","a.ts"]}"#;
    let input = TscInput {
        files: [
            file(
                "tsconfig.json",
                r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#.into(),
            ),
            file("p1/globals.d.ts", GLOBALS.into()),
            file(
                "p1/a.ts",
                private_class("A", "a") + &private_class("B", "b"),
            ),
            file("p1/tsconfig.json", config.into()),
            file("p2/globals.d.ts", GLOBALS.into()),
            file(
                "p2/a.ts",
                private_class("C", "c") + &private_class("D", "d"),
            ),
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
    let diagnostics: String = sys
        .output_text()
        .lines()
        .filter(|line| !line.starts_with("!!! List files"))
        .map(|line| format!("{line}\n"))
        .collect();
    // The output writes the internal prefix as U+FFFD, as Go's does.
    let error = |file: &str, line: usize, class: &str, id: usize, member: &str| {
        format!(
            "{file}({line},65): error TS2322: Type 'typeof {class}.\u{FFFD}#{id}@#{member}' is not assignable to type 'number'.\n"
        )
    };
    assert_eq!(
        diagnostics,
        error("p1/a.ts", 1, "A", 1, "a")
            + &error("p1/a.ts", 2, "B", 2, "b")
            + &error("p2/a.ts", 1, "C", 5, "c")
            + &error("p2/a.ts", 2, "D", 6, "d")
    );
}

#[test]
fn ids_count_on_across_watch_cycles_single_threaded() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::ids_count_on_across_watch_cycles_single_threaded",
        || {
            // Go: k4 gets a block id in both builds. Go 673a5f17d713 gave
            // it the global ids 9 and 40 (the next build counts on from the
            // first), and the second build showed one member less.
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
                sys.output_text().contains(&expected(13, 7)),
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
            let expected = expected(13, 7).replacen("(7,7)", "(8,7)", 1);
            assert!(
                sys.output_text().contains(&expected),
                "second build: {}",
                sys.output_text()
            );
        },
    );
}

/// `class <name> { static readonly #<member> = Symbol(); ... }` with an
/// error that prints the private name, one line (idcarry1 cases).
fn symbol_class(name: &str, member: &str) -> String {
    format!(
        "class {name} {{ static readonly #{member} = Symbol(); static t() {{ const n: number = {name}.#{member}; }} }}\n"
    )
}

/// The TS2322 error of `symbol_class(class, member)` on `line` of `file`,
/// with the class's symbol id `id`. The output writes the internal prefix as
/// U+FFFD, as Go's does.
fn symbol_class_error(file: &str, line: usize, class: &str, id: usize, member: &str) -> String {
    format!(
        "{file}({line},61): error TS2322: Type 'typeof {class}.\u{FFFD}#{id}@#{member}' is not assignable to type 'number'.\n"
    )
}

/// `files` (name in the project and text), with the bundled libs at the
/// default lib path. The test system's own libs are small stubs, and Go's
/// ids here come from the checks of the real libs.
fn with_real_libs(files: Vec<(&str, String)>) -> TscInput {
    let libs = ts_goport::frontend::tsoptions::LIB_FILES_SET
        .iter()
        .map(String::as_str)
        .chain(["lib.d.ts"])
        .filter_map(|name| {
            let text =
                ts_goport::frontend::bundled::bundled_text(&format!("bundled:///libs/{name}"))?;
            Some((format!("{TSC_LIB_PATH}/{name}"), text.to_string()))
        });
    TscInput {
        files: files
            .into_iter()
            .map(|(name, text)| (format!("{PROJECT}/{name}"), text))
            .chain(libs)
            .map(|(name, text)| (name, text.into()))
            .collect(),
        ..Default::default()
    }
}

/// The diagnostics of `tsc -b --pretty false` plus `extra` on idcarry1's
/// `build-def`: p2 references p1, each has one file with two classes with a
/// private name, and the projects check the es2022 lib.
fn build_def(extra: &[&str]) -> String {
    let config = |files: &str, references: &str| {
        format!(
            r#"{{"compilerOptions":{{"strict":true,"composite":true,"emitDeclarationOnly":true,"outDir":"out","types":[],"lib":["es2022"],"target":"es2022"}},"files":[{files}]{references}}}"#
        )
    };
    let input = with_real_libs(vec![
        (
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#.into(),
        ),
        ("p1/a.ts", symbol_class("A", "a") + &symbol_class("B", "b")),
        ("p1/tsconfig.json", config(r#""a.ts""#, "")),
        ("p2/b.ts", symbol_class("C", "c") + &symbol_class("D", "d")),
        (
            "p2/tsconfig.json",
            config(r#""b.ts""#, r#","references":[{"path":"../p1"}]"#),
        ),
    ]);
    let sys = new_test_sys(&input, false);
    let mut args: Vec<String> = ["-b", "--pretty", "false"].map(String::from).to_vec();
    args.extend(extra.iter().map(|arg| arg.to_string()));
    let result = run_command_in_child(&sys, &args).unwrap_or_else(|err| panic!("tsgo: {err}"));
    assert!(result.unported.is_none(), "unported {:?}", result.unported);
    // The test system adds the list of files of each project.
    sys.output_text()
        .lines()
        .filter(|line| !line.starts_with("!!! List files"))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The errors of `build_def` when p1's checkers gave `p1_ids` global ids:
/// p1's binds give A and B ids 1 and 2, so p2's give C and D the next two.
fn build_def_expected(p1_ids: usize) -> String {
    symbol_class_error("p1/a.ts", 1, "A", 1, "a")
        + &symbol_class_error("p1/a.ts", 2, "B", 2, "b")
        + &symbol_class_error("p2/b.ts", 1, "C", p1_ids + 3, "c")
        + &symbol_class_error("p2/b.ts", 2, "D", p1_ids + 4, "d")
}

// idcarry1 (R228 reviewer item 1). Go's symbol id counters are per process,
// so p2 counts on from the global ids that p1's checkers gave, in check and
// d.ts emit. Each checker of the port counts on its own; the pool hands its
// ids back to the loading thread when p1 is built
// (`program::CheckerPool::carry_symbol_ids`). With 4 checkers Go gives
// `SharedArrayBuffer` a block id on one checker before another names it
// (`getNameOfSymbolAsWritten`), so p1 gives 34 global ids; with 1 or 2
// checkers the name comes first and p1 gives 35. The expected texts are
// the output of `tsgo-oracle-3e9236361fa9 -b --pretty false` (with the same
// options) on the same files (idcarry1 `build-def`; the default printed
// `#37 #38` in 51 of 52 runs and `#38 #39` once, `--checkers 1` and `2`
// printed `#38 #39` in every run).
#[test]
fn ids_count_on_across_the_projects_of_tsc_b() {
    assert_eq!(build_def(&[]), build_def_expected(34));
}

#[test]
fn ids_count_on_across_the_projects_of_tsc_b_with_one_or_two_checkers() {
    assert_eq!(build_def(&["--checkers", "1"]), build_def_expected(35));
    assert_eq!(build_def(&["--checkers", "2"]), build_def_expected(35));
}

#[test]
fn ids_count_on_across_watch_cycles() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::ids_count_on_across_watch_cycles",
        || {
            // idcarry1 `w1`: the first build gives 36 global ids after the 2
            // ids of A and C, and the later builds check only b.ts and give
            // none. So D gets 39 and E 40. The texts are the output of
            // `tsgo-oracle-3e9236361fa9 -w --pretty false` on the same files
            // (6 of 6 runs).
            let b_ts = |class: &str, member: &str| symbol_class(class, member) + "export {};\n";
            let a_ts = symbol_class("A", "a")
                + "type R = Record<'x' | 'y', number>;
declare const k1: unique symbol;
export const obj = { [k1]: 1, m(a: Partial<R>) { return a; } };
const bad: string = obj;
";
            let input = with_real_libs(vec![
                ("a.ts", a_ts),
                ("b.ts", b_ts("C", "c")),
                (
                    "tsconfig.json",
                    r#"{"compilerOptions":{"strict":true,"noEmit":true,"types":[],"lib":["es2022"],"target":"es2022"},"files":["a.ts","b.ts"]}"#.into(),
                ),
            ]);
            let sys = new_in_process_test_sys(&input);
            let args = ["-w", "--pretty", "false"].map(String::from);
            let result = command_line_in_process(&context::background(), &sys, &args);
            let mut w = result
                .watcher
                .expect("expected Watcher to be non-nil in watch mode");
            let first = sys.output_text();
            for error in [
                symbol_class_error("a.ts", 1, "A", 2, "a"),
                symbol_class_error("b.ts", 1, "C", 1, "c"),
            ] {
                assert!(first.contains(&error), "first build: {first}");
            }
            for (class, member, id) in [("D", "d", 39), ("E", "e", 40)] {
                sys.set_output_bytes(Vec::new());
                let b = format!("{PROJECT}/b.ts");
                let _ = sys.fs_from_file_map().write_file(&b, &b_ts(class, member));
                sys.mock_watch_backend().send_events(vec![Event {
                    kind: EventKind::Update,
                    path: b,
                }]);
                w.do_cycle();
                let output = sys.output_text();
                for error in [
                    symbol_class_error("a.ts", 1, "A", 2, "a"),
                    symbol_class_error("b.ts", 1, class, id, member),
                ] {
                    assert!(output.contains(&error), "build of {class}: {output}");
                }
            }
        },
    );
}

/// The private name ids in `output` (`#<id>@#<name>`), in order, joined by
/// spaces.
fn private_name_ids(output: &str) -> String {
    let mut ids = Vec::new();
    for (at, _) in output.match_indices("@#") {
        let digits = output[..at]
            .bytes()
            .rev()
            .take_while(u8::is_ascii_digit)
            .count();
        if digits == 0 || !output[..at - digits].ends_with('#') {
            continue;
        }
        let name: String = output[at + 2..]
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        ids.push(format!("#{}@#{name}", &output[at - digits..at]));
    }
    ids.join(" ")
}

/// The private name ids (`private_name_ids`) of each build of a
/// `tsc -b -w --pretty false` session plus `extra` on `files` (with the
/// bundled libs): the first build, then one cycle for each edit, which
/// writes one file and sends its event.
fn build_watch_ids(
    files: Vec<(&str, String)>,
    edits: &[(&str, String)],
    extra: &[&str],
) -> Vec<String> {
    let sys = new_in_process_test_sys(&with_real_libs(files));
    let mut args: Vec<String> = ["-b", "-w", "--pretty", "false"].map(String::from).to_vec();
    args.extend(extra.iter().map(|arg| arg.to_string()));
    let result = command_line_in_process(&context::background(), &sys, &args);
    let mut w = result
        .watcher
        .expect("expected Watcher to be non-nil in watch mode");
    let mut ids = vec![private_name_ids(&sys.output_text())];
    for (name, text) in edits {
        sys.set_output_bytes(Vec::new());
        let path = format!("{PROJECT}/{name}");
        let _ = sys.fs_from_file_map().write_file(&path, text);
        sys.mock_watch_backend().send_events(vec![Event {
            kind: EventKind::Update,
            path,
        }]);
        w.do_cycle();
        ids.push(private_name_ids(&sys.output_text()));
    }
    ids
}

/// The project config of the watchreset1 cases, with `rest` after the
/// compiler options.
fn composite_config(root_dir: &str, rest: &str) -> String {
    format!(
        r#"{{"compilerOptions":{{"strict":true,"composite":true,"emitDeclarationOnly":true,{root_dir}"outDir":"out","types":[],"lib":["es2022"],"target":"es2022"}},{rest}}}"#
    )
}

// watchreset1r (option R). Go `tsc -b -w` parses every `.ts` file again in
// each program that loads it, and each `.d.ts` file (the libs here) once
// per cycle (build/host.go:50-56, orchestrator.go:511 `resetCaches`). The
// new symbols have no ids, and the bind gives each class with private names
// a new id. The port keeps the parses and gives their symbols new ids where
// Go parses again (`program::note_replayed_parse`, `program::bind_all`). The
// expected ids are the output of `tsgo-oracle-3e9236361fa9 -b -w --pretty
// false` (with the same options) on watchreset1's `bw` case (6 of 6 runs
// each mode, after231/watchreset1/logs/drive1.log): cycle 2 parses the libs
// again in p2 (A=78 in cycle 3), and cycle 3 binds the unchanged p2/b.ts
// again (G=117 in cycle 4).
fn bw_ids(extra: &[&str]) -> Vec<String> {
    let class2 =
        |a: (&str, &str), b: (&str, &str)| symbol_class(a.0, a.1) + &symbol_class(b.0, b.1);
    build_watch_ids(
        vec![
            (
                "tsconfig.json",
                r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#.into(),
            ),
            ("p1/a.ts", class2(("A", "a"), ("B", "b"))),
            (
                "p1/tsconfig.json",
                composite_config("", r#""files":["a.ts"]"#),
            ),
            ("p2/b.ts", class2(("C", "c"), ("D", "d"))),
            (
                "p2/tsconfig.json",
                composite_config("", r#""files":["b.ts"],"references":[{"path":"../p1"}]"#),
            ),
        ],
        &[
            (
                "p2/b.ts",
                class2(("E", "e"), ("F", "f"))
                    + "declare const k9: unique symbol;\nconst o = { [k9]: 1, [Symbol.iterator]: 2 };\nconst bad: string = o;\n",
            ),
            (
                "p1/a.ts",
                class2(("A", "a"), ("B", "b"))
                    + "export declare const k8: unique symbol;\nexport const p = { [k8]: 'x', [Symbol.hasInstance]: 3 };\nconst bad2: number = p;\n",
            ),
            ("p2/b.ts", class2(("G", "g"), ("H", "h"))),
        ],
        extra,
    )
}

#[test]
fn private_name_ids_follow_go_parses_across_build_watch_cycles() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::private_name_ids_follow_go_parses_across_build_watch_cycles",
        || {
            assert_eq!(
                bw_ids(&[]),
                [
                    "#1@#a #2@#b #37@#c #38@#d",
                    "#1@#a #2@#b #41@#e #42@#f",
                    "#78@#a #79@#b #41@#e #42@#f",
                    "#78@#a #79@#b #117@#g #118@#h",
                ]
            );
        },
    );
}

#[test]
fn private_name_ids_follow_go_parses_across_build_watch_cycles_single_threaded() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::private_name_ids_follow_go_parses_across_build_watch_cycles_single_threaded",
        || {
            assert_eq!(
                bw_ids(&["--singleThreaded"]),
                [
                    "#1@#a #2@#b #38@#c #39@#d",
                    "#1@#a #2@#b #42@#e #43@#f",
                    "#79@#a #80@#b #42@#e #43@#f",
                    "#79@#a #80@#b #118@#g #119@#h",
                ]
            );
        },
    );
}

// watchreset1r: `s.ts` is a file of both projects (no reference between
// them), so Go parses it in each program, already in the first build, and
// S gets a new id in p2 (go-note-check.md case `share`). The expected ids
// are the output of `tsgo-oracle-3e9236361fa9 -b -w --pretty false` with
// `--singleThreaded` and with `--builders 1` (5 of 5 runs each); the
// default builders race in Go.
fn share_ids(extra: &[&str]) -> Vec<String> {
    let config = |file: &str| {
        composite_config(
            r#""rootDir":"..","#,
            &format!(r#""files":["../s.ts","{file}"]"#),
        )
    };
    build_watch_ids(
        vec![
            (
                "tsconfig.json",
                r#"{"files":[],"references":[{"path":"p1"},{"path":"p2"}]}"#.into(),
            ),
            ("s.ts", symbol_class("S", "s")),
            ("p1/a.ts", symbol_class("A", "a")),
            ("p1/tsconfig.json", config("a.ts")),
            ("p2/b.ts", symbol_class("B", "b")),
            ("p2/tsconfig.json", config("b.ts")),
        ],
        &[
            ("s.ts", symbol_class("S", "s") + "const x: string = 1;\n"),
            ("p2/b.ts", symbol_class("B", "b") + "const y: string = 2;\n"),
        ],
        extra,
    )
}

#[test]
fn private_name_ids_follow_go_parses_of_a_shared_file_in_build_watch() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::private_name_ids_follow_go_parses_of_a_shared_file_in_build_watch",
        || {
            assert_eq!(
                share_ids(&["--singleThreaded"]),
                [
                    "#1@#a #2@#s #38@#b #39@#s",
                    "#42@#a #43@#s #79@#b #80@#s",
                    "#42@#a #43@#s #83@#b #84@#s",
                ]
            );
        },
    );
}

#[test]
fn private_name_ids_follow_go_parses_of_a_shared_file_with_one_builder() {
    run_test_in_child(
        "tsctests::symbol_id_truncation::private_name_ids_follow_go_parses_of_a_shared_file_with_one_builder",
        || {
            assert_eq!(
                share_ids(&["--builders", "1"]),
                [
                    "#1@#a #2@#s #37@#b #38@#s",
                    "#41@#a #42@#s #77@#b #78@#s",
                    "#41@#a #42@#s #81@#b #82@#s",
                ]
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
// (or after the private name site). At Go 673a5f17d713 the global id of `u`
// was at a digit boundary (10 or 100, `--checkers 1`), so the old order
// showed one more member. At the Go pin `u` gets a block id at its first
// link read, so both orders give the same text: the tests now check that
// no site gives `u` a global id first. The expected texts are the output of
// `tsgo-oracle-3e9236361fa9 -p tsconfig.json --pretty false --checkers 1`
// on the same files.
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

/// Go's error on `xq` at `line` of `a.ts` (`check_site`): with a block id
/// for the key, the members up to `w12` show.
fn site_error(line: usize, key: &str, pad: usize) -> String {
    format!(
        "a.ts({line},7): error TS2322: Type '{{ [{key}]: void; {}: number; {}... 7 more ...; q: string; }}' is not assignable to type 'number'.\n",
        "p".repeat(pad),
        members(13)
    )
}

#[test]
fn binding_pattern_symbols_get_ids_before_their_types() {
    // getTypeFromObjectBindingPattern: `b` gets its id, then its default
    // `u` (Go 673a5f17d713: 9 and 10).
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
    // before `u`. Go gives `C` its id when it binds `z.ts`. `C` is checked
    // after `u`.
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
