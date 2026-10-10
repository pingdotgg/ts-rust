//! Go: `internal/vfs/vfsmatch/vfsmatch_test.go`.
//!
//! PORT: Go `matchFiles(path, ..., host.UseCaseSensitiveFileNames(),
//! currentDir, depth, host)` is `read_directory(host, currentDir, path, ...,
//! depth)`, which passes the host's case flag the same way. Go
//! `compileGlobPattern(spec, base, usage, cs)` with `p.matches(path)` is
//! `new_spec_matcher(&[spec], base, usage, cs)` with `match_string(path)`: a
//! one-pattern `SpecMatcher` matches exactly when its pattern does, and a
//! spec that does not compile gives `None`, as Go `ok == false`.
//! The table cases and hosts are translated from the Go file by
//! `tests2/S2/vfsmatch_gen.py`.
//!
//! Blocked (private Rust items in `src/frontend/vfs/vfsmatch.rs`):
//! `TestGetBasePathsCaseSensitivity` (`get_base_paths`), and the
//! `TestGlobPatternInternals` subtests that call `nextPathPartParts`
//! (`next_path_part_parts`) or `ensureTrailingSlash` (`ensure_trailing_slash`).

use std::fmt::Debug;
use std::rc::Rc;

use ts_goport::frontend::vfs::{
    Fs, UNLIMITED_DEPTH, Usage, is_implicit_glob, new_spec_matcher, read_directory,
};

use super::Failures;
use crate::support::vfstest::{MapFile, from_map, symlink};

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn has(got: &[String], want: &str) -> bool {
    got.iter().any(|g| g == want)
}

/// The checks of one case (Go `assert.*` in an `expect` func).
struct Check {
    failures: Vec<String>,
}

impl Check {
    fn check(&mut self, ok: bool, message: String) {
        if !ok {
            self.failures.push(message);
        }
    }

    fn check_eq<A: PartialEq<B> + Debug + ?Sized, B: Debug + ?Sized>(&mut self, got: &A, want: &B) {
        if got != want {
            self.failures.push(format!("got {got:?}, want {want:?}"));
        }
    }
}

// Go: vfsmatch_test.go:135 readDirTestCase
struct ReadDirTestCase {
    name: &'static str,
    host: fn() -> Rc<dyn Fs>,
    current_dir: &'static str,
    path: &'static str,
    extensions: Vec<String>,
    excludes: Vec<String>,
    includes: Vec<String>,
    depth: i32,
    expect: fn(&mut Check, &[String]),
}

impl Default for ReadDirTestCase {
    fn default() -> Self {
        ReadDirTestCase {
            name: "",
            host: || panic!("no host"),
            current_dir: "",
            path: "",
            extensions: Vec::new(),
            excludes: Vec::new(),
            includes: Vec::new(),
            depth: 0,
            expect: |_, _| {},
        }
    }
}

// Go: vfsmatch_test.go:147 runReadDirectoryCase
fn run_read_directory_case(tc: &ReadDirTestCase) -> Vec<String> {
    let current_dir = if tc.current_dir.is_empty() {
        "/"
    } else {
        tc.current_dir
    };
    let path = if tc.path.is_empty() { "/dev" } else { tc.path };
    let depth = if tc.depth == 0 {
        UNLIMITED_DEPTH
    } else {
        tc.depth
    };
    let host = (tc.host)();
    let got = read_directory(
        host.as_ref(),
        current_dir,
        path,
        &tc.extensions,
        &tc.excludes,
        &tc.includes,
        depth,
    );
    let mut c = Check {
        failures: Vec::new(),
    };
    (tc.expect)(&mut c, &got);
    c.failures
}

fn run_cases(test: &str, cases: Vec<ReadDirTestCase>) {
    let mut failures = Failures::new(test);
    for tc in &cases {
        for failure in run_read_directory_case(tc) {
            failures.fail(tc.name, failure);
        }
    }
    failures.finish();
}

// Go: vfsmatch_test.go:16 caseInsensitiveHost
fn case_insensitive_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/a.ts", MapFile::from("")),
            ("/dev/a.d.ts", MapFile::from("")),
            ("/dev/a.js", MapFile::from("")),
            ("/dev/b.ts", MapFile::from("")),
            ("/dev/b.js", MapFile::from("")),
            ("/dev/c.d.ts", MapFile::from("")),
            ("/dev/z/a.ts", MapFile::from("")),
            ("/dev/z/abz.ts", MapFile::from("")),
            ("/dev/z/aba.ts", MapFile::from("")),
            ("/dev/z/b.ts", MapFile::from("")),
            ("/dev/z/bbz.ts", MapFile::from("")),
            ("/dev/z/bba.ts", MapFile::from("")),
            ("/dev/x/a.ts", MapFile::from("")),
            ("/dev/x/aa.ts", MapFile::from("")),
            ("/dev/x/b.ts", MapFile::from("")),
            ("/dev/x/y/a.ts", MapFile::from("")),
            ("/dev/x/y/b.ts", MapFile::from("")),
            ("/dev/js/a.js", MapFile::from("")),
            ("/dev/js/b.js", MapFile::from("")),
            ("/dev/js/d.min.js", MapFile::from("")),
            ("/dev/js/ab.min.js", MapFile::from("")),
            ("/ext/ext.ts", MapFile::from("")),
            ("/ext/b/a..b.ts", MapFile::from("")),
        ],
        false,
    )
}

// Go: vfsmatch_test.go:45 caseSensitiveHost
fn case_sensitive_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/a.ts", MapFile::from("")),
            ("/dev/a.d.ts", MapFile::from("")),
            ("/dev/a.js", MapFile::from("")),
            ("/dev/b.ts", MapFile::from("")),
            ("/dev/b.js", MapFile::from("")),
            ("/dev/A.ts", MapFile::from("")),
            ("/dev/B.ts", MapFile::from("")),
            ("/dev/c.d.ts", MapFile::from("")),
            ("/dev/z/a.ts", MapFile::from("")),
            ("/dev/z/abz.ts", MapFile::from("")),
            ("/dev/z/aba.ts", MapFile::from("")),
            ("/dev/z/b.ts", MapFile::from("")),
            ("/dev/z/bbz.ts", MapFile::from("")),
            ("/dev/z/bba.ts", MapFile::from("")),
            ("/dev/x/a.ts", MapFile::from("")),
            ("/dev/x/b.ts", MapFile::from("")),
            ("/dev/x/y/a.ts", MapFile::from("")),
            ("/dev/x/y/b.ts", MapFile::from("")),
            ("/dev/q/a/c/b/d.ts", MapFile::from("")),
            ("/dev/js/a.js", MapFile::from("")),
            ("/dev/js/b.js", MapFile::from("")),
            ("/dev/js/d.MIN.js", MapFile::from("")),
        ],
        true,
    )
}

