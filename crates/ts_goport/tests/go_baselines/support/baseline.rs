//! Port of internal/testutil/baseline (baseline.go and the tracking part of
//! testmain.go): compare a generated baseline with the pinned
//! typescript-go reference baseline.
//!
//! Environment:
//! - `TS_GO_REPO`: the Go checkout (default `DEFAULT_GO_REPO`), in either
//!   layout (`is_merged_layout`). The reference root is
//!   `testdata/baselines/reference`. At the typescript-go layout the
//!   submodule reference root is
//!   `_submodules/TypeScript/tests/baselines/reference`.
//! - `TS_GOPORT_BASELINE_LOCAL`: unset (or empty) compares only. `1` writes
//!   every generated baseline under `DEFAULT_LOCAL_ROOT` in the Go layout
//!   (`<subfolder>/<name>`, the submodule diff files, `.delete` markers).
//!   Any other value is the local root.
//! - `TS_GOPORT_BASELINE_TRACK=<file>`: appends `<subfolder>/<name>` of each
//!   compared baseline to the file (Go `baseline.Track`).
//!
//! PORT: Go reports through `t.Errorf` and `t.Fatalf`. `run` returns every
//! message, joined by newlines, as the `Err`. A mismatch message names the file and shows the first differing
//! line, never the whole file.
//!
//! PORT: the generated text is a port form string (see
//! `ts_goport::scanner_util::GO_STRING_MARKER`). It is compared and written
//! as its Go bytes (`go_string_bytes`), like the Go string.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ts_goport::scanner_util::{go_string_bytes, go_string_from_bytes};

use super::patience;

/// The pinned typescript-go checkout.
pub const DEFAULT_GO_REPO: &str = "/home/theo/.explore/repos/microsoft__typescript-go";
/// The local baseline root for `TS_GOPORT_BASELINE_LOCAL=1`.
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_LOCAL_ROOT: &str =
    "/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/go-baseline-tests/local";
/// macOS: `/home` is an autofs mount there, so the default is under /tmp.
#[cfg(target_os = "macos")]
pub const DEFAULT_LOCAL_ROOT: &str = "/tmp/ts-rust-go-baseline-tests/local";
pub const GO_REPO_ENV: &str = "TS_GO_REPO";
pub const LOCAL_ENV: &str = "TS_GOPORT_BASELINE_LOCAL";
pub const TRACK_ENV: &str = "TS_GOPORT_BASELINE_TRACK";

/// A Go `func(string) string` diff fixup.
pub type DiffFixup = Arc<dyn Fn(&str) -> String + Send + Sync>;

// Go: testutil/baseline/baseline.go:14 Options
#[derive(Clone, Default)]
pub struct Options {
    pub subfolder: String,
    pub is_submodule: bool,
    pub is_submodule_accepted: bool,
    pub is_submodule_triaged: bool,
    pub diff_fixup_old: Option<DiffFixup>,
    pub diff_fixup_new: Option<DiffFixup>,
    pub skip_diff_with_old: bool,
}

// Go: testutil/baseline/baseline.go:21 NoContent
pub const NO_CONTENT: &str = "<no content>";

/// The typescript-go checkout: `TS_GO_REPO`, or `DEFAULT_GO_REPO`.
pub fn go_repo() -> PathBuf {
    match std::env::var_os(GO_REPO_ENV) {
        Some(repo) if !repo.is_empty() => PathBuf::from(repo),
        _ => PathBuf::from(DEFAULT_GO_REPO),
    }
}

/// Whether the Go checkout has the microsoft/TypeScript `tsc/` layout
/// (5f647a841a, "Apply the TypeScript 7 repository layout"): no
/// `_submodules/TypeScript`, `TestLocal` runs every case under
/// `testdata/tests/cases`, the transpile cases are in
/// `testdata/tests/cases/transpile`, the `submodule*` reference dirs are
/// merged into `reference/<suite>` and there are no `.diff` files. Its
/// `go.mod` module is `github.com/microsoft/TypeScript/tsc`. Any other
/// checkout has the typescript-go layout (pin B and older).
// PORT: no Go equivalent. Go has one layout per commit; the port runs at
// pins of both layouts.
pub fn is_merged_layout() -> bool {
    static MERGED: OnceLock<bool> = OnceLock::new();
    *MERGED.get_or_init(|| {
        std::fs::read_to_string(go_repo().join("go.mod")).is_ok_and(|go_mod| {
            go_mod
                .lines()
                .any(|line| line.trim() == "module github.com/microsoft/TypeScript/tsc")
        })
    })
}

