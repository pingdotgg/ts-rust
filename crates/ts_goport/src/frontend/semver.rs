//! Port of semver/version.go and semver/version_range.go.
//!
//! PORT: Go compiles its patterns with `regexp`. This crate has no regex
//! dependency, so each pattern has a hand-written matcher below
//! (`match_version_pattern`, `is_prerelease_part`, `is_build_part`,
//! `match_hyphen`, `match_range`). Each matcher gives the same match result
//! and the same submatches as Go RE2 leftmost-first matching on the anchored
//! pattern. `(?i)` in RE2 uses Unicode simple case folding, so `[a-z]`
//! also matches `ſ` (U+017F) and the Kelvin sign (U+212A). `\d` and `\s` are
//! ASCII only in RE2 (`\s` is `[\t\n\f\r ]`).

#[allow(unused_imports)]
use crate::frontend::prelude::*;

use std::cmp::Ordering;

// `[a-z]` under `(?i)` (RE2 simple case folding).
fn is_fold_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '\u{17F}' || c == '\u{212A}'
}

// `[a-z0-9-.]` under `(?i)`.
fn is_part_char(c: char) -> bool {
    is_fold_letter(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

// RE2 `\s`.
fn is_re_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

// Matches `0|[1-9]\d*` (or `[x*0]|[1-9]\d*` under `(?i)` when `wildcards`)
// at the start of `s`. Returns the length of the match.
fn match_number_component(s: &str, wildcards: bool) -> Option<usize> {
    let b = s.as_bytes();
    match b.first() {
        Some(b'0') => Some(1),
        Some(b'x' | b'X' | b'*') if wildcards => Some(1),
        Some(b'1'..=b'9') => Some(1 + b[1..].iter().take_while(|c| c.is_ascii_digit()).count()),
        _ => None,
    }
}

// Matches a run of `[a-z0-9-.]+` (under `(?i)`) at the start of `s`.
fn match_part_run(s: &str) -> Option<usize> {
    let n: usize = s
        .chars()
        .take_while(|&c| is_part_char(c))
        .map(char::len_utf8)
        .sum();
    if n == 0 { None } else { Some(n) }
}

// Go: semver/version.go:19 versionRegexp, semver/version_range.go:28 partialRegExp
// PORT: hand-written matcher for Go `versionRegexp` (`wildcards == false`)
// and `partialRegExp` (`wildcards == true`):
// `(?i)^(N)(?:\.(N)(?:\.(N)(?:-([a-z0-9-.]+))?(?:\+([a-z0-9-.]+))?)?)?$`.
// Returns the five submatches ("" when a group does not take part), like
// Go `FindStringSubmatch(text)[1:]`.
fn match_version_pattern(text: &str, wildcards: bool) -> Option<[&str; 5]> {
    let mut groups = [""; 5];
    let mut pos = match_number_component(text, wildcards)?;
    groups[0] = &text[..pos];
    if pos == text.len() {
        return Some(groups);
    }
    // The optional groups must match here, because `$` cannot.
    let rest = text[pos..].strip_prefix('.')?;
    pos += 1;
    let n = match_number_component(rest, wildcards)?;
    groups[1] = &text[pos..pos + n];
    pos += n;
    if pos == text.len() {
        return Some(groups);
    }
    let rest = text[pos..].strip_prefix('.')?;
    pos += 1;
    let n = match_number_component(rest, wildcards)?;
    groups[2] = &text[pos..pos + n];
    pos += n;
    if let Some(rest) = text[pos..].strip_prefix('-') {
        // The class has no '+', so the greedy run cannot give back a
        // character that a later part could use.
        if let Some(n) = match_part_run(rest) {
            groups[3] = &text[pos + 1..pos + 1 + n];
            pos += 1 + n;
        }
    }
    if let Some(rest) = text[pos..].strip_prefix('+') {
        if let Some(n) = match_part_run(rest) {
            groups[4] = &text[pos + 1..pos + 1 + n];
            pos += 1 + n;
        }
    }
    if pos == text.len() {
        Some(groups)
    } else {
        None
    }
}

// Go: semver/version.go:28 prereleasePartRegexp
// PORT: hand-written matcher for `(?i)^(?:0|[1-9]\d*|[a-z-][a-z0-9-]*)$`.
fn is_prerelease_part(s: &str) -> bool {
    if s == "0" {
        return true;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some('1'..='9') => chars.all(|c| c.is_ascii_digit()),
        Some(c) if is_fold_letter(c) || c == '-' => {
            chars.all(|c| is_fold_letter(c) || c.is_ascii_digit() || c == '-')
        }
        _ => false,
    }
}

// Go: semver/version.go:27 prereleaseRegexp
// PORT: `(?i)^P(?:\.P)*$` with P the part pattern. A dot-separated
// sequence of parts, each matching `is_prerelease_part`.
fn is_prerelease(s: &str) -> bool {
    s.split('.').all(is_prerelease_part)
}

// Go: semver/version.go:37 buildPartRegExp
// PORT: hand-written matcher for `(?i)^[a-z0-9-]+$`.
fn is_build_part(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| is_fold_letter(c) || c.is_ascii_digit() || c == '-')
}

// Go: semver/version.go:36 buildRegExp
// PORT: `(?i)^[a-z0-9-]+(?:\.[a-z0-9-]+)*$`.
fn is_build(s: &str) -> bool {
    s.split('.').all(is_build_part)
}

// Go: semver/version.go:42 numericIdentifierRegExp
// PORT: hand-written matcher for `^(?:0|[1-9]\d*)$`.
fn is_numeric_identifier(s: &str) -> bool {
    match_number_component(s, false) == Some(s.len())
}

// Go: semver/version.go:44 Version
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    prerelease: Vec<String>,
    build: Vec<String>,
}

