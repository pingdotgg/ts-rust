//! The Go regular expressions of the compiler runner, written as plain
//! matchers. The test crate has no regex dependency, and each Go pattern is
//! small. Each function names its Go pattern and keeps the RE2 semantics
//! that the pattern needs: leftmost-first matches, greedy repeats, `\s` is
//! `[\t\n\f\r ]`, `\w` is `[0-9A-Za-z_]`, `\d` is `[0-9]` and `.` is any
//! char but `\n`.
//!
//! PORT: the texts are port form strings (see
//! `ts_goport::scanner_util::GO_STRING_MARKER`). The patterns only match
//! ASCII, so a port form unit never matches inside. `replace_non_whitespace`
//! reads the units, because RE2 replaces each rune.

use ts_goport::scanner_util::{GoUnit, go_unit_at};

/// RE2 `\s`.
fn is_space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

/// RE2 `\w`.
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The end of the `\s*` run that starts at `i`.
fn skip_spaces(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && is_space(bytes[i]) {
        i += 1;
    }
    i
}

/// The end of the `[^\r\n]*` run that starts at `i`.
fn skip_to_line_break(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\r' && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

/// Go `regexp.MustCompile("\r?\n").Split(s, -1)`.
pub fn split_line_delimiter(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let end = if i > start && bytes[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            lines.push(&s[start..end]);
            start = i + 1;
        }
        i += 1;
    }
    lines.push(&s[start..]);
    lines
}

/// One match of `(?m)^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)` that starts at
/// `start` (a line start): the name, the value and the match end.
fn option_at(s: &str, start: usize) -> Option<(&str, &str, usize)> {
    let bytes = s.as_bytes();
    if !s[start..].starts_with("//") {
        return None;
    }
    let mut i = skip_spaces(bytes, start + 2);
    if bytes.get(i) != Some(&b'@') {
        return None;
    }
    i += 1;
    let name_start = i;
    while i < bytes.len() && is_word(bytes[i]) {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let name = &s[name_start..i];
    i = skip_spaces(bytes, i);
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i = skip_spaces(bytes, i + 1);
    let value_end = skip_to_line_break(bytes, i);
    Some((name, &s[i..value_end], value_end))
}

/// Go `optionRegex.FindStringSubmatch(line)` for one line (no `\n`): the
/// name and the value.
pub fn match_option_line(line: &str) -> Option<(&str, &str)> {
    option_at(line, 0).map(|(name, value, _)| (name, value))
}

/// Go `optionRegex.FindAllStringSubmatch(content, -1)`: the name and the
/// value of each match, in order. `^` matches at the start and after each
/// `\n`. A match can run over line breaks (the `\s*` repeats), and the next
/// search starts at its end.
pub fn find_all_options(content: &str) -> Vec<(&str, &str)> {
    let bytes = content.as_bytes();
    let mut result = Vec::new();
    let mut pos = 0usize;
    while pos <= bytes.len() {
        // `pos` is a line start here.
        if let Some((name, value, end)) = option_at(content, pos) {
            result.push((name, value));
            pos = end;
        }
        // The next line start at or after `pos` (after a `\n`).
        match memchr_newline(bytes, pos) {
            Some(newline) => pos = newline + 1,
            None => break,
        }
    }
    result
}

fn memchr_newline(bytes: &[u8], from: usize) -> Option<usize> {
    bytes[from.min(bytes.len())..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| from + i)
}

/// Go `linkRegex.FindStringSubmatch(line)` with
/// `(?m)^\/{2}\s*@link\s*:\s*([^\r\n]*)\s*->\s*([^\r\n]*)` on one line:
/// the two groups.
pub fn match_link_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    if !line.starts_with("//") {
        return None;
    }
    let mut i = skip_spaces(bytes, 2);
    if !line[i..].starts_with("@link") {
        return None;
    }
    i = skip_spaces(bytes, i + "@link".len());
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    let first_start = skip_spaces(bytes, i + 1);
    let first_max = skip_to_line_break(bytes, first_start);
    // `([^\r\n]*)` is greedy: the longest first group after which
    // `\s*->` matches.
    let mut first_end = first_max;
    loop {
        let arrow = skip_spaces(bytes, first_end);
        if line[arrow..].starts_with("->") {
            let second_start = skip_spaces(bytes, arrow + 2);
            let second_end = skip_to_line_break(bytes, second_start);
            return Some((
                &line[first_start..first_end],
                &line[second_start..second_end],
            ));
        }
        if first_end == first_start {
            return None;
        }
        first_end -= 1;
        while !line.is_char_boundary(first_end) {
            first_end -= 1;
        }
    }
}