// Go: internal/repo TestDataPath
pub fn test_data_path() -> PathBuf {
    go_repo().join("testdata")
}

// Go: testutil/baseline/baseline.go:84 referenceRoot
pub fn reference_root() -> PathBuf {
    test_data_path().join("baselines").join("reference")
}

// Go: testutil/baseline/baseline.go:249 submoduleReferenceRoot
// (typescript-go layout only)
pub fn submodule_reference_root() -> PathBuf {
    go_repo()
        .join("_submodules")
        .join("TypeScript")
        .join("tests")
        .join("baselines")
        .join("reference")
}

// Go: testutil/baseline/baseline.go:83 localRoot
/// The local baseline root, or `None` when local baselines are off.
// PORT: Go always writes changed baselines to `testdata/baselines/local`.
// The port writes only when `TS_GOPORT_BASELINE_LOCAL` is set.
pub fn local_root() -> Option<PathBuf> {
    let value = std::env::var_os(LOCAL_ENV)?;
    if value.is_empty() {
        None
    } else if value == "1" {
        Some(PathBuf::from(DEFAULT_LOCAL_ROOT))
    } else {
        Some(PathBuf::from(value))
    }
}

/// Go `filepath.Join` for the relative baseline paths: empty parts are
/// skipped.
// PORT: Go also cleans the path. The parts here are plain relative names.
fn join_rel(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

// Go: testutil/baseline/baseline.go:23 Run
// At the merged layout Go `Options` has no `IsSubmodule*` fields and `Run`
// is the first block only; the runners there never set `is_submodule`.
pub fn run(file_name: &str, actual: &str, opts: &Options) -> Result<(), String> {
    let mut errors = Vec::new();
    let orig_subfolder = opts.subfolder.as_str();

    {
        let subfolder = if opts.is_submodule {
            join_rel(&["submodule", opts.subfolder.as_str()])
        } else {
            opts.subfolder.clone()
        };

        let rel = join_rel(&[subfolder.as_str(), file_name]);

        // Record this baseline for tracking unused baselines
        record_baseline(&rel);

        write_comparison(&mut errors, actual, &rel, &reference_root().join(&rel));
    }

    if !opts.is_submodule || opts.skip_diff_with_old {
        // Not a submodule, no diffs.
        return finish(errors);
    }

    let submodule_reference = submodule_reference_root().join(file_name);
    let submodule_expected = read_file_or_no_content(&submodule_reference);

    const SUBMODULE_FOLDER: &str = "submodule";
    const SUBMODULE_ACCEPTED_FOLDER: &str = "submoduleAccepted";
    const SUBMODULE_TRIAGED_FOLDER: &str = "submoduleTriaged";

    let diff_file_name = format!("{file_name}.diff");
    let diff_key = format!("{orig_subfolder}/{diff_file_name}");
    let is_submodule_accepted =
        opts.is_submodule_accepted || submodule_accepted_file_names().contains(&diff_key);
    let is_submodule_triaged =
        opts.is_submodule_triaged || submodule_triaged_file_names().contains(&diff_key);

    if is_submodule_accepted && is_submodule_triaged {
        errors.push(format!(
            "diff file {orig_subfolder}/{diff_file_name} is in both submoduleAccepted and submoduleTriaged; it should only be in one"
        ));
        return finish(errors);
    }

    let out_root = if is_submodule_accepted {
        SUBMODULE_ACCEPTED_FOLDER
    } else if is_submodule_triaged {
        SUBMODULE_TRIAGED_FOLDER
    } else {
        SUBMODULE_FOLDER
    };

    let all_roots = [
        SUBMODULE_FOLDER,
        SUBMODULE_ACCEPTED_FOLDER,
        SUBMODULE_TRIAGED_FOLDER,
    ];

    let diff = get_baseline_diff(
        actual,
        &submodule_expected,
        file_name,
        opts.diff_fixup_old.as_deref(),
        opts.diff_fixup_new.as_deref(),
    );

    for root in all_roots {
        let rel = join_rel(&[root, orig_subfolder, diff_file_name.as_str()]);

        // Record this baseline for tracking unused baselines
        record_baseline(&rel);

        let reference = reference_root().join(&rel);
        if root == out_root {
            write_comparison(&mut errors, &diff, &rel, &reference);
        } else {
            write_comparison(&mut errors, NO_CONTENT, &rel, &reference);
        }
    }

    finish(errors)
}

fn finish(errors: Vec<String>) -> Result<(), String> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

// Go: testutil/baseline/baseline.go:100 submoduleAcceptedFileNames
fn submodule_accepted_file_names() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| read_file_name_set(&test_data_path().join("submoduleAccepted.txt")))
}