// Go: semver/version.go:52 versionZero
fn version_zero() -> Version {
    Version {
        prerelease: vec!["0".to_string()],
        ..Version::default()
    }
}

const COMPARISON_LESS_THAN: i32 = -1;
const COMPARISON_EQUAL_TO: i32 = 0;
const COMPARISON_GREATER_THAN: i32 = 1;

fn cmp_to_i32(o: Ordering) -> i32 {
    match o {
        Ordering::Less => COMPARISON_LESS_THAN,
        Ordering::Equal => COMPARISON_EQUAL_TO,
        Ordering::Greater => COMPARISON_GREATER_THAN,
    }
}

impl Version {
    // Go: semver/version.go:56 incrementMajor
    // PORT: Go does not overflow-check `+ 1`. `wrapping_add` keeps the
    // Go uint32 wraparound.
    fn increment_major(&self) -> Version {
        Version {
            major: self.major.wrapping_add(1),
            ..Version::default()
        }
    }

    // Go: semver/version.go:62 incrementMinor
    fn increment_minor(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor.wrapping_add(1),
            ..Version::default()
        }
    }

    // Go: semver/version.go:69 incrementPatch
    fn increment_patch(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor,
            patch: self.patch.wrapping_add(1),
            ..Version::default()
        }
    }

    // Go: semver/version.go:83 Compare
    // PORT: Go takes pointers and orders nil first. Rust callers always
    // pass a version, so the nil cases are gone. The `a == b` pointer
    // shortcut gives the same result as the full compare.
    pub fn compare(&self, b: &Version) -> i32 {
        // https://semver.org/#spec-item-11
        // > Precedence is determined by the first difference when comparing each of these
        // > identifiers from left to right as follows: Major, minor, and patch versions are
        // > always compared numerically.
        //
        // https://semver.org/#spec-item-11
        // > Build metadata does not figure into precedence
        let r = cmp_to_i32(self.major.cmp(&b.major));
        if r != 0 {
            return r;
        }
        let r = cmp_to_i32(self.minor.cmp(&b.minor));
        if r != 0 {
            return r;
        }
        let r = cmp_to_i32(self.patch.cmp(&b.patch));
        if r != 0 {
            return r;
        }
        compare_pre_release_identifiers(&self.prerelease, &b.prerelease)
    }

    // Go: semver/version.go:190 String
    pub fn string(&self) -> String {
        let mut sb = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if !self.prerelease.is_empty() {
            sb.push('-');
            sb.push_str(&self.prerelease.join("."));
        }
        if !self.build.is_empty() {
            sb.push('+');
            sb.push_str(&self.build.join("."));
        }
        sb
    }
}