/// Go `referencesRegex.MatchString(s)` with `reference\spath`.
pub fn contains_reference_path(s: &str) -> bool {
    let bytes = s.as_bytes();
    let needle = b"reference";
    let mut from = 0usize;
    while let Some(i) = find(bytes, needle, from) {
        let after = i + needle.len();
        if after < bytes.len() && is_space(bytes[after]) && bytes[after + 1..].starts_with(b"path")
        {
            return true;
        }
        from = i + 1;
    }
    false
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|i| from + i)
}

/// Go `regexp.MustCompile(`\.tsx?$`).MatchString(s)`.
pub fn has_ts_or_tsx_suffix(s: &str) -> bool {
    s.ends_with(".ts") || s.ends_with(".tsx")
}

// Go: transpile_runner.go:23 transpileBaselineRegex
/// Go `regexp.MustCompile(`\.[cm]?[tj]sx?$`).MatchString(s)`.
pub fn has_transpile_test_suffix(s: &str) -> bool {
    let s = s.strip_suffix('x').unwrap_or(s);
    let Some(s) = s.strip_suffix("ts").or_else(|| s.strip_suffix("js")) else {
        return false;
    };
    let s = s
        .strip_suffix('c')
        .or_else(|| s.strip_suffix('m'))
        .unwrap_or(s);
    s.ends_with('.')
}

// Go: tsbaseline/contentmapper_baseline.go:17 ansiEscape (tsgo#4712)
/// Go `ansiEscape.ReplaceAllString(s, "")` with `\x1b\[[0-9;]*m`: removes
/// each ANSI color sequence.
pub fn replace_ansi_escapes(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if bytes.get(j) == Some(&b'm') {
                out.push_str(&s[copied..i]);
                i = j + 1;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

/// Go `tsExtension.ReplaceAllString(path, replacement)` with `\.tsx?$`.
pub fn replace_ts_extension(path: &str, replacement: &str) -> String {
    if let Some(stem) = path.strip_suffix(".tsx") {
        return format!("{stem}{replacement}");
    }
    if let Some(stem) = path.strip_suffix(".ts") {
        return format!("{stem}{replacement}");
    }
    path.to_string()
}

/// Go `nonWhitespace.ReplaceAllString(s, " ")` with `\S`: each rune that is
/// not RE2 white space becomes one space. RE2 reads each byte that is not
/// valid UTF-8 as one rune, so a port form unit becomes one space per Go
/// byte of an invalid byte and three for a lone surrogate.
pub fn replace_non_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        let (unit, size) = go_unit_at(s, i);
        match unit {
            GoUnit::Char(ch) if ch.is_ascii() && is_space(ch as u8) => out.push(ch),
            GoUnit::Surrogate(_) => out.push_str("   "),
            _ => out.push(' '),
        }
        i += size;
    }
    out
}

/// Go `utf8.RuneCountInString(s)` of the Go bytes of the port form `s`.
pub fn go_rune_count(s: &str) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while i < s.len() {
        let (unit, size) = go_unit_at(s, i);
        count += match unit {
            GoUnit::Surrogate(_) => 3,
            _ => 1,
        };
        i += size;
    }
    count
}

/// Go `testPathCharacters.ReplaceAllString(name, "_")` with `[\^<>:"|?*%]`.
pub fn replace_test_path_characters(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if matches!(ch, '^' | '<' | '>' | ':' | '"' | '|' | '?' | '*' | '%') {
                '_'
            } else {
                ch
            }
        })
        .collect()
}

/// Go `testPathDotDot.ReplaceAllString(path, "__dotdot/")` with `\.\.\/`.
pub fn replace_dot_dot_slash(path: &str) -> String {
    path.replace("../", "__dotdot/")
}

/// ASCII case-insensitive prefix test (RE2 `(?i)` for ASCII letters).
fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The end of `\d+` at `i`, or `None` when no digit is there.
fn digits_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    (j > i).then_some(j)
}

/// Matches `\.d\.ts` (case-insensitive) then `rest` at `i`; returns the
/// match end.
fn dts_then(bytes: &[u8], i: usize, rest: fn(&[u8], usize) -> Option<usize>) -> Option<usize> {
    if !starts_with_ignore_case(&bytes[i..], b".d.ts") {
        return None;
    }
    rest(bytes, i + 5)
}

/// `\(\d+,\d+\)` at `i`.
fn paren_location(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    let j = digits_end(bytes, i + 1)?;
    if bytes.get(j) != Some(&b',') {
        return None;
    }
    let k = digits_end(bytes, j + 1)?;
    if bytes.get(k) != Some(&b')') {
        return None;
    }
    Some(k + 1)
}