// Go: testutil/baseline/baseline.go:104 submoduleTriagedFileNames
fn submodule_triaged_file_names() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| read_file_name_set(&test_data_path().join("submoduleTriaged.txt")))
}

// Go: testutil/baseline/baseline.go:108 readFileNameSet
fn read_file_name_set(path: &Path) -> HashSet<String> {
    let mut set = HashSet::new();

    match std::fs::read(path) {
        Ok(content) => {
            let content = go_string_from_bytes(content);
            for line in content.split('\n') {
                // Go `strings.TrimSpace` trims Unicode white space, as `str::trim` does.
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                set.insert(line.to_string());
            }
        }
        Err(err) => panic!("failed to read file {}: {err}", path.display()),
    }

    set
}

// Go: testutil/baseline/baseline.go:126 readFileOrNoContent
fn read_file_or_no_content(file_name: &Path) -> String {
    match std::fs::read(file_name) {
        Ok(content) => go_string_from_bytes(content),
        Err(_) => NO_CONTENT.to_string(),
    }
}

// Go: testutil/baseline/baseline.go:31 DiffText
pub fn diff_text(old_name: &str, new_name: &str, expected: &str, actual: &str) -> String {
    let expected_lines = patience::split_lines(expected);
    let actual_lines = patience::split_lines(actual);
    let lines = patience::diff(&expected_lines, &actual_lines);
    patience::unified_diff_text_with_options(
        &lines,
        &patience::UnifiedDiffOptions {
            precontext: 3,
            postcontext: 3,
            src_header: old_name.to_string(),
            dst_header: new_name.to_string(),
        },
    )
}

// Go: testutil/baseline/baseline.go:144 getBaselineDiff
// PORT: both texts are normalized to the port form of their Go bytes first,
// so that equal Go strings compare equal.
fn get_baseline_diff(
    actual: &str,
    expected: &str,
    file_name: &str,
    fixup_old: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
    fixup_new: Option<&(dyn Fn(&str) -> String + Send + Sync)>,
) -> String {
    let normalize = |s: &str| go_string_from_bytes(go_string_bytes(s).into_owned());
    let mut expected = normalize(expected);
    let mut actual = normalize(actual);
    if let Some(fixup_old) = fixup_old {
        expected = fixup_old(&expected);
    }
    if let Some(fixup_new) = fixup_new {
        actual = fixup_new(&actual);
    }
    if actual == expected {
        return NO_CONTENT.to_string();
    }
    let s = diff_text(
        &format!("old.{file_name}"),
        &format!("new.{file_name}"),
        &expected,
        &actual,
    );

    // If the diff is empty (just headers, no hunks), return NoContent
    if !s.contains("@@") {
        return NO_CONTENT.to_string();
    }

    // Remove line numbers from unified diff headers; this avoids adding/deleting
    // lines in our baselines from causing knock-on header changes later in the diff.
    fix_unified_diff(&s)
}