// Go: semver/version.go:123 comparePreReleaseIdentifiers
fn compare_pre_release_identifiers(left: &[String], right: &[String]) -> i32 {
    // https://semver.org/#spec-item-11
    // > When major, minor, and patch are equal, a pre-release version has lower precedence
    // > than a normal version.
    if left.is_empty() {
        if right.is_empty() {
            return COMPARISON_EQUAL_TO;
        }
        return COMPARISON_GREATER_THAN;
    } else if right.is_empty() {
        return COMPARISON_LESS_THAN;
    }

    // > Precedence for two pre-release versions with the same major, minor, and patch version
    // > MUST be determined by comparing each dot separated identifier from left to right until
    // > a difference is found [...]
    // PORT: Go `slices.CompareFunc`: first nonzero element result, else
    // compare lengths.
    for (l, r) in left.iter().zip(right.iter()) {
        let c = compare_pre_release_identifier(l, r);
        if c != 0 {
            return c;
        }
    }
    cmp_to_i32(left.len().cmp(&right.len()))
}

// Go: semver/version.go:143 comparePreReleaseIdentifier
fn compare_pre_release_identifier(left: &str, right: &str) -> i32 {
    let compare_result = cmp_to_i32(left.cmp(right));
    if compare_result == 0 {
        return compare_result;
    }

    let left_is_numeric = is_numeric_identifier(left);
    let right_is_numeric = is_numeric_identifier(right);

    if left_is_numeric || right_is_numeric {
        // https://semver.org/#spec-item-11
        // > Numeric identifiers always have lower precedence than non-numeric identifiers.
        if !right_is_numeric {
            return COMPARISON_LESS_THAN;
        }
        if !left_is_numeric {
            return COMPARISON_GREATER_THAN;
        }

        // https://semver.org/#spec-item-11
        // > identifiers consisting of only digits are compared numerically
        let left_as_number = get_uint_component(left);
        let right_as_number = get_uint_component(right);
        match (left_as_number, right_as_number) {
            (Ok(l), Ok(r)) => return cmp_to_i32(l.cmp(&r)),
            _ => {
                // This should only happen in the event of an overflow.
                // If so, use the lengths or fall back to string comparison.
                let len_compare = cmp_to_i32(left.len().cmp(&right.len()));
                if len_compare == 0 {
                    return compare_result;
                } else {
                    return len_compare;
                }
            }
        }
    }

    // https://semver.org/#spec-item-11
    // > identifiers with letters or hyphens are compared lexically in ASCII sort order.
    compare_result
}

// Go: semver/version.go:202 SemverParseError
#[derive(Clone, Debug)]
pub struct SemverParseError {
    orig_input: String,
}

impl SemverParseError {
    // Go: semver/version.go:206 Error
    // PORT: Go `%q` quoting. For the inputs this sees (package.json keys
    // and the compiler version) `{:?}` gives the same text, except for
    // non-printable characters.
    pub fn error(&self) -> String {
        format!("Could not parse version string from {:?}", self.orig_input)
    }
}

// Go: semver/version.go:210 TryParseVersion
// PORT: Go returns `(Version, error)` with a partly filled version on
// error. No caller reads that version, so an error is `Err(message)`.
pub fn try_parse_version(text: &str) -> Result<Version, String> {
    let mut result = Version::default();

    let Some(m) = match_version_pattern(text, false) else {
        return Err(SemverParseError {
            orig_input: text.to_string(),
        }
        .error());
    };

    let major_str = m[0];
    let minor_str = m[1];
    let patch_str = m[2];
    let prerelease_str = m[3];
    let build_str = m[4];

    result.major = get_uint_component(major_str)?;

    if !minor_str.is_empty() {
        result.minor = get_uint_component(minor_str)?;
    }

    if !patch_str.is_empty() {
        result.patch = get_uint_component(patch_str)?;
    }

    if !prerelease_str.is_empty() {
        if !is_prerelease(prerelease_str) {
            return Err(SemverParseError {
                orig_input: text.to_string(),
            }
            .error());
        }
        result.prerelease = prerelease_str.split('.').map(str::to_string).collect();
    }
    if !build_str.is_empty() {
        if !is_build(build_str) {
            return Err(SemverParseError {
                orig_input: text.to_string(),
            }
            .error());
        }
        result.build = build_str.split('.').map(str::to_string).collect();
    }

    Ok(result)
}