// Go: vfsmatch_test.go:73 commonFoldersHost
fn common_folders_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/a.ts", MapFile::from("")),
            ("/dev/a.d.ts", MapFile::from("")),
            ("/dev/a.js", MapFile::from("")),
            ("/dev/b.ts", MapFile::from("")),
            ("/dev/x/a.ts", MapFile::from("")),
            ("/dev/node_modules/a.ts", MapFile::from("")),
            ("/dev/bower_components/a.ts", MapFile::from("")),
            ("/dev/jspm_packages/a.ts", MapFile::from("")),
        ],
        false,
    )
}

// Go: vfsmatch_test.go:87 dottedFoldersHost
fn dotted_folders_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/x/d.ts", MapFile::from("")),
            ("/dev/x/y/d.ts", MapFile::from("")),
            ("/dev/x/y/.e.ts", MapFile::from("")),
            ("/dev/x/.y/a.ts", MapFile::from("")),
            ("/dev/.z/.b.ts", MapFile::from("")),
            ("/dev/.z/c.ts", MapFile::from("")),
            ("/dev/w/.u/e.ts", MapFile::from("")),
            ("/dev/g.min.js/.g/g.ts", MapFile::from("")),
        ],
        false,
    )
}

// Go: vfsmatch_test.go:101 mixedExtensionHost
fn mixed_extension_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/a.ts", MapFile::from("")),
            ("/dev/a.d.ts", MapFile::from("")),
            ("/dev/a.js", MapFile::from("")),
            ("/dev/b.tsx", MapFile::from("")),
            ("/dev/b.d.ts", MapFile::from("")),
            ("/dev/b.jsx", MapFile::from("")),
            ("/dev/c.tsx", MapFile::from("")),
            ("/dev/c.js", MapFile::from("")),
            ("/dev/d.js", MapFile::from("")),
            ("/dev/e.jsx", MapFile::from("")),
            ("/dev/f.other", MapFile::from("")),
        ],
        false,
    )
}

// Go: vfsmatch_test.go:118 sameNamedDeclarationsHost
fn same_named_declarations_host() -> Rc<dyn Fs> {
    from_map(
        vec![
            ("/dev/a.tsx", MapFile::from("")),
            ("/dev/a.d.ts", MapFile::from("")),
            ("/dev/b.tsx", MapFile::from("")),
            ("/dev/b.ts", MapFile::from("")),
            ("/dev/c.tsx", MapFile::from("")),
            ("/dev/m.ts", MapFile::from("")),
            ("/dev/m.d.ts", MapFile::from("")),
            ("/dev/n.tsx", MapFile::from("")),
            ("/dev/n.ts", MapFile::from("")),
            ("/dev/n.d.ts", MapFile::from("")),
            ("/dev/o.ts", MapFile::from("")),
            ("/dev/x.d.ts", MapFile::from("")),
        ],
        false,
    )
}