// Go: testutil/baseline/baseline.go:166 (the ReplaceAllStringFunc callback)
// and :184 fixUnifiedDiff = regexp.MustCompile(`@@ -\d+,\d+ \+\d+,\d+ @@`)
// PORT: no regexp crate. Every match starts with "@@ -", so trying each
// such position from left to right finds the same non-overlapping matches.
fn fix_unified_diff(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut a_cur_line: i64 = 1;
    let mut b_cur_line: i64 = 1;
    let mut rest = s;
    while let Some(at) = rest.find("@@ -") {
        if let Some(([a_line, _a_line_count, b_line, _b_line_count], len)) =
            parse_unified_diff_header(&rest[at..])
        {
            out.push_str(&rest[..at]);
            let a_diff = a_line - a_cur_line;
            let b_diff = b_line - b_cur_line;
            a_cur_line = a_line;
            b_cur_line = b_line;

            // Keep surrounded by @@, to make GitHub's grammar happy.
            // https://github.com/textmate/diff.tmbundle/blob/0593bb775eab1824af97ef2172fd38822abd97d7/Syntaxes/Diff.plist#L68
            out.push_str(&format!("@@= skipped -{a_diff}, +{b_diff} lines =@@"));
            rest = &rest[at + len..];
        } else {
            // "@" is one byte.
            out.push_str(&rest[..=at]);
            rest = &rest[at + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Matches `@@ -\d+,\d+ \+\d+,\d+ @@` at the start of `s` and returns the
/// four numbers (Go `fmt.Sscanf(match, "@@ -%d,%d +%d,%d @@", ...)`) and
/// the match length.
fn parse_unified_diff_header(s: &str) -> Option<([i64; 4], usize)> {
    let b = s.as_bytes();
    let mut i = 0usize;
    let expect = |lit: &[u8], i: &mut usize| -> bool {
        if b.get(*i..*i + lit.len()) == Some(lit) {
            *i += lit.len();
            true
        } else {
            false
        }
    };
    let digits = |i: &mut usize| -> Option<i64> {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == start {
            return None;
        }
        // Go `Sscanf` fails on overflow, and the Go caller panics.
        Some(
            s[start..*i]
                .parse()
                .unwrap_or_else(|err| panic!("failed to parse unified diff header: {err}")),
        )
    };
    let mut numbers = [0i64; 4];
    if !expect(b"@@ -", &mut i) {
        return None;
    }
    numbers[0] = digits(&mut i)?;
    if !expect(b",", &mut i) {
        return None;
    }
    numbers[1] = digits(&mut i)?;
    if !expect(b" +", &mut i) {
        return None;
    }
    numbers[2] = digits(&mut i)?;
    if !expect(b",", &mut i) {
        return None;
    }
    numbers[3] = digits(&mut i)?;
    if !expect(b" @@", &mut i) {
        return None;
    }
    Some((numbers, i))
}

// Go `RunAgainstSubmodule` (typescript-go baseline.go:186) has no callers
// since microsoft/TypeScript 5f647a841a and is not ported.

// Go: testutil/baseline/baseline.go:41 writeComparison
// PORT: `rel` is the path under the local root (Go passes the full local
// path). With a local root, every generated baseline is written, not only a
// changed one, and a stale `.delete` marker is removed too.
// PORT: at the merged layout Go writes the `.delete` marker of a
// `NoContent` result whose reference exists and reports nothing. The port
// reports it at both layouts, as Go did at the typescript-go layout: the
// reference is the Go output, so a baseline that the port does not make is
// a port failure.
fn write_comparison(errors: &mut Vec<String>, actual_content: &str, rel: &str, reference: &Path) {
    assert!(
        !actual_content.is_empty(),
        "the generated content was \"\". Return 'baseline.NoContent' if no baselining is required."
    );

    let local = local_root().map(|root| root.join(rel));
    let local_delete = local.as_ref().map(|local| {
        let mut path = local.clone().into_os_string();
        path.push(".delete");
        PathBuf::from(path)
    });

    if let (Some(local), Some(local_delete)) = (&local, &local_delete) {
        if let Some(dir) = local.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            errors.push(format!(
                "failed to create directories for the local baseline file {}: {err}",
                local.display()
            ));
            return;
        }
        for path in [local, local_delete] {
            if path.exists()
                && let Err(err) = std::fs::remove_file(path)
            {
                errors.push(format!(
                    "failed to remove the local baseline file {}: {err}",
                    path.display()
                ));
                return;
            }
        }
    }

    let actual = go_string_bytes(actual_content);
    let (expected, found_expected) = match std::fs::read(reference) {
        Ok(content) => (content, true),
        Err(_) => (NO_CONTENT.as_bytes().to_vec(), false),
    };
    let changed = expected != *actual || (actual_content == NO_CONTENT && found_expected);

    if let (Some(local), Some(local_delete)) = (&local, &local_delete) {
        if actual_content == NO_CONTENT {
            if changed && let Err(err) = std::fs::write(local_delete, b"") {
                errors.push(format!(
                    "failed to write the local baseline file {}: {err}",
                    local_delete.display()
                ));
                return;
            }
        } else if let Err(err) = std::fs::write(local, &*actual) {
            errors.push(format!(
                "failed to write the local baseline file {}: {err}",
                local.display()
            ));
            return;
        }
    }

    if !changed {
        return;
    }

    if !found_expected {
        if let Some(local) = &local {
            errors.push(format!("new baseline created at {}.", local.display()));
        } else {
            // PORT: nothing is written without a local root.
            errors.push(format!(
                "new baseline {rel}: no reference file {}. Set {LOCAL_ENV}=1 to write it.",
                reference.display()
            ));
        }
    } else {
        // PORT: the Go hint to run `hereby baseline-accept` is left out.
        errors.push(format!(
            "the baseline file {} has changed.\n{}",
            reference.display(),
            first_difference(&expected, &actual)
        ));
    }
}

/// The longest part of a line that a mismatch message shows.
const MAX_SHOWN_CHARS: usize = 160;
/// How many chars before the first differing char a mismatch message shows.
const SHOWN_CONTEXT_CHARS: usize = 40;

/// A short description of the first line where `actual` differs from
/// `expected`: its number, the column of the first differing char, and
/// both lines around that column (escaped, truncated).
pub fn first_difference(expected: &[u8], actual: &[u8]) -> String {
    let mut expected_lines = expected.split(|&b| b == b'\n');
    let mut actual_lines = actual.split(|&b| b == b'\n');
    let mut line = 1usize;
    loop {
        let (e, a) = (expected_lines.next(), actual_lines.next());
        if e == a {
            if e.is_none() {
                return "no difference".to_string();
            }
            line += 1;
            continue;
        }
        let e_chars: Vec<char> = e
            .map(|l| String::from_utf8_lossy(l).chars().collect())
            .unwrap_or_default();
        let a_chars: Vec<char> = a
            .map(|l| String::from_utf8_lossy(l).chars().collect())
            .unwrap_or_default();
        let column = e_chars
            .iter()
            .zip(&a_chars)
            .take_while(|(x, y)| x == y)
            .count();
        return format!(
            "first difference at line {line}, column {}:\n  expected: {}\n  actual:   {}",
            column + 1,
            show_line(e.map(|_| e_chars.as_slice()), column),
            show_line(a.map(|_| a_chars.as_slice()), column),
        );
    }
}

fn show_line(line: Option<&[char]>, column: usize) -> String {
    let Some(chars) = line else {
        return "<end of file>".to_string();
    };
    let start = column.saturating_sub(SHOWN_CONTEXT_CHARS).min(chars.len());
    let end = (start + MAX_SHOWN_CHARS).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.push_str(&format!(
        "{:?}",
        chars[start..end].iter().collect::<String>()
    ));
    if end < chars.len() {
        out.push_str("...");
    }
    out
}

// Go: testutil/baseline/testmain.go:66 recordBaseline
/// Appends `relative_path` to the `TS_GOPORT_BASELINE_TRACK` file, once per
/// process.
// PORT: Go collects the paths in a set and writes one file per package when
// the tests end (`Track`). The port appends each new path at once under a
// process lock, so a crashed run keeps what it recorded. Different
// processes can append the same path; readers dedupe.
fn record_baseline(relative_path: &str) {
    static RECORDED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    let Some(tracking_file) = std::env::var_os(TRACK_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    let mut recorded = RECORDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !recorded
        .get_or_insert_with(HashSet::new)
        .insert(relative_path.to_string())
    {
        return;
    }
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&tracking_file)
        .and_then(|mut f| writeln!(f, "{relative_path}"));
    if let Err(err) = result {
        // Go: testmain.go:81 writeRecordedBaselines exits on a write error.
        panic!(
            "baseline: failed to write tracking file {}: {err}",
            Path::new(&tracking_file).display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_diff_text_and_header_rewrite() {
        let expected = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\n";
        let actual = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\nm\nn\n";
        assert_eq!(
            diff_text("old.x", "new.x", expected, actual),
            "--- old.x\n+++ new.x\n@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n d\n e\n@@ -9,5 +9,6 @@\n i\n j\n k\n-l\n+L\n m\n+n"
        );
        assert_eq!(
            get_baseline_diff(actual, expected, "x", None, None),
            "--- old.x\n+++ new.x\n@@= skipped -0, +0 lines =@@\n a\n-b\n+B\n c\n d\n e\n@@= skipped -8, +8 lines =@@\n i\n j\n k\n-l\n+L\n m\n+n"
        );
        // Repeated lines take the unique-element LCS path.
        assert_eq!(
            diff_text("o", "n", "x\ny\nz\nx\ny\nq\n", "y\nx\nz\nq\nx\n"),
            "--- o\n+++ n\n@@ -1,6 +1,5 @@\n-x\n y\n+x\n z\n-x\n-y\n q\n+x"
        );
        assert_eq!(
            get_baseline_diff("same", "same", "x", None, None),
            NO_CONTENT
        );
        assert_eq!(
            fix_unified_diff("@@ -1,2 @@ x @@ -3,4 +5,6 @@"),
            "@@ -1,2 @@ x @@= skipped -2, +4 lines =@@"
        );
    }

    #[test]
    fn baseline_first_difference() {
        assert_eq!(
            first_difference(b"a\nb\nc", b"a\nx\nc"),
            "first difference at line 2, column 1:\n  expected: \"b\"\n  actual:   \"x\""
        );
        assert_eq!(
            first_difference(b"a", b"a\nb"),
            "first difference at line 2, column 1:\n  expected: <end of file>\n  actual:   \"b\""
        );
    }
}