// Go: semver/version.go:263 MustParse
// PORT: named `must_parse_version` so the glob export does not claim the
// generic name `must_parse`.
pub fn must_parse_version(text: &str) -> Version {
    match try_parse_version(text) {
        Ok(v) => v,
        Err(err) => panic!("{err}"),
    }
}

// Go: semver/version.go:271 getUintComponent
// PORT: Go `strconv.ParseUint(text, 10, 32)`. Only ASCII digits are
// accepted; an empty string, a sign or a value above u32::MAX is an error.
fn get_uint_component(text: &str) -> Result<u32, String> {
    if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "strconv.ParseUint: parsing {text:?}: invalid syntax"
        ));
    }
    text.parse::<u32>()
        .map_err(|_| format!("strconv.ParseUint: parsing {text:?}: value out of range"))
}

// Go: semver/version_range.go:43 VersionRange
#[derive(Clone, Debug, Default)]
pub struct VersionRange {
    alternatives: Vec<Vec<VersionComparator>>,
}

// Go: semver/version_range.go:47 versionComparator
#[derive(Clone, Debug)]
struct VersionComparator {
    operator: ComparatorOperator,
    operand: Version,
}

// Go: semver/version_range.go:52 comparatorOperator
// PORT: Go uses a string type and also stores the raw `~`, `^` and ``
// operators in it while parsing. Those never reach a stored comparator,
// so the stored operator is an enum and `parse_comparator` switches on
// the raw `&str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComparatorOperator {
    RangeLessThan,
    RangeLessThanEqual,
    RangeEqual,
    RangeGreaterThanEqual,
    RangeGreaterThan,
}

impl ComparatorOperator {
    fn as_str(self) -> &'static str {
        match self {
            ComparatorOperator::RangeLessThan => "<",
            ComparatorOperator::RangeLessThanEqual => "<=",
            ComparatorOperator::RangeEqual => "=",
            ComparatorOperator::RangeGreaterThanEqual => ">=",
            ComparatorOperator::RangeGreaterThan => ">",
        }
    }
}

impl VersionRange {
    // Go: semver/version_range.go:62 String
    pub fn string(&self) -> String {
        let mut sb = String::new();
        format_disjunction(&mut sb, &self.alternatives);
        sb
    }

    // Go: semver/version_range.go:97 Test
    pub fn test(&self, version: &Version) -> bool {
        test_disjunction(&self.alternatives, version)
    }
}

// Go: semver/version_range.go:68 formatDisjunction
fn format_disjunction(sb: &mut String, alternatives: &[Vec<VersionComparator>]) {
    let orig_len = sb.len();

    for (i, alternative) in alternatives.iter().enumerate() {
        if i > 0 {
            sb.push_str(" || ");
        }
        format_alternative(sb, alternative);
    }

    if sb.len() == orig_len {
        sb.push('*');
    }
}

// Go: semver/version_range.go:83 formatAlternative
fn format_alternative(sb: &mut String, comparators: &[VersionComparator]) {
    for (i, comparator) in comparators.iter().enumerate() {
        if i > 0 {
            sb.push(' ');
        }
        format_comparator(sb, comparator);
    }
}

// Go: semver/version_range.go:92 formatComparator
fn format_comparator(sb: &mut String, comparator: &VersionComparator) {
    sb.push_str(comparator.operator.as_str());
    sb.push_str(&comparator.operand.string());
}

// Go: semver/version_range.go:101 testDisjunction
fn test_disjunction(alternatives: &[Vec<VersionComparator>], version: &Version) -> bool {
    // an empty disjunction is treated as "*" (all versions)
    if alternatives.is_empty() {
        return true;
    }

    for alternative in alternatives {
        if test_alternative(alternative, version) {
            return true;
        }
    }

    false
}