// Go: vfsmatch_test.go:165 TestReadDirectory
#[test]
fn test_read_directory() {
    let cases: Vec<ReadDirTestCase> = vec![
        ReadDirTestCase {
            name: "defaults include common package folders",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/node_modules/a.ts"),
                    "slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/bower_components/a.ts"),
                    "slices.Contains(got, \"/dev/bower_components/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/jspm_packages/a.ts"),
                    "slices.Contains(got, \"/dev/jspm_packages/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes without exclusions",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["a.ts", "b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts", "/dev/b.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes with non ts extensions excluded",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["a.js", "b.js"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes missing files excluded",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["z.ts", "x.ts"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes with literal excludes",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["b.ts"]),
            includes: strs(&["a.ts", "b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes with wildcard excludes",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["*.ts", "z/??z.ts", "*/b.ts"]),
            includes: strs(&["a.ts", "b.ts", "z/a.ts", "z/abz.ts", "z/aba.ts", "x/b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/z/a.ts", "/dev/z/aba.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal includes with recursive excludes",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**/b.ts"]),
            includes: strs(&["a.ts", "b.ts", "x/a.ts", "x/b.ts", "x/y/a.ts", "x/y/b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts", "/dev/x/a.ts", "/dev/x/y/a.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "case sensitive exclude is respected",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**/b.ts"]),
            includes: strs(&["B.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/B.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "explicit includes keep common package folders",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&[
                "a.ts",
                "b.ts",
                "node_modules/a.ts",
                "bower_components/a.ts",
                "jspm_packages/a.ts",
            ]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/node_modules/a.ts"),
                    "slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/bower_components/a.ts"),
                    "slices.Contains(got, \"/dev/bower_components/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/jspm_packages/a.ts"),
                    "slices.Contains(got, \"/dev/jspm_packages/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard include sorted order",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["z/*.ts", "x/*.ts"]),
            expect: |c, got| {
                let expected = strs(&[
                    "/dev/z/a.ts",
                    "/dev/z/aba.ts",
                    "/dev/z/abz.ts",
                    "/dev/z/b.ts",
                    "/dev/z/bba.ts",
                    "/dev/z/bbz.ts",
                    "/dev/x/a.ts",
                    "/dev/x/aa.ts",
                    "/dev/x/b.ts",
                ]);
                c.check_eq(got, &expected);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard include same named declarations excluded",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/a.d.ts"),
                    "slices.Contains(got, \"/dev/a.d.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/c.d.ts"),
                    "slices.Contains(got, \"/dev/c.d.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard star matches only ts files",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        f.contains(".ts") || f.contains(".tsx") || f.contains(".d.ts"),
                        format!("unexpected file: {}", f),
                    );
                }
                c.check(
                    !has(got, "/dev/a.js"),
                    "!slices.Contains(got, \"/dev/a.js\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/b.js"),
                    "!slices.Contains(got, \"/dev/b.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard question mark single character",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/?.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/x/a.ts", "/dev/x/b.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard recursive directory",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/z/a.ts"),
                    "slices.Contains(got, \"/dev/z/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/a.ts"),
                    "slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "double asterisk matches zero-or-more directories",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/**/a.ts"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &2);
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/a.ts"),
                    "slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard multiple recursive directories",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/y/**/a.ts", "x/**/a.ts", "z/**/a.ts"]),
            expect: |c, got| {
                c.check(got.len() > 0, "len(got) > 0".to_string());
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard case sensitive matching",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/A.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/A.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard missing files excluded",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*/z.ts"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude folders with wildcards",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["z", "x"]),
            includes: strs(&["**/*"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        !f.contains("/z/") && !f.contains("/x/"),
                        format!("should not contain z or x: {}", f),
                    );
                }
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include paths outside project absolute",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*", "/ext/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/ext/ext.ts"),
                    "slices.Contains(got, \"/ext/ext.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include paths outside project relative",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**"]),
            includes: strs(&["*", "../ext/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/ext/ext.ts"),
                    "slices.Contains(got, \"/ext/ext.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include files containing double dots",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**"]),
            includes: strs(&["/ext/b/a..b.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/ext/b/a..b.ts"),
                    "slices.Contains(got, \"/ext/b/a..b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude files containing double dots",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["/ext/b/a..b.ts"]),
            includes: strs(&["/ext/**/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/ext/ext.ts"),
                    "slices.Contains(got, \"/ext/ext.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/ext/b/a..b.ts"),
                    "!slices.Contains(got, \"/ext/b/a..b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "common package folders implicitly excluded",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/node_modules/a.ts"),
                    "!slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/bower_components/a.ts"),
                    "!slices.Contains(got, \"/dev/bower_components/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/jspm_packages/a.ts"),
                    "!slices.Contains(got, \"/dev/jspm_packages/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "common package folders explicit recursive include",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/a.ts", "**/node_modules/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/node_modules/a.ts"),
                    "slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "common package folders wildcard include",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/node_modules/a.ts"),
                    "!slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "common package folders explicit wildcard include",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*/a.ts", "node_modules/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/node_modules/a.ts"),
                    "slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "dotted folders not implicitly included",
            host: dotted_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/**/*", "w/*/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/d.ts"),
                    "slices.Contains(got, \"/dev/x/d.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/d.ts"),
                    "slices.Contains(got, \"/dev/x/y/d.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/.y/a.ts"),
                    "!slices.Contains(got, \"/dev/x/.y/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/y/.e.ts"),
                    "!slices.Contains(got, \"/dev/x/y/.e.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/w/.u/e.ts"),
                    "!slices.Contains(got, \"/dev/w/.u/e.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "dotted folders explicitly included",
            host: dotted_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/.y/a.ts", "/dev/.z/.b.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/.y/a.ts"),
                    "slices.Contains(got, \"/dev/x/.y/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/.z/.b.ts"),
                    "slices.Contains(got, \"/dev/.z/.b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "dotted folders recursive wildcard matches directories",
            host: dotted_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/.*/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/.y/a.ts"),
                    "slices.Contains(got, \"/dev/x/.y/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/.z/c.ts"),
                    "slices.Contains(got, \"/dev/.z/c.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/w/.u/e.ts"),
                    "slices.Contains(got, \"/dev/w/.u/e.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "trailing recursive include returns empty",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "trailing recursive exclude removes everything",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**"]),
            includes: strs(&["**/*"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "multiple recursive directory patterns in includes",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/x/**/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/a.ts"),
                    "slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "multiple recursive directory patterns in excludes",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**/x/**"]),
            includes: strs(&["**/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/z/a.ts"),
                    "slices.Contains(got, \"/dev/z/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/a.ts"),
                    "!slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/y/a.ts"),
                    "!slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "implicit globbification expands directory",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["z"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/z/a.ts"),
                    "slices.Contains(got, \"/dev/z/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/z/aba.ts"),
                    "slices.Contains(got, \"/dev/z/aba.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/z/b.ts"),
                    "slices.Contains(got, \"/dev/z/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude patterns starting with starstar",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**/x"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(!f.contains("/x/"), format!("should not contain /x/: {}", f));
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include patterns starting with starstar",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/x", "**/a/**/b"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/q/a/c/b/d.ts"),
                    "slices.Contains(got, \"/dev/q/a/c/b/d.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "depth limit one",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            depth: 1,
            expect: |c, got| {
                for f in got.iter() {
                    let suffix = &f["/dev/".len()..];
                    c.check(
                        !suffix.contains("/"),
                        format!("depth 1 should not include nested files: {}", f),
                    );
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "depth limit two",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            depth: 2,
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/z/a.ts"),
                    "slices.Contains(got, \"/dev/z/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/y/a.ts"),
                    "!slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "mixed extensions only ts",
            host: mixed_extension_host,
            extensions: strs(&[".ts"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        f.ends_with(".ts"),
                        format!("should only have .ts files: {}", f),
                    );
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "mixed extensions ts and tsx",
            host: mixed_extension_host,
            extensions: strs(&[".ts", ".tsx"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        f.ends_with(".ts") || f.ends_with(".tsx"),
                        format!("should only have .ts or .tsx files: {}", f),
                    );
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "mixed extensions js and jsx",
            host: mixed_extension_host,
            extensions: strs(&[".js", ".jsx"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        f.ends_with(".js") || f.ends_with(".jsx"),
                        format!("should only have .js or .jsx files: {}", f),
                    );
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "min js files excluded by wildcard",
            host: case_insensitive_host,
            extensions: strs(&[".js"]),
            includes: strs(&["js/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/js/a.js"),
                    "slices.Contains(got, \"/dev/js/a.js\")".to_string(),
                );
                c.check(
                    has(got, "/dev/js/b.js"),
                    "slices.Contains(got, \"/dev/js/b.js\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/js/d.min.js"),
                    "!slices.Contains(got, \"/dev/js/d.min.js\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/js/ab.min.js"),
                    "!slices.Contains(got, \"/dev/js/ab.min.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "min js exclusion is case-sensitive on case-sensitive FS",
            host: case_sensitive_host,
            extensions: strs(&[".js"]),
            includes: strs(&["js/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/js/a.js"),
                    "slices.Contains(got, \"/dev/js/a.js\")".to_string(),
                );
                c.check(
                    has(got, "/dev/js/b.js"),
                    "slices.Contains(got, \"/dev/js/b.js\")".to_string(),
                );
                // Legacy behavior: only lowercase ".min.js" is excluded by default when matching is case-sensitive.
                c.check(
                    has(got, "/dev/js/d.MIN.js"),
                    "slices.Contains(got, \"/dev/js/d.MIN.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "min js files explicitly included",
            host: case_insensitive_host,
            extensions: strs(&[".js"]),
            includes: strs(&["js/*.min.js"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/js/d.min.js"),
                    "slices.Contains(got, \"/dev/js/d.min.js\")".to_string(),
                );
                c.check(
                    has(got, "/dev/js/ab.min.js"),
                    "slices.Contains(got, \"/dev/js/ab.min.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "min js files included when pattern mentions .min.",
            host: case_insensitive_host,
            extensions: strs(&[".js"]),
            includes: strs(&["js/*.min.*"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &2);
                c.check(
                    has(got, "/dev/js/d.min.js"),
                    "slices.Contains(got, \"/dev/js/d.min.js\")".to_string(),
                );
                c.check(
                    has(got, "/dev/js/ab.min.js"),
                    "slices.Contains(got, \"/dev/js/ab.min.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude literal node_modules folder",
            host: common_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["node_modules"]),
            includes: strs(&["**/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/node_modules/a.ts"),
                    "!slices.Contains(got, \"/dev/node_modules/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "same named declarations include ts",
            host: same_named_declarations_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*.ts"]),
            expect: |c, got| {
                c.check(got.len() > 0, "len(got) > 0".to_string());
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "same named declarations include tsx",
            host: same_named_declarations_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*.tsx"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        f.ends_with(".tsx"),
                        format!("should only have .tsx files: {}", f),
                    );
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "empty includes returns all matching files",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            expect: |c, got| {
                c.check(got.len() > 0, "len(got) > 0".to_string());
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "nil extensions returns all files",
            host: case_insensitive_host,
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/a.js"),
                    "slices.Contains(got, \"/dev/a.js\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "empty extensions slice returns all files",
            host: case_insensitive_host,
            extensions: strs(&[]),
            expect: |c, got| {
                c.check(got.len() > 0, "expected files to be returned".to_string());
            },
            ..ReadDirTestCase::default()
        },
    ];
    run_cases("TestReadDirectory", cases);
}

// Go: vfsmatch_test.go:771 TestReadDirectoryEdgeCases
#[test]
fn test_read_directory_edge_cases() {
    let cases: Vec<ReadDirTestCase> = vec![
        ReadDirTestCase {
            name: "rooted include path",
            host: case_insensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["/dev/a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include with extension in path",
            host: case_insensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["a.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "special regex characters in path",
            host: || {
                from_map(
                    vec![
                        ("/dev/file+test.ts", MapFile::from("")),
                        ("/dev/file[0].ts", MapFile::from("")),
                        ("/dev/file(1).ts", MapFile::from("")),
                        ("/dev/file$money.ts", MapFile::from("")),
                        ("/dev/file^start.ts", MapFile::from("")),
                        ("/dev/file|pipe.ts", MapFile::from("")),
                        ("/dev/file#hash.ts", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".ts"]),
            includes: strs(&["file+test.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/file+test.ts"),
                    "slices.Contains(got, \"/dev/file+test.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include pattern starting with question mark",
            host: case_insensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["?.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include pattern starting with star",
            host: case_insensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["*b.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "case insensitive file matching",
            host: || {
                from_map(
                    vec![
                        ("/dev/File.ts", MapFile::from("")),
                        ("/dev/FILE.ts", MapFile::from("")),
                    ],
                    true,
                )
            },
            extensions: strs(&[".ts"]),
            includes: strs(&["*.ts"]),
            expect: |c, got| {
                c.check(got.len() == 2, "len(got) == 2".to_string());
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "nested subdirectory base path",
            host: case_sensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["q/a/c/b/d.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/q/a/c/b/d.ts"),
                    "slices.Contains(got, \"/dev/q/a/c/b/d.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "current directory differs from path",
            host: case_insensitive_host,
            extensions: strs(&[".ts"]),
            includes: strs(&["z/*.ts"]),
            expect: |c, got| {
                c.check(got.len() > 0, "len(got) > 0".to_string());
            },
            ..ReadDirTestCase::default()
        },
    ];
    run_cases("TestReadDirectoryEdgeCases", cases);
}

// Go: vfsmatch_test.go:859 TestReadDirectoryEmptyIncludes
#[test]
fn test_read_directory_empty_includes() {
    let cases: Vec<ReadDirTestCase> = vec![ReadDirTestCase {
        name: "empty includes slice behavior",
        host: || from_map(vec![("/root/a.ts", MapFile::from(""))], true),
        path: "/root",
        current_dir: "/",
        extensions: strs(&[".ts"]),
        includes: strs(&[]),
        expect: |c, got| {
            if got.len() == 0 {
                return;
            }
            c.check(
                has(got, "/root/a.ts"),
                "slices.Contains(got, \"/root/a.ts\")".to_string(),
            );
        },
        ..ReadDirTestCase::default()
    }];
    run_cases("TestReadDirectoryEmptyIncludes", cases);
}

// Go: vfsmatch_test.go:893 TestReadDirectorySymlinkCycle
#[test]
fn test_read_directory_symlink_cycle() {
    let cases: Vec<ReadDirTestCase> = vec![ReadDirTestCase {
        name: "detects and skips symlink cycles",
        host: || {
            from_map(
                vec![
                    ("/root/file.ts", MapFile::from("")),
                    ("/root/a/file.ts", MapFile::from("")),
                    ("/root/a/b", symlink("/root/a")),
                ],
                true,
            )
        },
        path: "/root",
        current_dir: "/",
        extensions: strs(&[".ts"]),
        includes: strs(&["**/*"]),
        expect: |c, got| {
            let expected = strs(&["/root/file.ts", "/root/a/file.ts"]);
            c.check_eq(got, &expected);
        },
        ..ReadDirTestCase::default()
    }];
    run_cases("TestReadDirectorySymlinkCycle", cases);
}

// Go: vfsmatch_test.go:926 TestReadDirectoryMatchesTypeScriptBaselines
#[test]
fn test_read_directory_matches_type_script_baselines() {
    let cases: Vec<ReadDirTestCase> = vec![
        ReadDirTestCase {
            name: "sorted in include order then alphabetical",
            host: || {
                from_map(
                    vec![
                        ("/dev/z/a.ts", MapFile::from("")),
                        ("/dev/z/aba.ts", MapFile::from("")),
                        ("/dev/z/abz.ts", MapFile::from("")),
                        ("/dev/z/b.ts", MapFile::from("")),
                        ("/dev/z/bba.ts", MapFile::from("")),
                        ("/dev/z/bbz.ts", MapFile::from("")),
                        ("/dev/x/a.ts", MapFile::from("")),
                        ("/dev/x/aa.ts", MapFile::from("")),
                        ("/dev/x/b.ts", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["z/*.ts", "x/*.ts"]),
            expect: |c, got| {
                let expected = strs(&[
                    "/dev/z/a.ts",
                    "/dev/z/aba.ts",
                    "/dev/z/abz.ts",
                    "/dev/z/b.ts",
                    "/dev/z/bba.ts",
                    "/dev/z/bbz.ts",
                    "/dev/x/a.ts",
                    "/dev/x/aa.ts",
                    "/dev/x/b.ts",
                ]);
                c.check_eq(got, &expected);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "recursive wildcards match dotted directories",
            host: || {
                from_map(
                    vec![
                        ("/dev/x/d.ts", MapFile::from("")),
                        ("/dev/x/y/d.ts", MapFile::from("")),
                        ("/dev/x/y/.e.ts", MapFile::from("")),
                        ("/dev/x/.y/a.ts", MapFile::from("")),
                        ("/dev/.z/.b.ts", MapFile::from("")),
                        ("/dev/.z/c.ts", MapFile::from("")),
                        ("/dev/w/.u/e.ts", MapFile::from("")),
                        ("/dev/g.min.js/.g/g.ts", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/.*/*"]),
            expect: |c, got| {
                let expected = strs(&[
                    "/dev/.z/c.ts",
                    "/dev/g.min.js/.g/g.ts",
                    "/dev/w/.u/e.ts",
                    "/dev/x/.y/a.ts",
                ]);
                c.check_eq(&got.len(), &expected.len());
                for want in expected.iter() {
                    c.check(has(got, &want), "slices.Contains(got, want)".to_string());
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "common package folders implicitly excluded with wildcard",
            host: || {
                from_map(
                    vec![
                        ("/dev/a.ts", MapFile::from("")),
                        ("/dev/a.d.ts", MapFile::from("")),
                        ("/dev/a.js", MapFile::from("")),
                        ("/dev/b.ts", MapFile::from("")),
                        ("/dev/x/a.ts", MapFile::from("")),
                        ("/dev/node_modules/a.ts", MapFile::from("")),
                        ("/dev/bower_components/a.ts", MapFile::from("")),
                        ("/dev/jspm_packages/a.ts", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/a.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts", "/dev/x/a.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "js wildcard excludes min js files",
            host: || {
                from_map(
                    vec![
                        ("/dev/js/a.js", MapFile::from("")),
                        ("/dev/js/b.js", MapFile::from("")),
                        ("/dev/js/d.min.js", MapFile::from("")),
                        ("/dev/js/ab.min.js", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".js"]),
            includes: strs(&["js/*"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/js/a.js", "/dev/js/b.js"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "explicit min js pattern includes min files",
            host: || {
                from_map(
                    vec![
                        ("/dev/js/a.js", MapFile::from("")),
                        ("/dev/js/b.js", MapFile::from("")),
                        ("/dev/js/d.min.js", MapFile::from("")),
                        ("/dev/js/ab.min.js", MapFile::from("")),
                    ],
                    false,
                )
            },
            extensions: strs(&[".js"]),
            includes: strs(&["js/*.min.js"]),
            expect: |c, got| {
                let expected = strs(&["/dev/js/ab.min.js", "/dev/js/d.min.js"]);
                c.check_eq(&got.len(), &expected.len());
                for want in expected.iter() {
                    c.check(has(got, &want), "slices.Contains(got, want)".to_string());
                }
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "literal excludes baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["b.ts"]),
            includes: strs(&["a.ts", "b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "wildcard excludes baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["*.ts", "z/??z.ts", "*/b.ts"]),
            includes: strs(&["a.ts", "b.ts", "z/a.ts", "z/abz.ts", "z/aba.ts", "x/b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/z/a.ts", "/dev/z/aba.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "recursive excludes baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**/b.ts"]),
            includes: strs(&["a.ts", "b.ts", "x/a.ts", "x/b.ts", "x/y/a.ts", "x/y/b.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/a.ts", "/dev/x/a.ts", "/dev/x/y/a.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "question mark baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/?.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/x/a.ts", "/dev/x/b.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "recursive directory pattern baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/a.ts"]),
            expect: |c, got| {
                c.check_eq(
                    got,
                    &strs(&["/dev/a.ts", "/dev/x/a.ts", "/dev/x/y/a.ts", "/dev/z/a.ts"]),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "case sensitive baseline",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/A.ts"]),
            expect: |c, got| {
                c.check_eq(got, &strs(&["/dev/A.ts"]));
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude folders baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["z", "x"]),
            includes: strs(&["**/*"]),
            expect: |c, got| {
                for f in got.iter() {
                    c.check(
                        !f.contains("/z/") && !f.contains("/x/"),
                        format!("should not contain z or x: {}", f),
                    );
                }
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/b.ts"),
                    "slices.Contains(got, \"/dev/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "implicit glob expansion baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["z"]),
            expect: |c, got| {
                c.check_eq(
                    got,
                    &strs(&[
                        "/dev/z/a.ts",
                        "/dev/z/aba.ts",
                        "/dev/z/abz.ts",
                        "/dev/z/b.ts",
                        "/dev/z/bba.ts",
                        "/dev/z/bbz.ts",
                    ]),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "trailing recursive directory baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude trailing recursive directory baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**"]),
            includes: strs(&["**/*"]),
            expect: |c, got| {
                c.check_eq(&got.len(), &0);
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "multiple recursive directory patterns baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/x/**/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/aa.ts"),
                    "slices.Contains(got, \"/dev/x/aa.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/b.ts"),
                    "slices.Contains(got, \"/dev/x/b.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/a.ts"),
                    "slices.Contains(got, \"/dev/x/y/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/b.ts"),
                    "slices.Contains(got, \"/dev/x/y/b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include dirs with starstar prefix baseline",
            host: case_sensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["**/x", "**/a/**/b"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/a.ts"),
                    "slices.Contains(got, \"/dev/x/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/b.ts"),
                    "slices.Contains(got, \"/dev/x/b.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/q/a/c/b/d.ts"),
                    "slices.Contains(got, \"/dev/q/a/c/b/d.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "dotted folders not implicitly included baseline",
            host: dotted_folders_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["x/**/*", "w/*/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/x/d.ts"),
                    "slices.Contains(got, \"/dev/x/d.ts\")".to_string(),
                );
                c.check(
                    has(got, "/dev/x/y/d.ts"),
                    "slices.Contains(got, \"/dev/x/y/d.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/.y/a.ts"),
                    "!slices.Contains(got, \"/dev/x/.y/a.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/x/y/.e.ts"),
                    "!slices.Contains(got, \"/dev/x/y/.e.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/dev/w/.u/e.ts"),
                    "!slices.Contains(got, \"/dev/w/.u/e.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include paths outside project baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            includes: strs(&["*", "/ext/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/dev/a.ts"),
                    "slices.Contains(got, \"/dev/a.ts\")".to_string(),
                );
                c.check(
                    has(got, "/ext/ext.ts"),
                    "slices.Contains(got, \"/ext/ext.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "include files with double dots baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["**"]),
            includes: strs(&["/ext/b/a..b.ts"]),
            expect: |c, got| {
                c.check(
                    has(got, "/ext/b/a..b.ts"),
                    "slices.Contains(got, \"/ext/b/a..b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
        ReadDirTestCase {
            name: "exclude files with double dots baseline",
            host: case_insensitive_host,
            extensions: strs(&[".ts", ".tsx", ".d.ts"]),
            excludes: strs(&["/ext/b/a..b.ts"]),
            includes: strs(&["/ext/**/*"]),
            expect: |c, got| {
                c.check(
                    has(got, "/ext/ext.ts"),
                    "slices.Contains(got, \"/ext/ext.ts\")".to_string(),
                );
                c.check(
                    !has(got, "/ext/b/a..b.ts"),
                    "!slices.Contains(got, \"/ext/b/a..b.ts\")".to_string(),
                );
            },
            ..ReadDirTestCase::default()
        },
    ];
    run_cases("TestReadDirectoryMatchesTypeScriptBaselines", cases);
}

// Go: vfsmatch_test.go:1200 TestReadDirectoryWithExtendedDynamicRoot (ts#64544)
#[test]
fn test_read_directory_with_extended_dynamic_root() {
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let host = from_map(
        vec![
            (format!("{PACKAGE_DIRECTORY}/value.d.ts"), MapFile::from("")),
            (
                format!("{PACKAGE_DIRECTORY}/node_modules/dep/index.d.ts"),
                MapFile::from(""),
            ),
        ],
        true,
    );
    let got = read_directory(
        host.as_ref(),
        PACKAGE_DIRECTORY,
        PACKAGE_DIRECTORY,
        &strs(&[".d.ts"]),
        &[],
        &strs(&["**/*"]),
        UNLIMITED_DEPTH,
    );
    assert_eq!(got, vec![format!("{PACKAGE_DIRECTORY}/value.d.ts")]);
}

// Go: vfsmatch_test.go:1219 TestReadDirectoryDynamicRootUsesCaseSensitivePatterns (ts#64159)
// PORT: Go wraps the case-sensitive map FS in `caseInsensitiveMatchFS`
// (vfsmatch_test.go:1253); here `wrapvfs_wrap` replaces only its case
// sensitivity.
#[test]
fn test_read_directory_dynamic_root_uses_case_sensitive_patterns() {
    use ts_goport::frontend::vfs::{Replacements, wrapvfs_wrap};
    const ROOT: &str = "^/~ts-uri~/custom/ts-nul-authority";
    let host = wrapvfs_wrap(
        from_map(
            vec![
                (format!("{ROOT}/Foo/a.ts"), MapFile::from("")),
                (format!("{ROOT}/foo/b.ts"), MapFile::from("")),
            ],
            true,
        ),
        Replacements {
            use_case_sensitive_file_names: Some(Box::new(|| false)),
            ..Default::default()
        },
    );
    let base = format!("{ROOT}/");
    let got = read_directory(
        host.as_ref(),
        &base,
        &base,
        &strs(&[".ts"]),
        &[],
        &strs(&["Foo/**/*.ts"]),
        UNLIMITED_DEPTH,
    );
    assert_eq!(got, vec![format!("{ROOT}/Foo/a.ts")]);
}

// Go: vfsmatch_test.go:1238 TestDynamicAbsoluteGlobUsesCaseSensitivePattern (ts#64544)
#[test]
fn test_dynamic_absolute_glob_uses_case_sensitive_pattern() {
    const ROOT: &str = "^/~ts-uri~/custom/ts-nul-authority";
    let matcher = new_spec_matcher(
        &strs(&[&format!("{ROOT}/Foo/**/*.ts")]),
        "/dev",
        Usage::Files,
        false,
    )
    .expect("matcher != nil");
    assert!(matcher.match_string(&format!("{ROOT}/Foo/a.ts")));
    assert!(!matcher.match_string(&format!("{ROOT}/foo/a.ts")));
}

// Go: vfsmatch_test.go:740 TestIsImplicitGlob
#[test]
fn test_is_implicit_glob() {
    let tests = [
        ("simple", "foo", true),
        ("folder", "src", true),
        ("with extension", "foo.ts", false),
        ("trailing dot", "foo.", false),
        ("star", "*", false),
        ("question", "?", false),
        ("star suffix", "foo*", false),
        ("question suffix", "foo?", false),
        ("dot name", "foo.bar", false),
        ("empty", "", true),
    ];
    let mut failures = Failures::new("TestIsImplicitGlob");
    for (name, input, expected) in tests {
        failures.check_eq(name, is_implicit_glob(input), expected);
    }
    failures.finish();
}

// Go: vfsmatch_test.go:1195 TestSpecMatcher
#[test]
fn test_spec_matcher() {
    let cases: &[(&str, &[&str], Usage, bool, &[&str], &[&str])] = &[
        (
            "simple wildcard",
            &["*.ts"],
            Usage::Files,
            true,
            &["/project/a.ts", "/project/b.ts", "/project/foo.ts"],
            &["/project/a.js", "/project/sub/a.ts"],
        ),
        (
            "recursive wildcard",
            &["**/*.ts"],
            Usage::Files,
            true,
            &[
                "/project/a.ts",
                "/project/sub/a.ts",
                "/project/sub/deep/a.ts",
            ],
            &["/project/a.js"],
        ),
        (
            "exclude pattern",
            &["node_modules"],
            Usage::Exclude,
            true,
            &["/project/node_modules/foo"],
            &["/project/node_modules", "/project/src"],
        ),
        (
            "case insensitive",
            &["*.ts"],
            Usage::Files,
            false,
            &["/project/A.TS", "/project/B.Ts"],
            &["/project/a.js"],
        ),
        (
            "multiple specs",
            &["*.ts", "*.tsx"],
            Usage::Files,
            true,
            &["/project/a.ts", "/project/b.tsx"],
            &["/project/a.js"],
        ),
    ];
    let mut failures = Failures::new("TestSpecMatcher");
    for (name, specs, usage, cs, matching, non_matching) in cases {
        let Some(matcher) = new_spec_matcher(&strs(specs), "/project", *usage, *cs) else {
            failures.fail(name, "matcher should not be nil".into());
            continue;
        };
        for path in *matching {
            if !matcher.match_string(path) {
                failures.fail(name, format!("should match: {path}"));
            }
        }
        for path in *non_matching {
            if matcher.match_string(path) {
                failures.fail(name, format!("should not match: {path}"));
            }
        }
    }
    failures.finish();
}

/// Go tests that check `MatchString` of each path against `expected`.
fn check_match_string(test: &str, cases: &[(&str, &[&str], Usage, &[&str], &[bool])]) {
    let mut failures = Failures::new(test);
    for (name, specs, usage, paths, expected) in cases {
        assert_eq!(paths.len(), expected.len());
        let Some(m) = new_spec_matcher(&strs(specs), "/project", *usage, true) else {
            failures.fail(name, "matcher is nil".into());
            continue;
        };
        for (path, want) in paths.iter().zip(expected.iter()) {
            failures.check_eq(&format!("{name} path: {path}"), m.match_string(path), *want);
        }
    }
    failures.finish();
}

// Go: vfsmatch_test.go:1271 TestSpecMatcher_MatchString
#[test]
fn test_spec_matcher_match_string() {
    check_match_string(
        "TestSpecMatcher_MatchString",
        &[
            (
                "simple wildcard files",
                &["*.ts"],
                Usage::Files,
                &["/project/a.ts", "/project/sub/a.ts", "/project/a.js"],
                &[true, false, false],
            ),
            (
                "recursive wildcard files",
                &["**/*.ts"],
                Usage::Files,
                &["/project/a.ts", "/project/sub/a.ts", "/project/a.js"],
                &[true, true, false],
            ),
            (
                "exclude pattern matches prefix",
                &["node_modules"],
                Usage::Exclude,
                &[
                    "/project/node_modules",
                    "/project/node_modules/foo",
                    "/project/src",
                ],
                &[false, true, false],
            ),
        ],
    );
}

// Go: vfsmatch_test.go:1325 TestSingleSpecMatcher_MatchString
#[test]
fn test_single_spec_matcher_match_string() {
    check_match_string(
        "TestSingleSpecMatcher_MatchString",
        &[
            (
                "single spec wildcard",
                &["*.ts"],
                Usage::Files,
                &["/project/a.ts", "/project/sub/a.ts", "/project/a.js"],
                &[true, false, false],
            ),
            (
                "single spec trailing starstar exclude allowed",
                &["**"],
                Usage::Exclude,
                &["/project/a.ts", "/project/sub/a.ts"],
                &[true, true],
            ),
        ],
    );
}

// Go: vfsmatch_test.go:1370 TestSpecMatchers_MatchIndex
#[test]
fn test_spec_matchers_match_index() {
    let cases: &[(&str, &[&str], Usage, &[&str], &[i32])] = &[
        (
            "index lookup prefers first match",
            &["*.ts", "*.tsx"],
            Usage::Files,
            &["/project/a.ts", "/project/a.tsx", "/project/a.js"],
            &[0, 1, -1],
        ),
        (
            "exclude index lookup",
            &["node_modules", "bower_components"],
            Usage::Exclude,
            &[
                "/project/node_modules",
                "/project/node_modules/foo",
                "/project/bower_components",
                "/project/bower_components/bar",
                "/project/src",
            ],
            &[-1, 0, -1, 1, -1],
        ),
    ];
    let mut failures = Failures::new("TestSpecMatchers_MatchIndex");
    for (name, specs, usage, paths, expected) in cases {
        let Some(m) = new_spec_matcher(&strs(specs), "/project", *usage, true) else {
            failures.fail(name, "matcher is nil".into());
            continue;
        };
        for (path, want) in paths.iter().zip(expected.iter()) {
            failures.check_eq(&format!("{name} path: {path}"), m.match_index(path), *want);
        }
    }
    failures.finish();
}

// Go: vfsmatch_test.go:1415 TestSingleSpecMatcher
#[test]
fn test_single_spec_matcher() {
    // simple spec
    let m = new_spec_matcher(&strs(&["*.ts"]), "/project", Usage::Files, true)
        .expect("matcher should not be nil");
    assert!(
        m.match_string("/project/a.ts"),
        "should match: /project/a.ts"
    );
    assert!(
        !m.match_string("/project/a.js"),
        "should not match: /project/a.js"
    );
    // trailing ** non-exclude returns nil
    assert!(
        new_spec_matcher(&strs(&["**"]), "/project", Usage::Files, true).is_none(),
        "should be nil"
    );
    // trailing ** exclude works
    let m = new_spec_matcher(&strs(&["**"]), "/project", Usage::Exclude, true)
        .expect("matcher should not be nil");
    for path in ["/project/anything", "/project/deep/path"] {
        assert!(m.match_string(path), "should match: {path}");
    }
}

// Go: vfsmatch_test.go:1476 TestSpecMatchers
#[test]
fn test_spec_matchers() {
    // multiple specs return correct index
    let m = new_spec_matcher(
        &strs(&["*.ts", "*.tsx", "*.js"]),
        "/project",
        Usage::Files,
        true,
    )
    .expect("matchers should not be nil");
    for (path, want) in [
        ("/project/a.ts", 0),
        ("/project/b.tsx", 1),
        ("/project/c.js", 2),
        ("/project/d.css", -1),
    ] {
        assert_eq!(m.match_index(path), want, "path: {path}");
    }
    // empty specs returns nil
    assert!(
        new_spec_matcher(&[], "/project", Usage::Files, true).is_none(),
        "should be nil"
    );
}

/// Go `compileGlobPattern(spec, "/", UsageFiles, true)`, `ok` asserted.
fn compile(spec: &str) -> ts_goport::frontend::vfs::SpecMatcher {
    new_spec_matcher(&strs(&[spec]), "/", Usage::Files, true)
        .unwrap_or_else(|| panic!("compileGlobPattern({spec:?}) not ok"))
}

// Go: vfsmatch_test.go:1532 TestGlobPatternInternals (the subtests that do
// not need a private item)
#[test]
fn test_glob_pattern_internals() {
    // question mark segment at end of string
    let p = compile("a?");
    assert!(p.match_string("/ab"));
    assert!(!p.match_string("/a"));

    // star segment with complex pattern
    let p = compile("a*b*c");
    assert!(p.match_string("/abc"));
    assert!(p.match_string("/aXbYc"));
    assert!(p.match_string("/aXXXbYYYc"));
    assert!(!p.match_string("/aXbY"));

    // literal component with package folder in include
    let host = from_map([("/dev/node_modules/pkg/index.ts", "")], false);
    let got = read_directory(
        host.as_ref(),
        "/",
        "/dev",
        &strs(&[".ts"]),
        &[],
        &strs(&["node_modules/pkg/index.ts"]),
        UNLIMITED_DEPTH,
    );
    assert!(has(&got, "/dev/node_modules/pkg/index.ts"), "{got:?}");
}

// Go: vfsmatch_test.go:1695 TestMatchSegmentsEdgeCases
#[test]
fn test_match_segments_edge_cases() {
    let check = |spec: &str, yes: &[&str], no: &[&str]| {
        let p = compile(spec);
        for path in yes {
            assert!(p.match_string(path), "{spec} should match {path}");
        }
        for path in no {
            assert!(!p.match_string(path), "{spec} should not match {path}");
        }
    };
    // question mark before slash in string
    check("a?b", &["/aXb"], &["/ab", "/aXYb"]);
    // star with no trailing content
    check("a*", &["/a", "/abc", "/aXYZ"], &[]);
    // multiple stars in pattern
    check("*a*", &["/a", "/Xa", "/aX", "/XaY"], &["/XYZ"]);
    // multiple stars requiring backtracking
    check(
        "*a*a",
        &["/aa", "/Xaa", "/aXa", "/XaYa", "/aaaa"],
        &["/a", "/Xa", "/aX", "/XaYaZ"],
    );
    check(
        "*a*b*c",
        &["/abc", "/XaYbZc", "/aXbYc", "/aaabbbccc"],
        &["/ab", "/ac", "/cba", "/abcX"],
    );
    check("*a*a*a", &["/aaa", "/aXaYa", "/XaYaZa"], &["/aa", "/aaX"]);
    check(
        "a*b*a",
        &["/aba", "/aXbYa", "/abba"],
        &["/ab", "/aba ", "/Xaba"],
    );
    // pathological pattern performance
    check(
        "*a*a*a*a*b",
        &["/aaaab", "/XaYaZaWab"],
        &["/aaaaaaaaaaaaaaaa", "/aaaaaaaaaaaaaaaaX"],
    );
    // literal segment not matching
    check("abcdefgh.ts", &["/abcdefgh.ts"], &["/abc.ts"]);
    // question mark matches multi-byte unicode rune
    check(
        "?.ts",
        &["/a.ts", "/é.ts", "/中.ts", "/🎉.ts"],
        &["/.ts", "/ab.ts"],
    );
    check(
        "??.ts",
        &["/ab.ts", "/é中.ts", "/🎉é.ts"],
        &["/a.ts", "/abc.ts"],
    );
    // star matches multi-byte unicode runes correctly
    check("*é.ts", &["/é.ts", "/café.ts"], &["/cafe.ts"]);
    check("*🎉*", &["/🎉", "/a🎉b"], &["/abc"]);
}

fn match_files_dev(host: &Rc<dyn Fs>, includes: &[&str]) -> Vec<String> {
    read_directory(
        host.as_ref(),
        "/",
        "/dev",
        &strs(&[".ts"]),
        &[],
        &strs(includes),
        UNLIMITED_DEPTH,
    )
}

// Go: vfsmatch_test.go:1863 TestReadDirectoryConsecutiveSlashes
#[test]
fn test_read_directory_consecutive_slashes() {
    let host = from_map([("/dev/a.ts", ""), ("/dev/x/b.ts", "")], false);
    let got = match_files_dev(&host, &["**/*.ts"]);
    assert!(got.len() >= 2, "should find files");
    assert!(has(&got, "/dev/a.ts"));
    assert!(has(&got, "/dev/x/b.ts"));
}

// Go: vfsmatch_test.go:1879 TestGlobPatternLiteralWithPackageFolders
#[test]
fn test_glob_pattern_literal_with_package_folders() {
    // wildcard skips package folders
    let host = from_map([("/dev/a.ts", ""), ("/dev/node_modules/b.ts", "")], false);
    let got = match_files_dev(&host, &["*/*.ts"]);
    assert!(
        !has(&got, "/dev/node_modules/b.ts"),
        "should skip node_modules with wildcard"
    );

    // explicit literal includes package folder
    let host = from_map([("/dev/node_modules/b.ts", "")], false);
    let got = match_files_dev(&host, &["node_modules/b.ts"]);
    assert!(
        has(&got, "/dev/node_modules/b.ts"),
        "should include explicit node_modules path"
    );
}