/// `:\d+:\d+` at `i`.
fn colon_location(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    let j = digits_end(bytes, i + 1)?;
    if bytes.get(j) != Some(&b':') {
        return None;
    }
    digits_end(bytes, j + 1)
}

/// A match of `(lib.*\.d\.ts)<rest>` (case-insensitive) that starts at
/// `start`: the end of group 1 and the match end. `.*` is greedy and stops
/// at `\n`, so the longest group that lets the rest match wins.
fn lib_dts_at(
    s: &str,
    start: usize,
    rest: fn(&[u8], usize) -> Option<usize>,
) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    if !starts_with_ignore_case(&bytes[start..], b"lib") {
        return None;
    }
    let line_end = memchr_newline(bytes, start).unwrap_or(bytes.len());
    let mut dot = line_end;
    loop {
        if dot >= start + 3 && s.is_char_boundary(dot) {
            if let Some(end) = dts_then(bytes, dot, rest) {
                return Some((dot + 5, end));
            }
        }
        if dot <= start + 3 {
            return None;
        }
        dot -= 1;
    }
}

/// Go `diagnosticsLocationPrefix.ReplaceAllString(s, "$1(--,--)")` with
/// `(?im)^(lib.*\.d\.ts)\(\d+,\d+\)`.
pub fn replace_diagnostics_location_prefix(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize;
    let mut pos = 0usize;
    loop {
        // `pos` is a line start.
        if let Some((group_end, end)) = lib_dts_at(s, pos, paren_location) {
            out.push_str(&s[copied..group_end]);
            out.push_str("(--,--)");
            copied = end;
            pos = end;
        }
        match memchr_newline(bytes, pos) {
            Some(newline) => pos = newline + 1,
            None => break,
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// Go `diagnosticsLocationPattern.ReplaceAllString(s, "$1:--:--")` with
/// `(?i)(lib.*\.d\.ts):\d+:\d+`.
pub fn replace_diagnostics_location_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize;
    let mut pos = 0usize;
    while pos < s.len() {
        if let Some((group_end, end)) = lib_dts_at(s, pos, colon_location) {
            out.push_str(&s[copied..group_end]);
            out.push_str(":--:--");
            copied = end;
            pos = end;
            continue;
        }
        pos += s[pos..].chars().next().map_or(1, char::len_utf8);
    }
    out.push_str(&s[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_lines() {
        assert_eq!(
            match_option_line("// @target: es5, es2015"),
            Some(("target", "es5, es2015"))
        );
        assert_eq!(
            match_option_line("//@Filename:a.ts"),
            Some(("Filename", "a.ts"))
        );
        assert_eq!(match_option_line("// normal comment"), None);
        assert_eq!(
            find_all_options("// @a: 1\n// @b:\n// x\ncode // @c: 2"),
            vec![("a", "1"), ("b", "// x")]
        );
    }

    #[test]
    fn link_lines() {
        assert_eq!(
            match_link_line("// @link: /a/b -> /c/d"),
            Some(("/a/b ", "/c/d"))
        );
        assert_eq!(match_link_line("// @link: /a/b"), None);
    }

    #[test]
    fn transpile_test_suffix() {
        for name in ["a.ts", "a.tsx", "a.js", "a.jsx", "a.cts", "a.mjs", "a.c.ts"] {
            assert!(has_transpile_test_suffix(name), "{name}");
        }
        for name in ["a.d", "a.xts", "a.mcts", "a.tsxx", "a.ts.map", "ats", "a.x"] {
            assert!(!has_transpile_test_suffix(name), "{name}");
        }
    }

    #[test]
    fn ansi_escapes() {
        assert_eq!(
            replace_ansi_escapes("\x1b[91merror\x1b[0m TS1: \x1b[1;30mx\x1b[m"),
            "error TS1: x"
        );
        assert_eq!(
            replace_ansi_escapes("\x1b[9x \x1b[ \x1b"),
            "\x1b[9x \x1b[ \x1b"
        );
    }

    #[test]
    fn location_patterns() {
        assert_eq!(
            replace_diagnostics_location_prefix("lib.es5.d.ts(12,3): error\nx lib.d.ts(1,1)"),
            "lib.es5.d.ts(--,--): error\nx lib.d.ts(1,1)"
        );
        assert_eq!(
            replace_diagnostics_location_pattern(" lib.es5.d.ts:12:3"),
            " lib.es5.d.ts:--:--"
        );
    }
}