// Go: semver/version_range.go:116 testAlternative
fn test_alternative(alternative: &[VersionComparator], version: &Version) -> bool {
    for comparator in alternative {
        if !test_comparator(comparator, version) {
            return false;
        }
    }
    true
}

// Go: semver/version_range.go:125 testComparator
fn test_comparator(comparator: &VersionComparator, version: &Version) -> bool {
    let cmp = version.compare(&comparator.operand);
    match comparator.operator {
        ComparatorOperator::RangeLessThan => cmp < 0,
        ComparatorOperator::RangeLessThanEqual => cmp <= 0,
        ComparatorOperator::RangeEqual => cmp == 0,
        ComparatorOperator::RangeGreaterThanEqual => cmp >= 0,
        ComparatorOperator::RangeGreaterThan => cmp > 0,
    }
}

// Go: semver/version_range.go:143 TryParseVersionRange
pub fn try_parse_version_range(text: &str) -> (VersionRange, bool) {
    let (alternatives, ok) = parse_alternatives(text);
    (VersionRange { alternatives }, ok)
}

// PORT: Go `whitespaceRegExp.Split(r, -1)` for `\s+`. `r` is already
// trimmed, so this is a split on each maximal run of RE2 `\s`.
fn split_whitespace_re(r: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut beg = 0;
    let mut chars = r.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if is_re_space(c) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if !is_re_space(d) {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
            out.push(&r[beg..i]);
            beg = end;
        }
    }
    out.push(&r[beg..]);
    out
}

// Go: semver/version_range.go:148 parseAlternatives
fn parse_alternatives(text: &str) -> (Vec<Vec<VersionComparator>>, bool) {
    let mut alternatives = Vec::new();

    let text = text.trim();
    // PORT: Go `logicalOrRegExp.Split(text, -1)` on `\|\|`. A literal
    // split gives the same pieces (non-overlapping, leftmost matches).
    let ranges = text.split("||");
    for r in ranges {
        let r = r.trim();
        if r.is_empty() {
            continue;
        }

        let mut comparators = Vec::new();

        if let Some((left, right)) = match_hyphen(r) {
            if let Some(parsed_comparators) = parse_hyphen(left, right) {
                comparators.extend(parsed_comparators);
            } else {
                return (Vec::new(), false);
            }
        } else {
            for simple in split_whitespace_re(r) {
                let Some((op, operand)) = match_range(simple.trim()) else {
                    return (Vec::new(), false);
                };

                if let Some(parsed_comparators) = parse_comparator(op, operand) {
                    comparators.extend(parsed_comparators);
                } else {
                    return (Vec::new(), false);
                }
            }
        }

        alternatives.push(comparators);
    }

    (alternatives, true)
}

// `[a-z0-9-+.*]` under `(?i)`.
fn is_range_operand_char(c: char) -> bool {
    is_fold_letter(c) || c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | '*')
}

fn operand_run_len(s: &str) -> usize {
    s.chars()
        .take_while(|&c| is_range_operand_char(c))
        .map(char::len_utf8)
        .sum()
}

fn space_run_len(s: &str) -> usize {
    s.chars()
        .take_while(|&c| is_re_space(c))
        .map(char::len_utf8)
        .sum()
}

// Go: semver/version_range.go:33 hyphenRegExp
// PORT: hand-written matcher for
// `(?i)^\s*([a-z0-9-+.*]+)\s+-\s+([a-z0-9-+.*]+)\s*$`. The operand class
// has no space and each space run must be followed by a non-space, so
// the maximal runs are the only possible match.
fn match_hyphen(s: &str) -> Option<(&str, &str)> {
    let mut pos = space_run_len(s);
    let n = operand_run_len(&s[pos..]);
    if n == 0 {
        return None;
    }
    let left = &s[pos..pos + n];
    pos += n;
    let n = space_run_len(&s[pos..]);
    if n == 0 {
        return None;
    }
    pos += n;
    pos += s[pos..].strip_prefix('-').map(|_| 1)?;
    let n = space_run_len(&s[pos..]);
    if n == 0 {
        return None;
    }
    pos += n;
    let n = operand_run_len(&s[pos..]);
    if n == 0 {
        return None;
    }
    let right = &s[pos..pos + n];
    pos += n;
    pos += space_run_len(&s[pos..]);
    if pos == s.len() {
        Some((left, right))
    } else {
        None
    }
}

