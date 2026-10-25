//! Port-only tests of the order in which `tsc -b` parses the configs of a
//! build (orchestrator.rs `create_build_tasks`), on builds where two
//! projects enter an `extends` cycle at different files (refsorder1).
//!
//! PORT: no Go counterpart: no Go baseline has an extends cycle in a build.
//! Go's extended config cache keeps the options of the first parse of each
//! extended file, cut at the cycle (tsoptions/tsconfigparsing.go:925
//! `getExtendedConfig`), so the parse order changes the options that each
//! project gets. With `--singleThreaded`, Go's work group is a stack
//! (core/workgroup.go:78), so the configs parse depth first, each reference
//! list last first, and command-line projects last first too. The expected
//! outputs are Go's at pin 3e9236361fa9 with `--singleThreaded` (20 of 20
//! runs for each case).
//!
//! Go's default mode parses on a parallel work group and has no fixed
//! order. Each of its runs that ends gives the output of some serial order,
//! and many runs deadlock (two parses each hold one extended config entry
//! and wait for the other). The port uses the `--singleThreaded` order in
//! both modes. Go default mode, 20 runs per case on 32 threads
//! (refsorder1 go-note-check.md):
//! - cfgcycle1-b: 14 as here, 6 hung; cfgcycle1-b-rev: 13 and 7;
//!   cfgcycle-b: 18 and 2; cfgcycle-b-rev: 18 and 2; cfgcycle-order3: 14
//!   and 6; three3: 19 and 1.
//! - nested2: 5 as here, 2 in another order, 13 hung.
//! - deepref: 0 as here, 19 in another order, 1 hung.
//! - siblings3: 1 as here, 6 in another order, 13 hung.
//! - tri3: 9 as here, 2 in another order, 9 hung.
//!
//! The tests run `tsgo` on a temp dir: in default mode on the OS file
//! system, the threads of `config_prefetch.rs` parse configs too.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A new empty dir under the system temp dir; removed on drop.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new(name: &str) -> TmpDir {
        let dir = std::env::temp_dir().join(format!("goport-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
        TmpDir(crate::support::eval_symlinks(&dir).expect("a real path"))
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {name}: {e}"));
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TS: &str = "function unused() {}\nexport function f(x) { return x; }\n";
const TS3: &str =
    "function unused() {}\nexport function f(x: number, y: number) { if (x) { return 1; } }\n";
const LEAF: &str = r#""strict":true,"noEmit":true,"types":[],"lib":["es5"]"#;
const LEAF3: &str = r#""noEmit":true,"types":[],"lib":["es5"]"#;
const NO_UNUSED_LOCALS: &str = r#""noUnusedLocals":true"#;
const NO_IMPLICIT_ANY: &str = r#""noImplicitAny":false"#;

/// The files of a case, as (path, text).
#[derive(Default)]
struct Files(Vec<(String, String)>);

impl Files {
    /// `name.json`, which extends `base.json`.
    fn ext(mut self, name: &str, base: &str, options: &str) -> Self {
        self.0.push((
            format!("{name}.json"),
            format!(r#"{{"extends":"./{base}.json","compilerOptions":{{{options}}}}}"#),
        ));
        self
    }

    /// A config with no files that references `names`; `dir` "" is the
    /// root config.
    fn refs(mut self, dir: &str, names: &[&str]) -> Self {
        let prefix = if dir.is_empty() { "./" } else { "../" };
        let refs: Vec<String> = names
            .iter()
            .map(|name| format!(r#"{{"path":"{prefix}{name}"}}"#))
            .collect();
        let config = if dir.is_empty() {
            "tsconfig.json".to_string()
        } else {
            format!("{dir}/tsconfig.json")
        };
        self.0.push((
            config,
            format!(r#"{{"files":[],"references":[{}]}}"#, refs.join(",")),
        ));
        self
    }

    /// A project with one file, `a.ts`, that extends `extends.json`.
    fn leaf_with(mut self, dir: &str, extends: Option<&str>, options: &str, ts: &str) -> Self {
        let extends = extends.map_or(String::new(), |e| format!(r#""extends":"../{e}.json","#));
        self.0.push((
            format!("{dir}/tsconfig.json"),
            format!(r#"{{{extends}"compilerOptions":{{{options}}},"files":["a.ts"]}}"#),
        ));
        self.0.push((format!("{dir}/a.ts"), ts.to_string()));
        self
    }

    fn leaf(self, dir: &str, extends: Option<&str>) -> Self {
        self.leaf_with(dir, extends, LEAF, TS)
    }

    /// a extends b extends a.
    fn cycle2(self, a: &str, b: &str) -> Self {
        self.ext("a", "b", a).ext("b", "a", b)
    }

    /// a extends b extends c extends a.
    fn cycle3(self) -> Self {
        self.ext("a", "b", NO_UNUSED_LOCALS)
            .ext("b", "c", NO_IMPLICIT_ANY)
            .ext("c", "a", "")
    }
}

const TS18000: &str = "error TS18000: Circularity detected while resolving configuration: {0}\n";

fn unused(project: &str) -> String {
    format!(
        "{project}/a.ts(1,10): error TS6133: 'unused' is declared but its value is never read.\n"
    )
}

fn implicit_any(project: &str) -> String {
    format!("{project}/a.ts(2,19): error TS7006: Parameter 'x' implicitly has an 'any' type.\n")
}

fn unused_y(project: &str) -> String {
    format!("{project}/a.ts(2,30): error TS6133: 'y' is declared but its value is never read.\n")
}

/// The cases (name, files, Go's `tsc -b --pretty false --singleThreaded`
/// output). Each exits 2.
fn cases() -> Vec<(&'static str, Files, String)> {
    let (s, u, a, y) = (TS18000, unused, implicit_any, unused_y);
    vec![
        // bunprobe1 min/cfgcycle1-b: the port reported TS6133 in p0.
        (
            "cfgcycle1-b",
            Files::default()
                .cycle2("", NO_UNUSED_LOCALS)
                .refs("", &["p0", "p1"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b")),
            [s, &a("p0"), s, &u("p1"), &a("p1")].concat(),
        ),
        (
            "cfgcycle1-b-rev",
            Files::default()
                .cycle2("", NO_UNUSED_LOCALS)
                .refs("", &["p1", "p0"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b")),
            [s, &u("p1"), &a("p1"), s, &u("p0"), &a("p0")].concat(),
        ),
        (
            "cfgcycle-b",
            Files::default()
                .cycle2(NO_UNUSED_LOCALS, NO_IMPLICIT_ANY)
                .refs("", &["p0", "p1"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b")),
            [s, &u("p0"), &a("p0"), s, &u("p1")].concat(),
        ),
        (
            "cfgcycle-b-rev",
            Files::default()
                .cycle2(NO_UNUSED_LOCALS, NO_IMPLICIT_ANY)
                .refs("", &["p1", "p0"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b")),
            [s, s, &u("p0")].concat(),
        ),
        // Depth first: p3, a reference of p2, parses before p1.
        (
            "cfgcycle-order3",
            Files::default()
                .cycle2("", NO_UNUSED_LOCALS)
                .refs("", &["p0", "p1", "p2"])
                .leaf("p0", None)
                .leaf("p1", Some("b"))
                .refs("p2", &["p3"])
                .leaf("p3", Some("a")),
            [&a("p0"), s, &u("p1"), &a("p1"), s, &u("p3"), &a("p3")].concat(),
        ),
        (
            "three3",
            Files::default()
                .cycle3()
                .refs("", &["p0", "p1", "p2"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b"))
                .leaf("p2", Some("c")),
            [s, &u("p0"), s, s, &u("p2")].concat(),
        ),
        (
            "nested2",
            Files::default()
                .cycle3()
                .refs("", &["p0", "p1"])
                .refs("p0", &["q0", "q1"])
                .refs("p1", &["q2", "q3"])
                .leaf("q0", Some("a"))
                .leaf("q1", Some("b"))
                .leaf("q2", Some("c"))
                .leaf("q3", None),
            [s, &u("q0"), s, s, &u("q2"), &a("q3")].concat(),
        ),
        // r0, a reference of a reference of p1, parses before p0.
        (
            "deepref",
            Files::default()
                .cycle2(NO_UNUSED_LOCALS, NO_IMPLICIT_ANY)
                .refs("", &["p0", "p1", "p2"])
                .leaf("p0", Some("a"))
                .refs("p1", &["q0"])
                .leaf("p2", None)
                .refs("q0", &["r0"])
                .leaf("r0", Some("b")),
            [s, &u("p0"), &a("p0"), s, &u("r0"), &a("p2")].concat(),
        ),
        (
            "siblings3",
            Files::default()
                .cycle2("", NO_UNUSED_LOCALS)
                .refs("", &["p0", "p1", "p2"])
                .leaf("p0", Some("a"))
                .leaf("p1", Some("b"))
                .leaf("p2", None),
            [s, &a("p0"), s, &u("p1"), &a("p1"), &a("p2")].concat(),
        ),
        // A 3-member cycle with an option in each member: c parses first.
        (
            "tri3",
            Files::default()
                .ext("a", "b", NO_UNUSED_LOCALS)
                .ext("b", "c", r#""noUnusedParameters":true"#)
                .ext("c", "a", r#""noImplicitReturns":true"#)
                .refs("", &["p0", "p1", "p2"])
                .refs("p0", &["q0", "q1"])
                .leaf_with("p1", Some("c"), LEAF3, TS3)
                .leaf_with("p2", None, LEAF3, "export const z = 1;\n")
                .leaf_with("q0", Some("a"), LEAF3, TS3)
                .leaf_with("q1", Some("b"), LEAF3, TS3),
            [
                s,
                &u("q0"),
                &y("q0"),
                s,
                &y("q1"),
                s,
                &u("p1"),
                "p1/a.ts(2,17): error TS7030: Not all code paths return a value.\n",
                &y("p1"),
            ]
            .concat(),
        ),
    ]
}

/// `tsgo -b --pretty false` and `args` in `dir`: the exit code and stdout.
fn build(dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["-b", "--pretty", "false"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run tsgo -b");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// Builds each case with `args` and compares the output with Go's.
fn check_cases(mode: &str, args: &[&str]) {
    let mut failures = Vec::new();
    for (name, files, expected) in cases() {
        let dir = TmpDir::new(&format!("cfgorder-{mode}-{name}"));
        for (path, text) in &files.0 {
            dir.write(path, text);
        }
        let actual = build(&dir.0, args);
        if actual != (Some(2), expected.clone()) {
            failures.push(format!(
                "{name}: exit {:?}\n--- expected (Go)\n{expected}--- actual\n{}",
                actual.0, actual.1
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn single_threaded_parses_references_last_first() {
    check_cases("single", &["--singleThreaded"]);
}

#[test]
fn default_mode_parses_in_the_single_threaded_order() {
    check_cases("default", &[]);
}