// Go: semver/version_range.go:41 rangeRegExp
// PORT: hand-written matcher for
// `(?i)^([~^<>=]|<=|>=)?\s*([a-z0-9-+.*]+)$`. With leftmost-first
// alternation, a one-character `<` or `>` followed by `=` cannot match
// (the operand class has no `=`), so `<=` and `>=` win there.
fn match_range(s: &str) -> Option<(&str, &str)> {
    let op = if s.starts_with("<=") || s.starts_with(">=") {
        &s[..2]
    } else if s.starts_with(['~', '^', '<', '>', '=']) {
        &s[..1]
    } else {
        ""
    };
    let mut pos = op.len();
    pos += space_run_len(&s[pos..]);
    let n = operand_run_len(&s[pos..]);
    if n == 0 || pos + n != s.len() {
        return None;
    }
    Some((op, &s[pos..]))
}

// Go: semver/version_range.go:188 parseHyphen
fn parse_hyphen(left: &str, right: &str) -> Option<Vec<VersionComparator>> {
    let left_result = parse_partial(left)?;
    let right_result = parse_partial(right)?;

    let mut comparators = Vec::new();
    if !is_wildcard(&left_result.major_str) {
        // `MAJOR.*.*-...` gives us `>=MAJOR.0.0 ...`
        comparators.push(VersionComparator {
            operator: ComparatorOperator::RangeGreaterThanEqual,
            operand: left_result.version,
        });
    }

    if !is_wildcard(&right_result.major_str) {
        let operator;
        let mut operand = right_result.version.clone();

        if is_wildcard(&right_result.minor_str) {
            // `...-MAJOR.*.*` gives us `... <(MAJOR+1).0.0`
            operand = operand.increment_major();
            operator = ComparatorOperator::RangeLessThan;
        } else if is_wildcard(&right_result.patch_str) {
            // `...-MAJOR.MINOR.*` gives us `... <MAJOR.(MINOR+1).0`
            operand = operand.increment_minor();
            operator = ComparatorOperator::RangeLessThan;
        } else {
            // `...-MAJOR.MINOR.PATCH` gives us `... <=MAJOR.MINOR.PATCH`
            operator = ComparatorOperator::RangeLessThanEqual;
        }

        comparators.push(VersionComparator { operator, operand });
    }

    Some(comparators)
}

// Go: semver/version_range.go:235 partialVersion
struct PartialVersion {
    version: Version,
    major_str: String,
    minor_str: String,
    patch_str: String,
}

// Go: semver/version_range.go:243 parsePartial
// Produces a "partial" version
fn parse_partial(text: &str) -> Option<PartialVersion> {
    let m = match_version_pattern(text, true)?;

    let major_str = m[0];
    let mut minor_str = m[1];
    let mut patch_str = m[2];
    let prerelease_str = m[3];
    let build_str = m[4];

    if minor_str.is_empty() {
        minor_str = "*";
    }
    if patch_str.is_empty() {
        patch_str = "*";
    }

    let major_numeric;
    let mut minor_numeric = 0;
    let mut patch_numeric = 0;

    if is_wildcard(major_str) {
        major_numeric = 0;
    } else {
        major_numeric = get_uint_component(major_str).ok()?;

        if !is_wildcard(minor_str) {
            minor_numeric = get_uint_component(minor_str).ok()?;

            if !is_wildcard(patch_str) {
                patch_numeric = get_uint_component(patch_str).ok()?;
            }
        }
    }

    let mut prerelease = Vec::new();
    if !prerelease_str.is_empty() {
        prerelease = prerelease_str.split('.').map(str::to_string).collect();
    }

    let mut build = Vec::new();
    if !build_str.is_empty() {
        build = build_str.split('.').map(str::to_string).collect();
    }

    Some(PartialVersion {
        version: Version {
            major: major_numeric,
            minor: minor_numeric,
            patch: patch_numeric,
            prerelease,
            build,
        },
        major_str: major_str.to_string(),
        minor_str: minor_str.to_string(),
        patch_str: patch_str.to_string(),
    })
}

// Go: semver/version_range.go:321 parseComparator
fn parse_comparator(op: &str, text: &str) -> Option<Vec<VersionComparator>> {
    use ComparatorOperator::*;

    let result = parse_partial(text)?;

    let mut comparators_result = Vec::new();

    if !is_wildcard(&result.major_str) {
        match op {
            "~" => {
                let first = VersionComparator {
                    operator: RangeGreaterThanEqual,
                    operand: result.version.clone(),
                };

                let second_version = if is_wildcard(&result.minor_str) {
                    result.version.increment_major()
                } else {
                    result.version.increment_minor()
                };

                let second = VersionComparator {
                    operator: RangeLessThan,
                    operand: second_version,
                };
                comparators_result = vec![first, second];
            }
            "^" => {
                let first = VersionComparator {
                    operator: RangeGreaterThanEqual,
                    operand: result.version.clone(),
                };

                let second_version = if result.version.major > 0 || is_wildcard(&result.minor_str) {
                    result.version.increment_major()
                } else if result.version.minor > 0 || is_wildcard(&result.patch_str) {
                    result.version.increment_minor()
                } else {
                    result.version.increment_patch()
                };
                let second = VersionComparator {
                    operator: RangeLessThan,
                    operand: second_version,
                };
                comparators_result = vec![first, second];
            }
            "<" | ">=" => {
                let operator = if op == "<" {
                    RangeLessThan
                } else {
                    RangeGreaterThanEqual
                };
                let mut version = result.version.clone();
                if is_wildcard(&result.minor_str) || is_wildcard(&result.patch_str) {
                    version.prerelease = vec!["0".to_string()];
                }
                comparators_result = vec![VersionComparator {
                    operator,
                    operand: version,
                }];
            }
            "<=" | ">" => {
                let mut operator = if op == "<=" {
                    RangeLessThanEqual
                } else {
                    RangeGreaterThan
                };
                let mut version = result.version.clone();
                if is_wildcard(&result.minor_str) {
                    if operator == RangeLessThanEqual {
                        operator = RangeLessThan;
                    } else {
                        operator = RangeGreaterThanEqual;
                    }

                    version = version.increment_major();
                    version.prerelease = vec!["0".to_string()];
                } else if is_wildcard(&result.patch_str) {
                    if operator == RangeLessThanEqual {
                        operator = RangeLessThan;
                    } else {
                        operator = RangeGreaterThanEqual;
                    }

                    version = version.increment_minor();
                    version.prerelease = vec!["0".to_string()];
                }

                comparators_result = vec![VersionComparator {
                    operator,
                    operand: version,
                }];
            }
            "=" | "" => {
                // normalize empty string to `=`
                let operator = RangeEqual;

                if is_wildcard(&result.minor_str) || is_wildcard(&result.patch_str) {
                    let original_version = result.version.clone();

                    let mut first_version = original_version.clone();
                    first_version.prerelease = vec!["0".to_string()];

                    let mut second_version = if is_wildcard(&result.minor_str) {
                        original_version.increment_major()
                    } else {
                        original_version.increment_minor()
                    };
                    second_version.prerelease = vec!["0".to_string()];

                    comparators_result = vec![
                        VersionComparator {
                            operator: RangeGreaterThanEqual,
                            operand: first_version,
                        },
                        VersionComparator {
                            operator: RangeLessThan,
                            operand: second_version,
                        },
                    ];
                } else {
                    comparators_result = vec![VersionComparator {
                        operator,
                        operand: result.version,
                    }];
                }
            }
            _ => panic!("Unexpected operator: {op}"),
        }
    } else if op == "<" || op == ">" {
        comparators_result = vec![
            // < 0.0.0-0
            VersionComparator {
                operator: RangeLessThan,
                operand: version_zero(),
            },
        ];
    }

    Some(comparators_result)
}

// Go: semver/version_range.go:436 isWildcard
fn is_wildcard(text: &str) -> bool {
    text == "*" || text == "x" || text == "X"
}
