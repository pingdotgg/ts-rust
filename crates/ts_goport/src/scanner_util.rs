//! Port of the scanner, stringutil and core text helpers that the pinned
//! typescript-go binder and checker call:
//! `scanner/scanner.go`, `scanner/utilities.go`, `stringutil/util.go`,
//! `stringutil/identifier.go`, `stringutil/js_case.go` (plus the generated
//! Unicode tables at the end of this file) and a few `core/core.go` helpers.
//!
//! Full tokenization (Go `Scanner.Scan`) uses the Go scanner port
//! (`frontend::scanner::Scanner`), through
//! `frontend::scanner::scanner_ls::get_scanner_for_source_file`.
//!
//! Go `rune` ports to `char` where the value is a real code point. Go strings
//! that hold WTF-8 lone-surrogate sentinels cannot exist in a Rust `&str`, so
//! the surrogate helpers take `u32` code points.

use std::borrow::Cow;

use crate::frontend::scanner::scanner_ls::get_scanner_for_source_file;
use crate::prelude::*;

// ---------------------------------------------------------------------------
// Go `unicode/utf8` helpers (private).
// ---------------------------------------------------------------------------

/// Go `utf8.DecodeRuneInString(text[pos:])`. Returns `(RuneError, 0)` at the
/// end of the text and `(RuneError, 1)` when `pos` is not a char boundary.
fn decode_rune_at(text: &str, pos: usize) -> (char, usize) {
    if pos >= text.len() {
        return (char::REPLACEMENT_CHARACTER, 0);
    }
    match text.get(pos..).and_then(|rest| rest.chars().next()) {
        Some(ch) => (ch, ch.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// Go `utf8.DecodeLastRuneInString(text[:end])`.
fn decode_last_rune_before(text: &str, end: usize) -> (char, usize) {
    if end == 0 {
        return (char::REPLACEMENT_CHARACTER, 0);
    }
    match text
        .get(..end)
        .and_then(|prefix| prefix.chars().next_back())
    {
        Some(ch) => (ch, ch.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

/// Go `unicode.Is(table, ch)` for the generated range tables in this file.
/// Each entry is `(lo, hi, stride)`, sorted and non-overlapping.
fn unicode_is(table: &[(u32, u32, u32)], ch: u32) -> bool {
    let mut low = 0usize;
    let mut high = table.len();
    while low < high {
        let middle = low + (high - low) / 2;
        let (lo, hi, stride) = table[middle];
        if lo <= ch && ch <= hi {
            return stride == 1 || (ch - lo) % stride == 0;
        }
        if ch < lo {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    false
}

/// Go `unicode.ToLower(ch)` (simple case mapping).
// PORT: Rust only exposes full case mapping; when it yields several chars
// (only U+0130) Go's simple mapping equals the first one.
// PERF: an ASCII char maps with a table-free test; `to_lowercase` looks up
// the Unicode tables.
fn unicode_to_lower(ch: char) -> char {
    if ch.is_ascii() {
        return ch.to_ascii_lowercase();
    }
    ch.to_lowercase().next().unwrap_or(ch)
}

/// Go `unicode.ToUpper(ch)` (simple case mapping).
fn unicode_to_upper(ch: char) -> char {
    let mut upper = ch.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => ch,
    }
}

/// Go `strings.EqualFold(a, b)` on the runes of two strings. Go reads each
/// invalid byte as U+FFFD (see `go_runes`).
// PORT: approximates Go simple case folding with single-char lower/upper maps.
fn equal_fold(a: &[char], b: &[char]) -> bool {
    let mut left = a.iter().copied();
    let mut right = b.iter().copied();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if x == y
                    || unicode_to_lower(x) == unicode_to_lower(y)
                    || unicode_to_upper(x) == unicode_to_upper(y)
                {
                    continue;
                }
                return false;
            }
            _ => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// stringutil/util.go
// ---------------------------------------------------------------------------

// Go: stringutil/util.go:12 IsWhiteSpaceLike
pub fn is_white_space_like(ch: char) -> bool {
    is_white_space_single_line(ch) || is_line_break(ch)
}

// Go: stringutil/util.go:16 IsWhiteSpaceSingleLine
pub fn is_white_space_single_line(ch: char) -> bool {
    // Note: nextLine is in the Zs space, and should be considered to be a whitespace.
    // It is explicitly not a line-break as it isn't in the exact set specified by EcmaScript.
    matches!(
        ch,
        ' '          // space
        | '\t'       // tab
        | '\u{000B}' // verticalTab
        | '\u{000C}' // formFeed
        | '\u{0085}' // nextLine
        | '\u{00A0}' // nonBreakingSpace
        | '\u{1680}' // ogham
        | '\u{2000}' // enQuad
        | '\u{2001}' // emQuad
        | '\u{2002}' // enSpace
        | '\u{2003}' // emSpace
        | '\u{2004}' // threePerEmSpace
        | '\u{2005}' // fourPerEmSpace
        | '\u{2006}' // sixPerEmSpace
        | '\u{2007}' // figureSpace
        | '\u{2008}' // punctuationEmSpace
        | '\u{2009}' // thinSpace
        | '\u{200A}' // hairSpace
        | '\u{200B}' // zeroWidthSpace
        | '\u{202F}' // narrowNoBreakSpace
        | '\u{205F}' // mathematicalSpace
        | '\u{3000}' // ideographicSpace
        | '\u{FEFF}' // byteOrderMark
    )
}

// Go: stringutil/util.go:49 IsLineBreak
pub fn is_line_break(ch: char) -> bool {
    // ES5 7.3:
    // The ECMAScript line terminator characters are listed in Table 3.
    //     Table 3: Line Terminator Characters
    //     Code Unit Value     Name                    Formal Name
    //     \u000A              Line Feed               <LF>
    //     \u000D              Carriage Return         <CR>
    //                    Line separator          <LS>
    //                    Paragraph separator     <PS>
    // Only the characters in Table 3 are treated as line terminators. Other new line or line
    // breaking characters are treated as white space but not as line terminators.
    matches!(
        ch,
        '\n'         // lineFeed
        | '\r'       // carriageReturn
        | '\u{2028}' // lineSeparator
        | '\u{2029}' // paragraphSeparator
    )
}

// Go: stringutil/util.go:71 IsDigit
pub fn is_digit(ch: char) -> bool {
    ('0'..='9').contains(&ch)
}

// Go: stringutil/util.go:75 IsOctalDigit
pub fn is_octal_digit(ch: char) -> bool {
    ('0'..='7').contains(&ch)
}

// Go: stringutil/util.go:79 IsHexDigit
pub fn is_hex_digit(ch: char) -> bool {
    ('0'..='9').contains(&ch) || ('A'..='F').contains(&ch) || ('a'..='f').contains(&ch)
}

// Go: stringutil/util.go:83 IsASCIILetter
pub fn is_ascii_letter(ch: char) -> bool {
    ('A'..='Z').contains(&ch) || ('a'..='z').contains(&ch)
}

// Go: stringutil/util.go:222 StripQuotes
pub fn strip_quotes(name: &str) -> String {
    if name.len() < 2 {
        return name.to_string();
    }
    let (first_char, _) = decode_rune_at(name, 0);
    let (last_char, _) = decode_last_rune_before(name, name.len());
    if first_char == last_char && (first_char == '\'' || first_char == '"' || first_char == '`') {
        return name[1..name.len() - 1].to_string();
    }
    name.to_string()
}

// Go: stringutil/util.go:240 UnquoteString
pub fn unquote_string(str: &str) -> String {
    // strconv.Unquote is insufficient as that only handles a single character inside single quotes, as those are character literals in go
    let inner = strip_quotes(str);
    // In strada we do str.replace(/\\./g, s => s.substring(1)) - which is to say, replace all backslash-something with just something
    // That's replicated here faithfully, but it seems wrong! This should probably be an actual unquote operation?
    // PORT: Go uses regexp `\\.` (`.` does not match '\n'); this loop is the
    // same leftmost, non-overlapping replacement.
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                if next != '\n' {
                    result.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        result.push(ch);
    }
    result
}

// Go string helpers: `gostring.rs` in goport_util.
pub use goport_util::scanner_util::*;

// ---------------------------------------------------------------------------
// stringutil/identifier.go
// ---------------------------------------------------------------------------

// Go: stringutil/identifier.go:8 IsUnicodeIdentifierStart
// IsUnicodeIdentifierStart reports whether ch may begin an ECMAScript
// identifier, i.e. whether it has the Unicode ID_Start (or Other_ID_Start)
// property.
pub fn is_unicode_identifier_start(ch: char) -> bool {
    unicode_is(UNICODE_ES_NEXT_IDENTIFIER_START, ch as u32)
}

// Go: stringutil/identifier.go:15 IsUnicodeIdentifierPart
// IsUnicodeIdentifierPart reports whether ch may appear after the first
// character of an ECMAScript identifier, i.e. whether it has the Unicode
// ID_Continue (or Other_ID_Continue) property, which also includes ID_Start.
pub fn is_unicode_identifier_part(ch: char) -> bool {
    unicode_is(UNICODE_ES_NEXT_IDENTIFIER_PART, ch as u32)
}

// ---------------------------------------------------------------------------
// stringutil/js_case.go
// ---------------------------------------------------------------------------

// Go stringutil/js_case_generated.go specialCasingMappings, in less space.
// crates/ts_goport/scripts/gen-js-case-tables.py writes it.
mod js_case_tables;

use js_case_tables::{CASE_RANGES, FINAL_SIGMA, SPECIAL_CASE_MAPPINGS, UPPER_LOWER};

/// The lower or the upper case of a code point in Go `specialCasingMappings`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaseMapping {
    Char(char),
    Str(&'static str),
}

impl CaseMapping {
    fn push_to(self, builder: &mut String) {
        match self {
            CaseMapping::Char(ch) => builder.push(ch),
            CaseMapping::Str(text) => builder.push_str(text),
        }
    }
}

/// Go `specialCasingMappings[r]`: `(lower, upper)`.
// PORT: Go keeps one map of strings. The port keeps the one to one mappings
// as runs (`CASE_RANGES`), the others as strings, and the one conditional
// mapping (`conditionalLower`, Final_Sigma) as `FINAL_SIGMA`.
fn special_casing_mapping(r: char) -> Option<(CaseMapping, CaseMapping)> {
    let code = r as u32;
    let index = CASE_RANGES.partition_point(|&(_, hi, _, _)| hi < code);
    if let Some(&(lo, _, lower, upper)) = CASE_RANGES.get(index)
        && lo <= code
    {
        let (lower, upper) = if lower == UPPER_LOWER {
            (lo + ((code - lo) | 1), lo + ((code - lo) & !1))
        } else {
            (
                code.wrapping_add_signed(lower),
                code.wrapping_add_signed(upper),
            )
        };
        // case_tables_hold_go_special_casing_mappings checks that every run
        // maps to valid chars.
        let mapping =
            |code| CaseMapping::Char(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
        return Some((mapping(lower), mapping(upper)));
    }
    let index = SPECIAL_CASE_MAPPINGS
        .binary_search_by_key(&code, |entry| entry.0)
        .ok()?;
    let (_, lower, upper) = SPECIAL_CASE_MAPPINGS[index];
    Some((CaseMapping::Str(lower), CaseMapping::Str(upper)))
}

// Go: stringutil/js_case.go:9 ToLowerJS
pub fn to_lower_js(str: &str) -> String {
    if let Some(ascii) = to_lower_ascii(str) {
        return ascii;
    }

    let mut builder = String::with_capacity(str.len());
    // casedBefore tracks whether the most recent non-Case_Ignorable code point is
    // "cased", which is the backward half of the Final_Sigma context. We
    // accumulate it as we stream so we never have to scan (or decode) backwards.
    let mut cased_before = false;
    let mut i = 0usize;
    while i < str.len() {
        let (code, size) = decode_js_string_rune(&str[i..]);
        i += size as usize;
        // PORT: a non-surrogate rune from `decode_js_string_rune` is always a
        // valid `char`; the Go body works on `rune` directly.
        let r = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
        if is_surrogate(code) {
            // A lone surrogate has no case mapping; preserve it verbatim, matching
            // String.prototype.toLowerCase.
            builder.push_str(&encode_js_string_rune(code));
        } else if let Some((lower, _upper)) = special_casing_mapping(r) {
            if r == FINAL_SIGMA.0 && is_final_sigma_context(cased_before, str, i) {
                builder.push(FINAL_SIGMA.1);
            } else {
                lower.push_to(&mut builder);
            }
        } else {
            // PORT: keeps the U+FDD0 escape (see GO_STRING_MARKER).
            push_js_string_rune(&mut builder, code);
        }
        if !is_unicode_case_ignorable(r) {
            cased_before = is_sigma_cased(r);
        }
    }
    builder
}

// Go: stringutil/js_case.go:44 ToUpperJS
pub fn to_upper_js(str: &str) -> String {
    if let Some(ascii) = to_upper_ascii(str) {
        return ascii;
    }

    let mut builder = String::with_capacity(str.len());
    let mut i = 0usize;
    while i < str.len() {
        let (code, size) = decode_js_string_rune(&str[i..]);
        let size = size as usize;
        // PORT: a non-surrogate rune is always a valid `char` (see to_lower_js).
        let r = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
        if is_surrogate(code) {
            // A lone surrogate has no case mapping; preserve it verbatim, matching
            // String.prototype.toUpperCase.
            builder.push_str(&str[i..i + size]);
        } else if let Some((_lower, upper)) = special_casing_mapping(r) {
            upper.push_to(&mut builder);
        } else {
            // PORT: keeps the U+FDD0 escape (see GO_STRING_MARKER).
            push_js_string_rune(&mut builder, code);
        }
        i += size;
    }

    builder
}

// Go: stringutil/js_case.go:69 toLowerASCII
fn to_lower_ascii(str: &str) -> Option<String> {
    let mut needs_mapping = false;
    for &ch in str.as_bytes() {
        if ch >= 0x80 {
            return None;
        }
        needs_mapping = needs_mapping || (b'A' <= ch && ch <= b'Z');
    }
    if !needs_mapping {
        return Some(str.to_string());
    }
    Some(str.to_ascii_lowercase())
}

// Go: stringutil/js_case.go:91 toUpperASCII
fn to_upper_ascii(str: &str) -> Option<String> {
    let mut needs_mapping = false;
    for &ch in str.as_bytes() {
        if ch >= 0x80 {
            return None;
        }
        needs_mapping = needs_mapping || (b'a' <= ch && ch <= b'z');
    }
    if !needs_mapping {
        return Some(str.to_string());
    }
    Some(str.to_ascii_uppercase())
}

// Go: stringutil/js_case.go:135 isFinalSigmaContext
// isFinalSigmaContext reports whether a sigma at the current position is in
// Final_Sigma context: it is preceded by a cased code point and not followed by
// one. casedBefore carries the backward half; afterOffset is the byte offset
// just past the sigma, from which we scan forward.
fn is_final_sigma_context(cased_before: bool, str: &str, after_offset: usize) -> bool {
    cased_before && !has_sigma_cased_after(str, after_offset)
}

// Go: stringutil/js_case.go:139 hasSigmaCasedAfter
fn has_sigma_cased_after(str: &str, start: usize) -> bool {
    let mut i = start;
    while i < str.len() {
        let (code, size) = decode_js_string_rune(&str[i..]);
        i += size as usize;
        // PORT: a lone surrogate becomes U+FFFD, which is neither case
        // ignorable nor cased, the same as the surrogate rune in Go.
        let r = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
        if is_unicode_case_ignorable(r) {
            continue;
        }
        return is_sigma_cased(r);
    }
    false
}

// Go: stringutil/js_case.go:151 isSigmaCased
fn is_sigma_cased(r: char) -> bool {
    unicode_is(UNICODE_CASED_RANGES, r as u32)
}

// Go: stringutil/js_case.go:155 isUnicodeCaseIgnorable
fn is_unicode_case_ignorable(r: char) -> bool {
    unicode_is(UNICODE_CASE_IGNORABLE_RANGES, r as u32)
}

// ---------------------------------------------------------------------------
// core/core.go
// ---------------------------------------------------------------------------

// Go: core/core.go:428 ComputeECMALineStarts
// PORT: Go `ECMALineStarts` ([]TextPos) -> `Vec<i32>`; `ComputeECMALineStartsSeq`
// is inlined.
pub fn compute_ecma_line_starts(text: &str) -> Vec<i32> {
    let bytes = text.as_bytes();
    let mut result = Vec::with_capacity(memchr::memchr_iter(b'\n', bytes).count() + 1);
    let mut line_start = 0usize;
    // The `\n` of a `\r\n` pair, which the `\r` already counted.
    let mut skip_lf_at = usize::MAX;
    // Line breaks are `\n`, `\r`, U+2028 (E2 80 A8) and U+2029 (E2 80 A9).
    // 0xE2 is never a UTF-8 continuation byte, so each hit starts a char.
    for i in memchr::memchr3_iter(b'\n', b'\r', 0xE2, bytes) {
        let next = match bytes[i] {
            b'\n' if i == skip_lf_at => continue,
            b'\n' => i + 1,
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                skip_lf_at = i + 1;
                i + 2
            }
            b'\r' => i + 1,
            _ if bytes.get(i + 1) == Some(&0x80)
                && matches!(bytes.get(i + 2), Some(0xA8 | 0xA9)) =>
            {
                i + 3
            }
            _ => continue,
        };
        result.push(line_start as i32);
        line_start = next;
    }
    result.push(line_start as i32);
    result
}

// Go: core/core.go:484 UTF16Len
// UTF16Len returns the number of UTF-16 code units needed to
// represent the given UTF-8 encoded string.
// PORT: `s` is the port form of a Go string. Each unit counts as its Go bytes
// do (see `GoUnit::go_utf16_len`).
pub fn utf16_len(s: &str) -> i32 {
    // Fast path: scan for non-ASCII bytes. For ASCII-only strings,
    // each byte is one UTF-16 code unit, so we can return len(s) directly.
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b >= 0x80 {
            // Found non-ASCII; count the ASCII prefix, then decode the rest.
            let mut n = i as i32;
            let rest = &s[i..];
            let mut chars = rest.char_indices();
            while let Some((j, r)) = chars.next() {
                if r != GO_STRING_MARKER {
                    n += r.len_utf16() as i32;
                    continue;
                }
                let (unit, size) = go_unit_at(rest, j);
                n += unit.go_utf16_len() as i32;
                if size > r.len_utf8() {
                    // Skip the second char of the unit.
                    chars.next();
                }
            }
            return n;
        }
    }
    s.len() as i32
}

/// Go `core.UTF16Len(s[start:end])`. Go slices bytes, so the range can cut a
/// char at either end. Go `range` reads each byte of a cut char as one
/// RuneError, which is one UTF-16 unit, and the whole chars count as
/// `utf16_len` counts them. So a range inside one char counts
/// `end - start`. `start` must be at most `end` and `s.len()`; a byte past
/// the end of `s` also counts 1.
// PORT: `s` is the port form (see `GO_STRING_MARKER`). An edge `k` bytes
// into a unit of `g` Go bytes is Go offset `min(k, g)` in the unit, as
// `go_byte_offset` maps it. So a cut start counts the `g - k` Go bytes
// that are left, and a cut end counts `k` bytes, or the whole unit when
// `k >= g`.
// PERF: source map ranges are mostly ASCII. `go_unit_cut_at` returns after
// one compare at an ASCII byte, `is_ascii` checks a word at a time, and for
// ASCII the result is the byte length.
pub fn utf16_len_of_range(s: &str, start: usize, end: usize) -> i32 {
    let past = end.saturating_sub(s.len());
    let end = end - past;
    let mut n = past;
    let mut head = start;
    if let Some((at, unit, size)) = go_unit_cut_at(s, start) {
        let (k, g) = (start - at, unit.go_len());
        if end < at + size {
            // The range is inside the unit, after its first Go byte: each
            // Go byte is one RuneError.
            return (n + (end - at).min(g).saturating_sub(k.min(g))) as i32;
        }
        n += g.saturating_sub(k);
        head = at + size;
    }
    let mut tail = end;
    if let Some((at, unit, _)) = go_unit_cut_at(s, end) {
        let (k, g) = (end - at, unit.go_len());
        n += if k < g { k } else { unit.go_utf16_len() };
        tail = at;
    }
    let whole = &s[head..tail];
    let n = n as i32;
    if whole.is_ascii() {
        n + whole.len() as i32
    } else {
        n + utf16_len(whole)
    }
}

// Go: core/core.go:582 GetSpellingSuggestion
// Given a name and a list of candidates, returns the candidate whose name is
// closest to `name`, or `T::default()` (Go zero value) when none is close
// enough.
// PORT: Go `iter.Seq[T]` -> `IntoIterator`; `getName(T)` -> `FnMut(&T)`.
// Names are port forms (see `GO_STRING_MARKER`). Go counts their bytes
// (`go_len`) and runes (`go_runes`, where each invalid byte is U+FFFD).
// PERF: `get_name` returns any string type (`S`), so a caller whose names are
// already stored (such as `&'static str`) makes no `String` per candidate.
pub fn get_spelling_suggestion<T: Clone + Default, S: AsRef<str>>(
    name: &str,
    candidates: impl IntoIterator<Item = T>,
    get_name: impl FnMut(&T) -> S,
    compare: impl FnMut(&T, &T) -> i32,
) -> T {
    get_spelling_suggestion_unexported(
        name, candidates, get_name, compare, 0, /*maxCandidates*/
    )
}

// Go: core/core.go:586 GetSpellingSuggestionWithMaxCandidateCount
pub fn get_spelling_suggestion_with_max_candidate_count<T: Clone + Default, S: AsRef<str>>(
    name: &str,
    candidates: impl IntoIterator<Item = T>,
    get_name: impl FnMut(&T) -> S,
    compare: impl FnMut(&T, &T) -> i32,
    max_candidates: i32,
) -> T {
    get_spelling_suggestion_unexported(name, candidates, get_name, compare, max_candidates)
}

// Go: core/core.go:590 getSpellingSuggestion
// PORT: Go `getSpellingSuggestion` has the same snake name as the exported
// `GetSpellingSuggestion`, which keeps the plain name because other packages
// call it; this private one gets the `_unexported` suffix.
fn get_spelling_suggestion_unexported<T: Clone + Default, S: AsRef<str>>(
    name: &str,
    candidates: impl IntoIterator<Item = T>,
    mut get_name: impl FnMut(&T) -> S,
    mut compare: impl FnMut(&T, &T) -> i32,
    max_candidates: i32,
) -> T {
    let rune_name: Vec<char> = go_runes(name);
    let maximum_length_difference = 2i64.max((rune_name.len() as f64 * 0.34) as i64);
    let mut best_distance = (rune_name.len() as f64 * 0.4).floor() + 0.9; // If the best result is worse than this, don't bother.
    let mut best_candidate = T::default();
    let mut has_best = false;
    let mut checked_candidates = 0;
    // PERF: one rune buffer and one Levenshtein buffer pair for the whole
    // call, as Go's `levenshteinBuffersPool`, not 3 `Vec`s per candidate.
    let mut candidate_runes: Vec<char> = Vec::new();
    let mut buffers = LevenshteinBuffers::default();
    for candidate in candidates {
        checked_candidates += 1;
        if max_candidates > 0 && checked_candidates > max_candidates {
            return T::default();
        }
        let candidate_name = get_name(&candidate);
        let candidate_name: &str = candidate_name.as_ref();
        // PORT: Go compares the candidate byte length with the name rune count.
        let candidate_len = go_len(candidate_name);
        let max_len = candidate_len.max(rune_name.len()) as i64;
        let min_len = candidate_len.min(rune_name.len()) as i64;
        if !candidate_name.is_empty() && max_len - min_len <= maximum_length_difference {
            if candidate_name == name {
                continue;
            }
            candidate_runes.clear();
            if contains_go_string_marker(candidate_name) {
                candidate_runes.extend(go_runes(candidate_name));
            } else {
                candidate_runes.extend(candidate_name.chars());
            }
            // Only consider candidates less than 3 characters long when they differ by case.
            // Otherwise, don't bother, since a user would usually notice differences of a 2-character name.
            if candidate_len < 3 && !equal_fold(&candidate_runes, &rune_name) {
                continue;
            }
            let distance =
                levenshtein_with_max(&mut buffers, &rune_name, &candidate_runes, best_distance);
            if distance < 0.0 {
                continue;
            }
            debug_assert!(distance <= best_distance); // Else `levenshteinWithMax` should return undefined
            if distance < best_distance {
                best_distance = distance;
                best_candidate = candidate;
                has_best = true;
            } else if !has_best || compare(&candidate, &best_candidate) < 0 {
                best_candidate = candidate;
                has_best = true;
            }
        }
    }
    best_candidate
}

// Go: core/core.go:635 GetSpellingSuggestionForStrings
// PORT: Go `strings.Compare` compares the Go bytes (`compare_go_strings`).
// The port form of an invalid byte sorts by its marker bytes, so `str`
// order can break a tie the other way.
pub fn get_spelling_suggestion_for_strings(
    name: &str,
    candidates: impl IntoIterator<Item = String>,
) -> String {
    get_spelling_suggestion(
        name,
        candidates,
        |s| s.clone(),
        |a, b| compare_go_strings(a, b) as i32,
    )
}

// Go: core/core.go:639 levenshteinBuffers
// PORT: Go keeps them in a `sync.Pool`; one `get_spelling_suggestion` call
// owns one.
#[derive(Default)]
struct LevenshteinBuffers {
    previous: Vec<f64>,
    current: Vec<f64>,
}

// Go: core/core.go:650 levenshteinWithMax
fn levenshtein_with_max(
    buffers: &mut LevenshteinBuffers,
    s1: &[char],
    s2: &[char],
    max_value: f64,
) -> f64 {
    let buffer_size = s2.len() + 1;
    // Each row writes every slot of `current`, so old values are never read.
    buffers.previous.resize(buffer_size, 0.0);
    buffers.current.resize(buffer_size, 0.0);
    let mut previous = &mut buffers.previous[..];
    let mut current = &mut buffers.current[..];

    let big = max_value + 0.01;
    for (i, slot) in previous.iter_mut().enumerate() {
        *slot = i as f64;
    }
    for i in 1..=s1.len() {
        let c1 = s1[i - 1];
        let c1_lower = unicode_to_lower(c1);
        let min_j = ((i as f64 - max_value).ceil() as i64).max(1) as usize;
        let max_j = ((max_value + i as f64).floor() as i64).min(s2.len() as i64);
        let mut col_min = i as f64;
        current[0] = col_min;
        for slot in current.iter_mut().take(min_j).skip(1) {
            *slot = big;
        }
        let mut j = min_j as i64;
        while j <= max_j {
            let ju = j as usize;
            let substitution_distance = if c1_lower == unicode_to_lower(s2[ju - 1]) {
                previous[ju - 1] + 0.1
            } else {
                previous[ju - 1] + 2.0
            };
            let dist = if c1 == s2[ju - 1] {
                previous[ju - 1]
            } else {
                (previous[ju] + 1.0).min((current[ju - 1] + 1.0).min(substitution_distance))
            };
            current[ju] = dist;
            col_min = col_min.min(dist);
            j += 1;
        }
        let mut j = (max_j + 1).max(0) as usize;
        while j <= s2.len() {
            current[j] = big;
            j += 1;
        }
        if col_min > max_value {
            // Give up -- everything in this column is > max and it can't get better in future columns.
            return -1.0;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let res = previous[s2.len()];
    if res > max_value {
        return -1.0;
    }
    res
}

// Go: core/core.go:829 Deduplicate
pub fn deduplicate<T: PartialEq + Clone>(slice: Vec<T>) -> Vec<T> {
    if slice.len() > 1 {
        for i in 0..slice.len() {
            if slice[..i].contains(&slice[i]) {
                let mut result: Vec<T> = slice[..i].to_vec();
                for value in &slice[i + 1..] {
                    if !result.contains(value) {
                        result.push(value.clone());
                    }
                }
                return result;
            }
        }
    }
    slice
}

// Go: core/core.go:867 CompareBooleans
// CompareBooleans treats true as greater than false.
pub fn compare_booleans(a: bool, b: bool) -> i32 {
    if a && !b {
        return 1;
    } else if !a && b {
        return -1;
    }
    0
}

// ---------------------------------------------------------------------------
// scanner/scanner.go
// ---------------------------------------------------------------------------

/// Go `textToKeyword` map, in Go source order.
static TEXT_TO_KEYWORD: &[(&str, SyntaxKind)] = &[
    ("abstract", SyntaxKind::AbstractKeyword),
    ("accessor", SyntaxKind::AccessorKeyword),
    ("any", SyntaxKind::AnyKeyword),
    ("as", SyntaxKind::AsKeyword),
    ("asserts", SyntaxKind::AssertsKeyword),
    ("assert", SyntaxKind::AssertKeyword),
    ("bigint", SyntaxKind::BigIntKeyword),
    ("boolean", SyntaxKind::BooleanKeyword),
    ("break", SyntaxKind::BreakKeyword),
    ("case", SyntaxKind::CaseKeyword),
    ("catch", SyntaxKind::CatchKeyword),
    ("class", SyntaxKind::ClassKeyword),
    ("continue", SyntaxKind::ContinueKeyword),
    ("const", SyntaxKind::ConstKeyword),
    ("constructor", SyntaxKind::ConstructorKeyword),
    ("debugger", SyntaxKind::DebuggerKeyword),
    ("declare", SyntaxKind::DeclareKeyword),
    ("default", SyntaxKind::DefaultKeyword),
    ("defer", SyntaxKind::DeferKeyword),
    ("delete", SyntaxKind::DeleteKeyword),
    ("do", SyntaxKind::DoKeyword),
    ("else", SyntaxKind::ElseKeyword),
    ("enum", SyntaxKind::EnumKeyword),
    ("export", SyntaxKind::ExportKeyword),
    ("extends", SyntaxKind::ExtendsKeyword),
    ("false", SyntaxKind::FalseKeyword),
    ("finally", SyntaxKind::FinallyKeyword),
    ("for", SyntaxKind::ForKeyword),
    ("from", SyntaxKind::FromKeyword),
    ("function", SyntaxKind::FunctionKeyword),
    ("get", SyntaxKind::GetKeyword),
    ("if", SyntaxKind::IfKeyword),
    ("immediate", SyntaxKind::ImmediateKeyword),
    ("implements", SyntaxKind::ImplementsKeyword),
    ("import", SyntaxKind::ImportKeyword),
    ("in", SyntaxKind::InKeyword),
    ("infer", SyntaxKind::InferKeyword),
    ("instanceof", SyntaxKind::InstanceOfKeyword),
    ("interface", SyntaxKind::InterfaceKeyword),
    ("intrinsic", SyntaxKind::IntrinsicKeyword),
    ("is", SyntaxKind::IsKeyword),
    ("keyof", SyntaxKind::KeyOfKeyword),
    ("let", SyntaxKind::LetKeyword),
    ("module", SyntaxKind::ModuleKeyword),
    ("namespace", SyntaxKind::NamespaceKeyword),
    ("never", SyntaxKind::NeverKeyword),
    ("new", SyntaxKind::NewKeyword),
    ("null", SyntaxKind::NullKeyword),
    ("number", SyntaxKind::NumberKeyword),
    ("object", SyntaxKind::ObjectKeyword),
    ("package", SyntaxKind::PackageKeyword),
    ("private", SyntaxKind::PrivateKeyword),
    ("protected", SyntaxKind::ProtectedKeyword),
    ("public", SyntaxKind::PublicKeyword),
    ("override", SyntaxKind::OverrideKeyword),
    ("out", SyntaxKind::OutKeyword),
    ("readonly", SyntaxKind::ReadonlyKeyword),
    ("require", SyntaxKind::RequireKeyword),
    ("global", SyntaxKind::GlobalKeyword),
    ("return", SyntaxKind::ReturnKeyword),
    ("satisfies", SyntaxKind::SatisfiesKeyword),
    ("set", SyntaxKind::SetKeyword),
    ("source", SyntaxKind::SourceKeyword),
    ("static", SyntaxKind::StaticKeyword),
    ("string", SyntaxKind::StringKeyword),
    ("super", SyntaxKind::SuperKeyword),
    ("switch", SyntaxKind::SwitchKeyword),
    ("symbol", SyntaxKind::SymbolKeyword),
    ("this", SyntaxKind::ThisKeyword),
    ("throw", SyntaxKind::ThrowKeyword),
    ("true", SyntaxKind::TrueKeyword),
    ("try", SyntaxKind::TryKeyword),
    ("type", SyntaxKind::TypeKeyword),
    ("typeof", SyntaxKind::TypeOfKeyword),
    ("undefined", SyntaxKind::UndefinedKeyword),
    ("unique", SyntaxKind::UniqueKeyword),
    ("unknown", SyntaxKind::UnknownKeyword),
    ("using", SyntaxKind::UsingKeyword),
    ("var", SyntaxKind::VarKeyword),
    ("void", SyntaxKind::VoidKeyword),
    ("while", SyntaxKind::WhileKeyword),
    ("with", SyntaxKind::WithKeyword),
    ("yield", SyntaxKind::YieldKeyword),
    ("async", SyntaxKind::AsyncKeyword),
    ("await", SyntaxKind::AwaitKeyword),
    ("of", SyntaxKind::OfKeyword),
];

/// Go `textToToken` punctuation entries. Go merges `textToKeyword` into the
/// same map; lookups here check both tables.
static TEXT_TO_PUNCTUATION: &[(&str, SyntaxKind)] = &[
    ("{", SyntaxKind::OpenBraceToken),
    ("}", SyntaxKind::CloseBraceToken),
    ("(", SyntaxKind::OpenParenToken),
    (")", SyntaxKind::CloseParenToken),
    ("[", SyntaxKind::OpenBracketToken),
    ("]", SyntaxKind::CloseBracketToken),
    (".", SyntaxKind::DotToken),
    ("...", SyntaxKind::DotDotDotToken),
    (";", SyntaxKind::SemicolonToken),
    (",", SyntaxKind::CommaToken),
    ("<", SyntaxKind::LessThanToken),
    (">", SyntaxKind::GreaterThanToken),
    ("<=", SyntaxKind::LessThanEqualsToken),
    (">=", SyntaxKind::GreaterThanEqualsToken),
    ("==", SyntaxKind::EqualsEqualsToken),
    ("!=", SyntaxKind::ExclamationEqualsToken),
    ("===", SyntaxKind::EqualsEqualsEqualsToken),
    ("!==", SyntaxKind::ExclamationEqualsEqualsToken),
    ("=>", SyntaxKind::EqualsGreaterThanToken),
    ("+", SyntaxKind::PlusToken),
    ("-", SyntaxKind::MinusToken),
    ("**", SyntaxKind::AsteriskAsteriskToken),
    ("*", SyntaxKind::AsteriskToken),
    ("/", SyntaxKind::SlashToken),
    ("%", SyntaxKind::PercentToken),
    ("++", SyntaxKind::PlusPlusToken),
    ("--", SyntaxKind::MinusMinusToken),
    ("<<", SyntaxKind::LessThanLessThanToken),
    ("</", SyntaxKind::LessThanSlashToken),
    (">>", SyntaxKind::GreaterThanGreaterThanToken),
    (">>>", SyntaxKind::GreaterThanGreaterThanGreaterThanToken),
    ("&", SyntaxKind::AmpersandToken),
    ("|", SyntaxKind::BarToken),
    ("^", SyntaxKind::CaretToken),
    ("!", SyntaxKind::ExclamationToken),
    ("~", SyntaxKind::TildeToken),
    ("&&", SyntaxKind::AmpersandAmpersandToken),
    ("||", SyntaxKind::BarBarToken),
    ("?", SyntaxKind::QuestionToken),
    ("??", SyntaxKind::QuestionQuestionToken),
    ("?.", SyntaxKind::QuestionDotToken),
    (":", SyntaxKind::ColonToken),
    ("=", SyntaxKind::EqualsToken),
    ("+=", SyntaxKind::PlusEqualsToken),
    ("-=", SyntaxKind::MinusEqualsToken),
    ("*=", SyntaxKind::AsteriskEqualsToken),
    ("**=", SyntaxKind::AsteriskAsteriskEqualsToken),
    ("/=", SyntaxKind::SlashEqualsToken),
    ("%=", SyntaxKind::PercentEqualsToken),
    ("<<=", SyntaxKind::LessThanLessThanEqualsToken),
    (">>=", SyntaxKind::GreaterThanGreaterThanEqualsToken),
    (
        ">>>=",
        SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken,
    ),
    ("&=", SyntaxKind::AmpersandEqualsToken),
    ("|=", SyntaxKind::BarEqualsToken),
    ("^=", SyntaxKind::CaretEqualsToken),
    ("||=", SyntaxKind::BarBarEqualsToken),
    ("&&=", SyntaxKind::AmpersandAmpersandEqualsToken),
    ("??=", SyntaxKind::QuestionQuestionEqualsToken),
    ("@", SyntaxKind::AtToken),
    ("#", SyntaxKind::HashToken),
    ("`", SyntaxKind::BacktickToken),
];

/// Go `textToKeyword[text]`: returns `SyntaxKind::Unknown` (Go zero value)
/// on a miss.
// PERF: the ported scanner's copy is a match on the same table, which does
// not hash the text.
fn text_to_keyword(text: &str) -> SyntaxKind {
    crate::frontend::scanner::scanner_p1::text_to_keyword(text)
}

// Go: scanner/scanner.go:2213 GetIdentifierToken
pub fn get_identifier_token(str: &str) -> SyntaxKind {
    let bytes = str.as_bytes();
    if str.len() >= 2 && str.len() <= 12 && bytes[0] >= b'a' && bytes[0] <= b'z' {
        let keyword = text_to_keyword(str);
        if keyword != SyntaxKind::Unknown {
            return keyword;
        }
    }
    SyntaxKind::Identifier
}

// Go: scanner/scanner.go:2223 IsValidIdentifier
pub fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    for (i, ch) in s.char_indices() {
        if i == 0 && !is_identifier_start(ch) || i != 0 && !is_identifier_part(ch) {
            return false;
        }
    }
    true
}

// Go: scanner/scanner.go:2236 isWordCharacter
// Section 6.1.4
fn is_word_character(ch: char) -> bool {
    is_ascii_letter(ch) || is_digit(ch) || ch == '_'
}

// Go: scanner/scanner.go:2240 IsIdentifierStart
pub fn is_identifier_start(ch: char) -> bool {
    is_ascii_letter(ch)
        || ch == '_'
        || ch == '$'
        || (ch as u32) >= 0x80 && is_unicode_identifier_start(ch)
}

// Go: scanner/scanner.go:2244 IsIdentifierPart
pub fn is_identifier_part(ch: char) -> bool {
    is_identifier_part_ex(ch, LanguageVariant::STANDARD)
}

// Go: scanner/scanner.go:2248 IsIdentifierPartEx
pub fn is_identifier_part_ex(ch: char, language_variant: LanguageVariant) -> bool {
    is_word_character(ch)
        || ch == '$'
        || (ch as u32) >= 0x80 && is_unicode_identifier_part(ch)
        || language_variant == LanguageVariant::JSX && ch == '-' // ":" is part of JSXNamespacedName, but not JSXIdentifier.
}

// Go: scanner/scanner.go:2262 TokenToString
// PORT: returns `&'static str` (Go string from the `tokenToText` table);
// empty for kinds without fixed text.
pub fn token_to_string(token: SyntaxKind) -> &'static str {
    static TOKEN_TO_TEXT: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    let table = TOKEN_TO_TEXT.get_or_init(|| {
        let mut result = vec![""; SyntaxKind::COUNT as usize];
        for &(text, kind) in TEXT_TO_PUNCTUATION.iter().chain(TEXT_TO_KEYWORD.iter()) {
            result[kind as u16 as usize] = text;
        }
        result
    });
    table[token as u16 as usize]
}

// Go: scanner/scanner.go:2266 StringToToken
pub fn string_to_token(s: &str) -> SyntaxKind {
    if let Some(&(_, kind)) = TEXT_TO_PUNCTUATION.iter().find(|(text, _)| *text == s) {
        return kind;
    }
    text_to_keyword(s)
}

const MAX_ASCII_CHARACTER: u8 = 127;

// Go: scanner/scanner.go:2284 couldStartTrivia
#[allow(dead_code)]
fn could_start_trivia(text: &str, pos: usize) -> bool {
    // Keep in sync with skipTrivia
    let ch = text.as_bytes()[pos];
    match ch {
        // Characters that could start normal trivia
        b'\r' | b'\n' | b'\t' | 0x0B | 0x0C | b' ' | b'/'
        // Characters that could start conflict marker trivia
        | b'<' | b'|' | b'=' | b'>' => true,
        // Only if its the beginning can we have #! trivia
        b'#' => pos == 0,
        _ => ch > MAX_ASCII_CHARACTER,
    }
}

/// Go `SkipTriviaOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SkipTriviaOptions {
    pub stop_after_line_break: bool,
    pub stop_at_comments: bool,
    pub in_js_doc: bool,
}

// Go: scanner/scanner.go:2306 SkipTrivia
pub fn skip_trivia(text: &str, pos: i32) -> i32 {
    skip_trivia_ex(text, pos, None)
}

// Go: scanner/scanner.go:2310 SkipTriviaEx
pub fn skip_trivia_ex(text: &str, pos: i32, options: Option<&SkipTriviaOptions>) -> i32 {
    if position_is_synthesized(pos) {
        return pos;
    }
    let default_options = SkipTriviaOptions::default();
    let options = options.unwrap_or(&default_options);

    let bytes = text.as_bytes();
    let text_len = bytes.len();
    let mut pos = pos as usize;
    let mut can_consume_star = false;
    // Keep in sync with couldStartTrivia
    loop {
        if pos >= text_len {
            return pos as i32;
        }
        let (ch, size) = decode_rune_at(text, pos);
        match ch {
            '\r' | '\n' => {
                if ch == '\r' && pos + 1 < text_len && bytes[pos + 1] == b'\n' {
                    pos += 1;
                }
                pos += 1;
                if options.stop_after_line_break {
                    return pos as i32;
                }
                can_consume_star = options.in_js_doc;
                continue;
            }
            '\t' | '\u{000B}' | '\u{000C}' | ' ' => {
                pos += 1;
                continue;
            }
            '/' => {
                if !options.stop_at_comments && pos + 1 < text_len {
                    if bytes[pos + 1] == b'/' {
                        pos += 2;
                        while pos < text_len {
                            let (ch, size) = decode_rune_at(text, pos);
                            if is_line_break(ch) {
                                break;
                            }
                            pos += size;
                        }
                        can_consume_star = false;
                        continue;
                    }
                    if bytes[pos + 1] == b'*' {
                        pos += 2;
                        while pos < text_len {
                            if bytes[pos] == b'*' && (pos + 1 < text_len) && bytes[pos + 1] == b'/'
                            {
                                pos += 2;
                                break;
                            }
                            let (_, size) = decode_rune_at(text, pos);
                            pos += size;
                        }
                        can_consume_star = false;
                        continue;
                    }
                }
            }
            '<' | '|' | '=' | '>' => {
                if is_conflict_marker_trivia(text, pos) {
                    pos = scan_conflict_marker_trivia(text, pos, None);
                    can_consume_star = false;
                    continue;
                }
            }
            '#' => {
                if pos == 0 && is_shebang_trivia(text, pos) {
                    pos = scan_shebang_trivia(text, pos);
                    can_consume_star = false;
                    continue;
                }
            }
            '*' => {
                if can_consume_star {
                    pos += 1;
                    can_consume_star = false;
                    continue;
                }
            }
            _ => {
                if (ch as u32) > u32::from(MAX_ASCII_CHARACTER) && is_white_space_like(ch) {
                    pos += size;
                    continue;
                }
            }
        }
        return pos as i32;
    }
}

// All conflict markers consist of the same character repeated seven times.  If it is
// a <<<<<<< or >>>>>>> marker then it is also followed by a space.
const MERGE_CONFLICT_MARKER_LENGTH: usize = "<<<<<<<".len();

// Go: scanner/scanner.go:2408 isConflictMarkerTrivia
pub(crate) fn is_conflict_marker_trivia(text: &str, pos: usize) -> bool {
    let bytes = text.as_bytes();

    // Fast reject: a conflict marker is the same byte repeated seven times. If the
    // second byte differs (the overwhelmingly common case for `<`, `>`, `=`, `|`
    // tokens), it cannot be a marker, so skip the line-start check entirely.
    if pos + 1 >= bytes.len() || bytes[pos + 1] != bytes[pos] {
        return false;
    }

    // Conflict markers must be at the start of a line.
    let mut at_line_start = pos == 0 || is_line_break(char::from(bytes[pos - 1]));
    if !at_line_start && pos >= 2 {
        let (prev, _) = decode_last_rune_before(text, pos - 2);
        at_line_start = is_line_break(prev);
    }
    if at_line_start {
        let ch = bytes[pos];

        if (pos + MERGE_CONFLICT_MARKER_LENGTH) < bytes.len() {
            for i in 0..MERGE_CONFLICT_MARKER_LENGTH {
                if bytes[pos + i] != ch {
                    return false;
                }
            }

            return ch == b'=' || bytes[pos + MERGE_CONFLICT_MARKER_LENGTH] == b' ';
        }
    }

    false
}

// Go: scanner/scanner.go:2443 scanConflictMarkerTrivia
// PORT: `reportError` keeps the Go shape; only `SkipTriviaEx` calls this and
// it passes nil.
pub(crate) fn scan_conflict_marker_trivia(
    text: &str,
    pos: usize,
    report_error: Option<&mut dyn FnMut(&'static crate::diagnostics::Message, i32, i32)>,
) -> usize {
    if let Some(report_error) = report_error {
        report_error(
            diag::Merge_conflict_marker_encountered,
            pos as i32,
            MERGE_CONFLICT_MARKER_LENGTH as i32,
        );
    }
    let mut pos = pos;
    let (mut ch, mut size) = decode_rune_at(text, pos);
    let length = text.len();

    if ch == '<' || ch == '>' {
        while pos < length && !is_line_break(ch) {
            pos += size;
            (ch, size) = decode_rune_at(text, pos);
        }
    } else {
        assert!(
            ch == '|' || ch == '=',
            "Assertion failed: ch must be either '|' or '='"
        );
        // Consume everything from the start of a ||||||| or ======= marker to the start
        // of the next ======= or >>>>>>> marker.
        let bytes = text.as_bytes();
        while pos < length {
            let current_char = bytes[pos];
            if (current_char == b'=' || current_char == b'>')
                && char::from(current_char) != ch
                && is_conflict_marker_trivia(text, pos)
            {
                break;
            }

            pos += 1;
        }
    }

    pos
}

// Go: scanner/scanner.go:2474 isShebangTrivia
pub(crate) fn is_shebang_trivia(text: &str, pos: usize) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    assert!(
        pos == 0,
        "Shebangs check must only be done at the start of the file"
    );
    bytes[0] == b'#' && bytes[1] == b'!'
}

// Go: scanner/scanner.go:2484 scanShebangTrivia
pub(crate) fn scan_shebang_trivia(text: &str, pos: usize) -> usize {
    let mut pos = pos + 2;
    while pos < text.len() {
        let (ch, size) = decode_rune_at(text, pos);
        if is_line_break(ch) {
            break;
        }
        pos += size;
    }
    pos
}

// Go: scanner/scanner.go:2496 GetShebang
pub fn get_shebang(text: &str) -> String {
    if !is_shebang_trivia(text, 0) {
        return String::new();
    }

    let end = scan_shebang_trivia(text, 0);
    text[..end].to_string()
}

// Go: scanner/scanner.go:2515 ScanTokenAtPosition
pub fn scan_token_at_position(source_file: Node, pos: i32) -> SyntaxKind {
    let sf_text = source_file_text(source_file);
    let s = get_scanner_for_source_file(source_file, &sf_text, pos);
    s.token()
}

// Go: scanner/scanner.go:2520 GetRangeOfTokenAtPosition
pub fn get_range_of_token_at_position(source_file: Node, pos: i32) -> TextRange {
    let sf_text = source_file_text(source_file);
    let s = get_scanner_for_source_file(source_file, &sf_text, pos);
    TextRange::new(s.token_start(), s.token_end())
}

// Go: scanner/scanner.go:2525 GetTokenPosOfNode
pub fn get_token_pos_of_node(node: Node, source_file: Node, include_js_doc: bool) -> i32 {
    // With nodes that have no width (i.e. 'Missing' nodes), we actually *don't*
    // want to skip trivia because this will launch us forward to the next token.
    if node_is_missing(node) {
        return node.pos();
    }
    if is_js_doc_node(node) || node.kind() == SyntaxKind::JsxText {
        // JsxText cannot actually contain comments, even though the scanner will think it sees comments
        return skip_trivia_ex(
            &source_file_text(source_file),
            node.pos(),
            Some(&SkipTriviaOptions {
                stop_at_comments: true,
                ..SkipTriviaOptions::default()
            }),
        );
    }
    if include_js_doc && !node.js_doc(source_file).is_empty() {
        return get_token_pos_of_node(
            node.js_doc(source_file).get(0),
            source_file,
            false, /*includeJSDoc*/
        );
    }
    skip_trivia_ex(
        &source_file_text(source_file),
        node.pos(),
        Some(&SkipTriviaOptions {
            in_js_doc: node.flags().intersects(NodeFlags::JS_DOC),
            ..SkipTriviaOptions::default()
        }),
    )
}

// Go: scanner/scanner.go:2541 getErrorRangeForArrowFunction
fn get_error_range_for_arrow_function(source_file: Node, node: Node) -> TextRange {
    let pos = skip_trivia(&source_file_text(source_file), node.pos());
    let body = node.body();
    if body.is_some() && body.kind() == SyntaxKind::Block {
        let start_line = get_ecma_line_of_position(source_file, body.pos());
        let end_line = get_ecma_line_of_position(source_file, body.end());
        if start_line < end_line {
            // The arrow function spans multiple lines, make the error span be the first line, inclusive.
            return TextRange::new(pos, get_ecma_end_line_position(source_file, start_line) + 1);
        }
    }
    TextRange::new(pos, node.end())
}

// Go: scanner/scanner.go:2555 findOriginatingJSDocSatisfiesTag
fn find_originating_js_doc_satisfies_tag(source_file: Node, node: Node) -> Node {
    let target_type = node.type_();
    if !target_type.flags().intersects(NodeFlags::REPARSED) {
        return Node::NIL;
    }
    let mut current = node.parent();
    while current.is_some() {
        if !current.flags().intersects(NodeFlags::HAS_JS_DOC) {
            current = current.parent();
            continue;
        }
        let mut first_satisfies_tag = Node::NIL;
        for js_doc in current.eager_js_doc(source_file).iter() {
            let tags = js_doc.tags();
            if !tags.is_nil() {
                for tag in tags.nodes().iter() {
                    if !is_js_doc_satisfies_tag(tag) {
                        continue;
                    }
                    if first_satisfies_tag.is_nil() {
                        first_satisfies_tag = tag;
                    }
                    let type_expr = tag.type_expression();
                    if type_expr.is_some() {
                        let t = type_expr.type_();
                        // PORT: Go compares `t.Loc == targetType.Loc` (TextRange equality).
                        if t.is_some()
                            && t.pos() == target_type.pos()
                            && t.end() == target_type.end()
                        {
                            return tag;
                        }
                    }
                }
            }
        }
        return first_satisfies_tag;
    }
    Node::NIL
}

// Go: scanner/scanner.go:2587 GetErrorRangeForNode
pub fn get_error_range_for_node(source_file: Node, node: Node) -> TextRange {
    let mut error_node = node;
    // PORT: the Go `fallthrough` from FunctionDeclaration/MethodDeclaration
    // into the declaration-name case is a flag here.
    let mut use_declaration_name = false;
    match node.kind() {
        SyntaxKind::SourceFile => {
            let pos = skip_trivia(&source_file_text(source_file), 0);
            if pos as usize == source_file_text(source_file).len() {
                return TextRange::new(0, 0);
            }
            return get_range_of_token_at_position(source_file, pos);
        }
        // This list is a work in progress. Add missing node kinds to improve their error spans
        SyntaxKind::FunctionDeclaration | SyntaxKind::MethodDeclaration => {
            if node.flags().intersects(NodeFlags::REPARSED) {
                error_node = node;
            } else {
                use_declaration_name = true;
            }
        }
        SyntaxKind::VariableDeclaration
        | SyntaxKind::BindingElement
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::EnumMember
        | SyntaxKind::FunctionExpression
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::NamespaceImport => {
            use_declaration_name = true;
        }
        SyntaxKind::ClassExpression => {
            error_node = node.name();
        }
        SyntaxKind::ArrowFunction => {
            return get_error_range_for_arrow_function(source_file, node);
        }
        SyntaxKind::CaseClause | SyntaxKind::DefaultClause => {
            let start = skip_trivia(&source_file_text(source_file), node.pos());
            let mut end = node.end();
            let statements = node.statements();
            if !statements.is_empty() {
                end = statements.get(0).pos();
            }
            return TextRange::new(start, end);
        }
        SyntaxKind::ReturnStatement | SyntaxKind::YieldExpression => {
            let pos = skip_trivia(&source_file_text(source_file), node.pos());
            return get_range_of_token_at_position(source_file, pos);
        }
        SyntaxKind::SatisfiesExpression => {
            let js_doc_satisfies_tag = find_originating_js_doc_satisfies_tag(source_file, node);
            if js_doc_satisfies_tag.is_some() {
                let pos = skip_trivia(
                    &source_file_text(source_file),
                    js_doc_satisfies_tag.tag_name().pos(),
                );
                return get_range_of_token_at_position(source_file, pos);
            }
            let pos = skip_trivia(&source_file_text(source_file), node.expression().end());
            return get_range_of_token_at_position(source_file, pos);
        }
        SyntaxKind::Constructor => {
            if node.flags().intersects(NodeFlags::REPARSED) {
                error_node = node;
            } else {
                let sf_text = source_file_text(source_file);
                let mut scanner = get_scanner_for_source_file(source_file, &sf_text, node.pos());
                let start = scanner.token_start();
                while scanner.token() != SyntaxKind::ConstructorKeyword
                    && scanner.token() != SyntaxKind::StringLiteral
                    && scanner.token() != SyntaxKind::EndOfFile
                {
                    scanner.scan();
                }
                return TextRange::new(start, scanner.token_end());
            }
        }
        _ => {}
    }
    if use_declaration_name {
        error_node = get_name_of_declaration(node);
    }
    if error_node.is_nil() {
        // If we don't have a better node, then just set the error on the first token of
        // construct.
        return get_range_of_token_at_position(source_file, node.pos());
    }
    let mut pos = error_node.pos();
    if !node_is_missing(error_node) && !is_jsx_text(error_node) {
        pos = skip_trivia(&source_file_text(source_file), pos);
    }
    TextRange::new(pos, error_node.end())
}

// Go: scanner/scanner.go:2655 ComputeLineOfPosition
pub fn compute_line_of_position(line_starts: &[i32], pos: i32) -> i32 {
    let mut low: i32 = 0;
    let mut high: i32 = line_starts.len() as i32 - 1;
    while low <= high {
        let middle = low + ((high - low) >> 1);
        let value = line_starts[middle as usize];
        if value < pos {
            low = middle + 1;
        } else if value > pos {
            high = middle - 1;
        } else {
            return middle;
        }
    }
    low - 1
}

thread_local! {
    /// Go `SourceFile.ECMALineMap()` cache for files without a frozen store,
    /// keyed by `Node::file_index`.
    static ECMA_LINE_MAPS: RefCell<FxHashMap<usize, &'static [i32]>> = RefCell::new(FxHashMap::default());
    /// The same cache for synthetic (transformed) source files, keyed by node.
    static SYNTHETIC_ECMA_LINE_MAPS: RefCell<FxHashMap<Node, &'static [i32]>> = RefCell::new(FxHashMap::default());
}

// Go: scanner/scanner.go:2672 GetECMALineStarts
// PORT: Go reads the lazily computed `sourceFile.ECMALineMap()`. A frozen
// store file has one shared map for all threads. Other files use a
// per-thread cache keyed by the file index. A synthetic (transformed) source
// file uses a per-thread cache keyed by its node. The map of a freeable file
// version is in its store, so the guard pins the version (lsshells M3b).
pub fn get_ecma_line_starts(source_file: Node) -> FileRef<[i32]> {
    // All synthetic nodes share one file index, so a transformed (synthetic)
    // source file must not use the file index cache.
    if is_synthetic_node(source_file) {
        if let Some(line_map) =
            SYNTHETIC_ECMA_LINE_MAPS.with(|maps| maps.borrow().get(&source_file).copied())
        {
            return FileRef::Static(line_map);
        }
        let line_map: &'static [i32] =
            Vec::leak(compute_ecma_line_starts(&source_file_text(source_file)));
        SYNTHETIC_ECMA_LINE_MAPS.with(|maps| maps.borrow_mut().insert(source_file, line_map));
        return FileRef::Static(line_map);
    }
    let file_index = source_file.file_index();
    if let Some(line_map) = crate::ast::store::frozen_file_ecma_line_starts(file_index) {
        return line_map;
    }
    if let Some(line_map) = ECMA_LINE_MAPS.with(|maps| maps.borrow().get(&file_index).copied()) {
        return FileRef::Static(line_map);
    }
    let line_map: &'static [i32] =
        Vec::leak(compute_ecma_line_starts(&source_file_text(source_file)));
    ECMA_LINE_MAPS.with(|maps| maps.borrow_mut().insert(file_index, line_map));
    FileRef::Static(line_map)
}

/// Runs `read` on `get_ecma_line_starts(source_file)` with no guard: a
/// freeable file version is pinned only while `read` runs.
// PERF: lsshells M3 repair. A `FileRef` guard of a freeable file version
// clones and drops its `Arc`; the position reads of the edited file are
// frequent.
fn with_ecma_line_starts<R>(source_file: Node, read: impl FnOnce(&[i32]) -> R) -> R {
    let mut read = Some(read);
    if !is_synthetic_node(source_file)
        && let Some(result) =
            crate::ast::store::with_frozen_file_ecma_line_starts(source_file.file_index(), |map| {
                (read.take().expect("the line map is read once"))(map)
            })
    {
        return result;
    }
    let read = read.take().expect("the line map is read once");
    read(&get_ecma_line_starts(source_file))
}

// Go: scanner/scanner.go:2676 GetECMALineOfPosition
pub fn get_ecma_line_of_position(source_file: Node, pos: i32) -> i32 {
    with_ecma_line_starts(source_file, |line_map| {
        compute_line_of_position(line_map, pos)
    })
}

// Go: scanner/scanner.go:2684 GetECMALineAndUTF16CharacterOfPosition
// GetECMALineAndUTF16CharacterOfPosition returns the 0-based line number and the
// UTF-16 code unit offset from the start of that line for the given byte position.
// Uses ECMAScript line separators (LF, CR, CRLF, LS, PS).
// PORT: `pos` can be inside a char. The regular expression scanner reads
// a real U+FFFD as three single bytes in a non-Unicode pattern, as Go does,
// and reports errors there. Go `range` reads each byte of the cut char as
// one RuneError, which is one UTF-16 unit.
pub fn get_ecma_line_and_utf16_character_of_position(source_file: Node, pos: i32) -> (i32, i32) {
    // The panics of a bad `pos` come after the read, outside the line map
    // pin (`with_ecma_line_starts`).
    let (line, line_start) = with_ecma_line_starts(source_file, |line_map| {
        let line = compute_line_of_position(line_map, pos);
        (line, usize::try_from(line).map_or(0, |line| line_map[line]))
    });
    ecma_utf16_character_of_line_position(&source_file_text(source_file), line, line_start, pos)
}

/// The Go panic of `text[i:pos]` (the slice in Go
/// `GetECMALineAndUTF16CharacterOfPosition`) for a Go int `go_pos` (a Go
/// byte offset, see `go_byte_offset`) past the end of `text`.
pub fn panic_past_text(text: &str, go_pos: i64) -> ! {
    let go_len = go_len(text);
    crate::core::go_panic(format!(
        "runtime error: slice bounds out of range [:{go_pos}] with length {go_len}"
    ))
}

/// Go `GetECMALineAndUTF16CharacterOfPosition` (scanner.go:2684) on a text
/// and its ECMA line map, for the port forms that have no file node.
pub fn ecma_line_and_utf16_character_of_text_position(
    line_map: &[i32],
    text: &str,
    pos: i32,
) -> (i32, i32) {
    let line = compute_line_of_position(line_map, pos);
    let line_start = usize::try_from(line).map_or(0, |line| line_map[line]);
    ecma_utf16_character_of_line_position(text, line, line_start, pos)
}

/// The end of Go `GetECMALineAndUTF16CharacterOfPosition`: `line` and
/// `core.UTF16Len(text[lineMap[line]:pos])`, where `line_start` is
/// `lineMap[line]` (any value for line -1). A `pos` inside a char counts
/// each byte of the cut char as one unit (see
/// `get_ecma_line_and_utf16_character_of_position`).
// A `pos` before the text (line -1) or past its end comes only from a bad
// position in the input (a diagnostic in a `.tsbuildinfo`). Go
// `lineMap[line]` and `text[lineMap[line]:pos]` panic there
// (`panic_past_text`).
fn ecma_utf16_character_of_line_position(
    text: &str,
    line: i32,
    line_start: i32,
    pos: i32,
) -> (i32, i32) {
    if line < 0 {
        crate::core::go_panic(format!("runtime error: index out of range [{line}]"));
    }
    let end = pos as usize;
    if end > text.len() {
        panic_past_text(text, i64::from(go_byte_offset(text, pos)));
    }
    let mut boundary = end;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let character = utf16_len(&text[line_start as usize..boundary]) + (end - boundary) as i32;
    (line, character)
}

// Go: scanner/scanner.go:2695 GetECMALineAndByteOffsetOfPosition
// GetECMALineAndByteOffsetOfPosition returns the 0-based line number and the
// raw UTF-8 byte offset from the start of that line for the given byte position.
pub fn get_ecma_line_and_byte_offset_of_position(source_file: Node, pos: i32) -> (i32, i32) {
    with_ecma_line_starts(source_file, |line_map| {
        let line = compute_line_of_position(line_map, pos);
        let byte_offset = pos - line_map[line as usize];
        (line, byte_offset)
    })
}

// Go: scanner/scanner.go:2702 GetECMAEndLinePosition
pub fn get_ecma_end_line_position(source_file: Node, line: i32) -> i32 {
    let text = source_file_text(source_file);
    let mut pos = get_ecma_line_starts(source_file)[line as usize] as usize;
    loop {
        let (ch, size) = decode_rune_at(&text, pos);
        if size == 0 || is_line_break(ch) {
            return pos as i32 - 1;
        }
        pos += size;
    }
}

// Go: scanner/scanner.go:2730 ComputePositionOfLineAndByteOffset
// ComputePositionOfLineAndByteOffset computes a byte position from a line and
// raw byte offset from the line start. This is a simple addition with validation.
pub fn compute_position_of_line_and_byte_offset(
    line_starts: &[i32],
    line: i32,
    byte_offset: i32,
) -> i32 {
    if line < 0 || line as usize >= line_starts.len() {
        panic!(
            "Bad line number. Line: {}, lineStarts.length: {}.",
            line,
            line_starts.len()
        );
    }
    line_starts[line as usize] + byte_offset
}

// ---------------------------------------------------------------------------
// scanner/utilities.go
// ---------------------------------------------------------------------------

// Go: scanner/utilities.go:14 tokenIsIdentifierOrKeyword
#[allow(dead_code)]
fn token_is_identifier_or_keyword(token: SyntaxKind) -> bool {
    token as u16 >= SyntaxKind::Identifier as u16
}

// Go: scanner/utilities.go:18 IdentifierToKeywordKind
pub fn identifier_to_keyword_kind(node: Node) -> SyntaxKind {
    text_to_keyword(node.text())
}

// Go: scanner/utilities.go:22 GetSourceTextOfNodeFromSourceFile
pub fn get_source_text_of_node_from_source_file(
    source_file: Node,
    node: Node,
    include_trivia: bool,
) -> String {
    get_text_of_node_from_source_text(&source_file_text(source_file), node, include_trivia)
}

// Go: scanner/utilities.go:26 isJSDocTypeExpressionOrChild
fn is_js_doc_type_expression_or_child(node: Node) -> bool {
    if is_js_doc_type_expression(node) {
        return true;
    }
    if !node
        .flags()
        .intersects(NodeFlags::JS_DOC | NodeFlags::REPARSED)
    {
        return false;
    }
    let mut current = node;
    while current.is_some() {
        if is_type_node(current) {
            return true;
        }
        current = current.parent();
    }
    false
}

// Go: scanner/utilities.go:41 normalizeJSDocTypeSourceText
fn normalize_js_doc_type_source_text(text: &str) -> String {
    let line_starts = compute_ecma_line_starts(text);
    if line_starts.len() == 1 {
        return strip_leading_js_doc_comment(text).to_string();
    }

    let mut result = String::with_capacity(text.len());
    let new_line = NewLineKind::LF.get_new_line_character();
    for (i, &line_start) in line_starts.iter().enumerate() {
        if i > 0 {
            result.push_str(new_line);
        }
        let mut line_end = text.len();
        if i + 1 < line_starts.len() {
            line_end = line_starts[i + 1] as usize;
        }
        let line = text[line_start as usize..line_end].trim_end_matches(is_line_break);
        result.push_str(strip_leading_js_doc_comment(line));
    }
    result
}

// Go: scanner/utilities.go:64 stripLeadingJSDocComment
fn strip_leading_js_doc_comment(line: &str) -> &str {
    let mut line = line.trim_start_matches(is_white_space_like);
    if let Some(rest) = line.strip_prefix('*') {
        line = rest;
    }
    line.trim_start_matches(is_white_space_like)
}

// Go: scanner/utilities.go:72 GetTextOfNodeFromSourceText
pub fn get_text_of_node_from_source_text(
    source_text: &str,
    node: Node,
    include_trivia: bool,
) -> String {
    if node_is_missing(node) {
        return String::new();
    }
    let mut pos = node.pos();
    if !include_trivia {
        pos = skip_trivia(source_text, pos);
    }
    // PORT: a node of a JSDoc comment that ends the file inside a char can
    // start or end inside that char (parser `jsdoc_text_cut`). Go slices the
    // bytes there; `go_cut_slice` keeps the cut bytes as Go does.
    let mut text = go_cut_slice(source_text, pos as usize, node.end() as usize);
    if is_js_doc_type_expression_or_child(node) {
        text = Cow::Owned(normalize_js_doc_type_source_text(&text));
    }
    if node
        .flags()
        .intersects(NodeFlags::REPARSER_TRANSFORMED_LITERAL)
    {
        // This is similar to `getLiteralTextOfNode` in the printer, but without the context of an `emitContext` to provide overrides
        if is_string_literal(node) {
            if node.token_flags().intersects(TokenFlags::SINGLE_QUOTE) {
                return format!("'{text}'");
            }
            return format!("\"{text}\"");
        } else if is_identifier(node) {
            return node.text().to_string();
        }
        // Only the above node kinds are currently transformed into one another by the reparser, requiring the textual remapping.
        // (Any reamppings done by emit transforms are handled by `getLiteralTextOfNode` in the printer)
        // Fail on any other kinds.
        crate::gostd::debug::fail_bad_syntax_kind(
            node.kind(),
            Some("Unexpected reparser-transformed node kind"),
        );
    }
    text.into_owned()
}

// Go: scanner/utilities.go:102 GetTextOfNode
pub fn get_text_of_node(node: Node) -> String {
    get_source_text_of_node_from_source_file(
        get_source_file_of_node(node),
        node,
        false, /*includeTrivia*/
    )
}

// Go: scanner/utilities.go:106 GetTextOfJSDocComment
pub fn get_text_of_js_doc_comment(comment: NodeList) -> String {
    if comment.is_nil() {
        return String::new();
    }
    let mut b = String::new();
    for n in comment.nodes().iter() {
        match n.kind() {
            SyntaxKind::JsDocText => b.push_str(n.text()),
            SyntaxKind::JsDocLink | SyntaxKind::JsDocLinkCode | SyntaxKind::JsDocLinkPlain => {
                b.push_str(&get_text_of_node(n));
            }
            _ => {}
        }
    }
    // PORT: Go `unicode.IsSpace` matches Rust `char::is_whitespace` (White_Space).
    b.trim_end_matches(char::is_whitespace).to_string()
}

// Go: scanner/utilities.go:122 DeclarationNameToString
pub fn declaration_name_to_string(name: Node) -> String {
    if name.is_nil() || name.pos() == name.end() {
        return "(Missing)".to_string();
    }
    get_text_of_node(name)
}

// Go: scanner/utilities.go:129 IsIdentifierText
pub fn is_identifier_text(name: &str, language_variant: LanguageVariant) -> bool {
    let (mut ch, mut size) = decode_rune_at(name, 0);
    if !is_identifier_start(ch) {
        return false;
    }
    let mut i = size;
    while i < name.len() {
        (ch, size) = decode_rune_at(name, i);
        if !is_identifier_part_ex(ch, language_variant) {
            return false;
        }
        i += size;
    }
    true
}

// Go: scanner/utilities.go:144 IsIntrinsicJsxName
pub fn is_intrinsic_jsx_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !name.is_empty() && (bytes[0] >= b'a' && bytes[0] <= b'z' || name.contains('-'))
}

// ---------------------------------------------------------------------------
// Generated Unicode tables (stringutil/identifier_parts_generated.go and
// stringutil/js_case_generated.go).
// ---------------------------------------------------------------------------
// Go: stringutil/identifier_parts_generated.go unicodeESNextIdentifierStart (Unicode 15.1.0)
static UNICODE_ES_NEXT_IDENTIFIER_START: &[(u32, u32, u32)] = &[
    (0x41, 0x5A, 1),
    (0x61, 0x7A, 1),
    (0xAA, 0xB5, 11),
    (0xBA, 0xC0, 6),
    (0xC1, 0xD6, 1),
    (0xD8, 0xF6, 1),
    (0xF8, 0x2C1, 1),
    (0x2C6, 0x2D1, 1),
    (0x2E0, 0x2E4, 1),
    (0x2EC, 0x2EE, 2),
    (0x370, 0x374, 1),
    (0x376, 0x377, 1),
    (0x37A, 0x37D, 1),
    (0x37F, 0x386, 7),
    (0x388, 0x38A, 1),
    (0x38C, 0x38E, 2),
    (0x38F, 0x3A1, 1),
    (0x3A3, 0x3F5, 1),
    (0x3F7, 0x481, 1),
    (0x48A, 0x52F, 1),
    (0x531, 0x556, 1),
    (0x559, 0x560, 7),
    (0x561, 0x588, 1),
    (0x5D0, 0x5EA, 1),
    (0x5EF, 0x5F2, 1),
    (0x620, 0x64A, 1),
    (0x66E, 0x66F, 1),
    (0x671, 0x6D3, 1),
    (0x6D5, 0x6E5, 16),
    (0x6E6, 0x6EE, 8),
    (0x6EF, 0x6FA, 11),
    (0x6FB, 0x6FC, 1),
    (0x6FF, 0x710, 17),
    (0x712, 0x72F, 1),
    (0x74D, 0x7A5, 1),
    (0x7B1, 0x7CA, 25),
    (0x7CB, 0x7EA, 1),
    (0x7F4, 0x7F5, 1),
    (0x7FA, 0x800, 6),
    (0x801, 0x815, 1),
    (0x81A, 0x824, 10),
    (0x828, 0x840, 24),
    (0x841, 0x858, 1),
    (0x860, 0x86A, 1),
    (0x870, 0x887, 1),
    (0x889, 0x88E, 1),
    (0x8A0, 0x8C9, 1),
    (0x904, 0x939, 1),
    (0x93D, 0x950, 19),
    (0x958, 0x961, 1),
    (0x971, 0x980, 1),
    (0x985, 0x98C, 1),
    (0x98F, 0x990, 1),
    (0x993, 0x9A8, 1),
    (0x9AA, 0x9B0, 1),
    (0x9B2, 0x9B6, 4),
    (0x9B7, 0x9B9, 1),
    (0x9BD, 0x9CE, 17),
    (0x9DC, 0x9DD, 1),
    (0x9DF, 0x9E1, 1),
    (0x9F0, 0x9F1, 1),
    (0x9FC, 0xA05, 9),
    (0xA06, 0xA0A, 1),
    (0xA0F, 0xA10, 1),
    (0xA13, 0xA28, 1),
    (0xA2A, 0xA30, 1),
    (0xA32, 0xA33, 1),
    (0xA35, 0xA36, 1),
    (0xA38, 0xA39, 1),
    (0xA59, 0xA5C, 1),
    (0xA5E, 0xA72, 20),
    (0xA73, 0xA74, 1),
    (0xA85, 0xA8D, 1),
    (0xA8F, 0xA91, 1),
    (0xA93, 0xAA8, 1),
    (0xAAA, 0xAB0, 1),
    (0xAB2, 0xAB3, 1),
    (0xAB5, 0xAB9, 1),
    (0xABD, 0xAD0, 19),
    (0xAE0, 0xAE1, 1),
    (0xAF9, 0xB05, 12),
    (0xB06, 0xB0C, 1),
    (0xB0F, 0xB10, 1),
    (0xB13, 0xB28, 1),
    (0xB2A, 0xB30, 1),
    (0xB32, 0xB33, 1),
    (0xB35, 0xB39, 1),
    (0xB3D, 0xB5C, 31),
    (0xB5D, 0xB5F, 2),
    (0xB60, 0xB61, 1),
    (0xB71, 0xB83, 18),
    (0xB85, 0xB8A, 1),
    (0xB8E, 0xB90, 1),
    (0xB92, 0xB95, 1),
    (0xB99, 0xB9A, 1),
    (0xB9C, 0xB9E, 2),
    (0xB9F, 0xBA3, 4),
    (0xBA4, 0xBA8, 4),
    (0xBA9, 0xBAA, 1),
    (0xBAE, 0xBB9, 1),
    (0xBD0, 0xC05, 53),
    (0xC06, 0xC0C, 1),
    (0xC0E, 0xC10, 1),
    (0xC12, 0xC28, 1),
    (0xC2A, 0xC39, 1),
    (0xC3D, 0xC58, 27),
    (0xC59, 0xC5A, 1),
    (0xC5D, 0xC60, 3),
    (0xC61, 0xC80, 31),
    (0xC85, 0xC8C, 1),
    (0xC8E, 0xC90, 1),
    (0xC92, 0xCA8, 1),
    (0xCAA, 0xCB3, 1),
    (0xCB5, 0xCB9, 1),
    (0xCBD, 0xCDD, 32),
    (0xCDE, 0xCE0, 2),
    (0xCE1, 0xCF1, 16),
    (0xCF2, 0xD04, 18),
    (0xD05, 0xD0C, 1),
    (0xD0E, 0xD10, 1),
    (0xD12, 0xD3A, 1),
    (0xD3D, 0xD4E, 17),
    (0xD54, 0xD56, 1),
    (0xD5F, 0xD61, 1),
    (0xD7A, 0xD7F, 1),
    (0xD85, 0xD96, 1),
    (0xD9A, 0xDB1, 1),
    (0xDB3, 0xDBB, 1),
    (0xDBD, 0xDC0, 3),
    (0xDC1, 0xDC6, 1),
    (0xE01, 0xE30, 1),
    (0xE32, 0xE33, 1),
    (0xE40, 0xE46, 1),
    (0xE81, 0xE82, 1),
    (0xE84, 0xE86, 2),
    (0xE87, 0xE8A, 1),
    (0xE8C, 0xEA3, 1),
    (0xEA5, 0xEA7, 2),
    (0xEA8, 0xEB0, 1),
    (0xEB2, 0xEB3, 1),
    (0xEBD, 0xEC0, 3),
    (0xEC1, 0xEC4, 1),
    (0xEC6, 0xEDC, 22),
    (0xEDD, 0xEDF, 1),
    (0xF00, 0xF40, 64),
    (0xF41, 0xF47, 1),
    (0xF49, 0xF6C, 1),
    (0xF88, 0xF8C, 1),
    (0x1000, 0x102A, 1),
    (0x103F, 0x1050, 17),
    (0x1051, 0x1055, 1),
    (0x105A, 0x105D, 1),
    (0x1061, 0x1065, 4),
    (0x1066, 0x106E, 8),
    (0x106F, 0x1070, 1),
    (0x1075, 0x1081, 1),
    (0x108E, 0x10A0, 18),
    (0x10A1, 0x10C5, 1),
    (0x10C7, 0x10CD, 6),
    (0x10D0, 0x10FA, 1),
    (0x10FC, 0x1248, 1),
    (0x124A, 0x124D, 1),
    (0x1250, 0x1256, 1),
    (0x1258, 0x125A, 2),
    (0x125B, 0x125D, 1),
    (0x1260, 0x1288, 1),
    (0x128A, 0x128D, 1),
    (0x1290, 0x12B0, 1),
    (0x12B2, 0x12B5, 1),
    (0x12B8, 0x12BE, 1),
    (0x12C0, 0x12C2, 2),
    (0x12C3, 0x12C5, 1),
    (0x12C8, 0x12D6, 1),
    (0x12D8, 0x1310, 1),
    (0x1312, 0x1315, 1),
    (0x1318, 0x135A, 1),
    (0x1380, 0x138F, 1),
    (0x13A0, 0x13F5, 1),
    (0x13F8, 0x13FD, 1),
    (0x1401, 0x166C, 1),
    (0x166F, 0x167F, 1),
    (0x1681, 0x169A, 1),
    (0x16A0, 0x16EA, 1),
    (0x16EE, 0x16F8, 1),
    (0x1700, 0x1711, 1),
    (0x171F, 0x1731, 1),
    (0x1740, 0x1751, 1),
    (0x1760, 0x176C, 1),
    (0x176E, 0x1770, 1),
    (0x1780, 0x17B3, 1),
    (0x17D7, 0x17DC, 5),
    (0x1820, 0x1878, 1),
    (0x1880, 0x18A8, 1),
    (0x18AA, 0x18B0, 6),
    (0x18B1, 0x18F5, 1),
    (0x1900, 0x191E, 1),
    (0x1950, 0x196D, 1),
    (0x1970, 0x1974, 1),
    (0x1980, 0x19AB, 1),
    (0x19B0, 0x19C9, 1),
    (0x1A00, 0x1A16, 1),
    (0x1A20, 0x1A54, 1),
    (0x1AA7, 0x1B05, 94),
    (0x1B06, 0x1B33, 1),
    (0x1B45, 0x1B4C, 1),
    (0x1B83, 0x1BA0, 1),
    (0x1BAE, 0x1BAF, 1),
    (0x1BBA, 0x1BE5, 1),
    (0x1C00, 0x1C23, 1),
    (0x1C4D, 0x1C4F, 1),
    (0x1C5A, 0x1C7D, 1),
    (0x1C80, 0x1C88, 1),
    (0x1C90, 0x1CBA, 1),
    (0x1CBD, 0x1CBF, 1),
    (0x1CE9, 0x1CEC, 1),
    (0x1CEE, 0x1CF3, 1),
    (0x1CF5, 0x1CF6, 1),
    (0x1CFA, 0x1D00, 6),
    (0x1D01, 0x1DBF, 1),
    (0x1E00, 0x1F15, 1),
    (0x1F18, 0x1F1D, 1),
    (0x1F20, 0x1F45, 1),
    (0x1F48, 0x1F4D, 1),
    (0x1F50, 0x1F57, 1),
    (0x1F59, 0x1F5F, 2),
    (0x1F60, 0x1F7D, 1),
    (0x1F80, 0x1FB4, 1),
    (0x1FB6, 0x1FBC, 1),
    (0x1FBE, 0x1FC2, 4),
    (0x1FC3, 0x1FC4, 1),
    (0x1FC6, 0x1FCC, 1),
    (0x1FD0, 0x1FD3, 1),
    (0x1FD6, 0x1FDB, 1),
    (0x1FE0, 0x1FEC, 1),
    (0x1FF2, 0x1FF4, 1),
    (0x1FF6, 0x1FFC, 1),
    (0x2071, 0x207F, 14),
    (0x2090, 0x209C, 1),
    (0x2102, 0x2107, 5),
    (0x210A, 0x2113, 1),
    (0x2115, 0x2118, 3),
    (0x2119, 0x211D, 1),
    (0x2124, 0x212A, 2),
    (0x212B, 0x2139, 1),
    (0x213C, 0x213F, 1),
    (0x2145, 0x2149, 1),
    (0x214E, 0x2160, 18),
    (0x2161, 0x2188, 1),
    (0x2C00, 0x2CE4, 1),
    (0x2CEB, 0x2CEE, 1),
    (0x2CF2, 0x2CF3, 1),
    (0x2D00, 0x2D25, 1),
    (0x2D27, 0x2D2D, 6),
    (0x2D30, 0x2D67, 1),
    (0x2D6F, 0x2D80, 17),
    (0x2D81, 0x2D96, 1),
    (0x2DA0, 0x2DA6, 1),
    (0x2DA8, 0x2DAE, 1),
    (0x2DB0, 0x2DB6, 1),
    (0x2DB8, 0x2DBE, 1),
    (0x2DC0, 0x2DC6, 1),
    (0x2DC8, 0x2DCE, 1),
    (0x2DD0, 0x2DD6, 1),
    (0x2DD8, 0x2DDE, 1),
    (0x3005, 0x3007, 1),
    (0x3021, 0x3029, 1),
    (0x3031, 0x3035, 1),
    (0x3038, 0x303C, 1),
    (0x3041, 0x3096, 1),
    (0x309B, 0x309F, 1),
    (0x30A1, 0x30FA, 1),
    (0x30FC, 0x30FF, 1),
    (0x3105, 0x312F, 1),
    (0x3131, 0x318E, 1),
    (0x31A0, 0x31BF, 1),
    (0x31F0, 0x31FF, 1),
    (0x3400, 0x4DBF, 1),
    (0x4E00, 0xA48C, 1),
    (0xA4D0, 0xA4FD, 1),
    (0xA500, 0xA60C, 1),
    (0xA610, 0xA61F, 1),
    (0xA62A, 0xA62B, 1),
    (0xA640, 0xA66E, 1),
    (0xA67F, 0xA69D, 1),
    (0xA6A0, 0xA6EF, 1),
    (0xA717, 0xA71F, 1),
    (0xA722, 0xA788, 1),
    (0xA78B, 0xA7CA, 1),
    (0xA7D0, 0xA7D1, 1),
    (0xA7D3, 0xA7D5, 2),
    (0xA7D6, 0xA7D9, 1),
    (0xA7F2, 0xA801, 1),
    (0xA803, 0xA805, 1),
    (0xA807, 0xA80A, 1),
    (0xA80C, 0xA822, 1),
    (0xA840, 0xA873, 1),
    (0xA882, 0xA8B3, 1),
    (0xA8F2, 0xA8F7, 1),
    (0xA8FB, 0xA8FD, 2),
    (0xA8FE, 0xA90A, 12),
    (0xA90B, 0xA925, 1),
    (0xA930, 0xA946, 1),
    (0xA960, 0xA97C, 1),
    (0xA984, 0xA9B2, 1),
    (0xA9CF, 0xA9E0, 17),
    (0xA9E1, 0xA9E4, 1),
    (0xA9E6, 0xA9EF, 1),
    (0xA9FA, 0xA9FE, 1),
    (0xAA00, 0xAA28, 1),
    (0xAA40, 0xAA42, 1),
    (0xAA44, 0xAA4B, 1),
    (0xAA60, 0xAA76, 1),
    (0xAA7A, 0xAA7E, 4),
    (0xAA7F, 0xAAAF, 1),
    (0xAAB1, 0xAAB5, 4),
    (0xAAB6, 0xAAB9, 3),
    (0xAABA, 0xAABD, 1),
    (0xAAC0, 0xAAC2, 2),
    (0xAADB, 0xAADD, 1),
    (0xAAE0, 0xAAEA, 1),
    (0xAAF2, 0xAAF4, 1),
    (0xAB01, 0xAB06, 1),
    (0xAB09, 0xAB0E, 1),
    (0xAB11, 0xAB16, 1),
    (0xAB20, 0xAB26, 1),
    (0xAB28, 0xAB2E, 1),
    (0xAB30, 0xAB5A, 1),
    (0xAB5C, 0xAB69, 1),
    (0xAB70, 0xABE2, 1),
    (0xAC00, 0xD7A3, 1),
    (0xD7B0, 0xD7C6, 1),
    (0xD7CB, 0xD7FB, 1),
    (0xF900, 0xFA6D, 1),
    (0xFA70, 0xFAD9, 1),
    (0xFB00, 0xFB06, 1),
    (0xFB13, 0xFB17, 1),
    (0xFB1D, 0xFB1F, 2),
    (0xFB20, 0xFB28, 1),
    (0xFB2A, 0xFB36, 1),
    (0xFB38, 0xFB3C, 1),
    (0xFB3E, 0xFB40, 2),
    (0xFB41, 0xFB43, 2),
    (0xFB44, 0xFB46, 2),
    (0xFB47, 0xFBB1, 1),
    (0xFBD3, 0xFD3D, 1),
    (0xFD50, 0xFD8F, 1),
    (0xFD92, 0xFDC7, 1),
    (0xFDF0, 0xFDFB, 1),
    (0xFE70, 0xFE74, 1),
    (0xFE76, 0xFEFC, 1),
    (0xFF21, 0xFF3A, 1),
    (0xFF41, 0xFF5A, 1),
    (0xFF66, 0xFFBE, 1),
    (0xFFC2, 0xFFC7, 1),
    (0xFFCA, 0xFFCF, 1),
    (0xFFD2, 0xFFD7, 1),
    (0xFFDA, 0xFFDC, 1),
    (0x10000, 0x1000B, 1),
    (0x1000D, 0x10026, 1),
    (0x10028, 0x1003A, 1),
    (0x1003C, 0x1003D, 1),
    (0x1003F, 0x1004D, 1),
    (0x10050, 0x1005D, 1),
    (0x10080, 0x100FA, 1),
    (0x10140, 0x10174, 1),
    (0x10280, 0x1029C, 1),
    (0x102A0, 0x102D0, 1),
    (0x10300, 0x1031F, 1),
    (0x1032D, 0x1034A, 1),
    (0x10350, 0x10375, 1),
    (0x10380, 0x1039D, 1),
    (0x103A0, 0x103C3, 1),
    (0x103C8, 0x103CF, 1),
    (0x103D1, 0x103D5, 1),
    (0x10400, 0x1049D, 1),
    (0x104B0, 0x104D3, 1),
    (0x104D8, 0x104FB, 1),
    (0x10500, 0x10527, 1),
    (0x10530, 0x10563, 1),
    (0x10570, 0x1057A, 1),
    (0x1057C, 0x1058A, 1),
    (0x1058C, 0x10592, 1),
    (0x10594, 0x10595, 1),
    (0x10597, 0x105A1, 1),
    (0x105A3, 0x105B1, 1),
    (0x105B3, 0x105B9, 1),
    (0x105BB, 0x105BC, 1),
    (0x10600, 0x10736, 1),
    (0x10740, 0x10755, 1),
    (0x10760, 0x10767, 1),
    (0x10780, 0x10785, 1),
    (0x10787, 0x107B0, 1),
    (0x107B2, 0x107BA, 1),
    (0x10800, 0x10805, 1),
    (0x10808, 0x1080A, 2),
    (0x1080B, 0x10835, 1),
    (0x10837, 0x10838, 1),
    (0x1083C, 0x1083F, 3),
    (0x10840, 0x10855, 1),
    (0x10860, 0x10876, 1),
    (0x10880, 0x1089E, 1),
    (0x108E0, 0x108F2, 1),
    (0x108F4, 0x108F5, 1),
    (0x10900, 0x10915, 1),
    (0x10920, 0x10939, 1),
    (0x10980, 0x109B7, 1),
    (0x109BE, 0x109BF, 1),
    (0x10A00, 0x10A10, 16),
    (0x10A11, 0x10A13, 1),
    (0x10A15, 0x10A17, 1),
    (0x10A19, 0x10A35, 1),
    (0x10A60, 0x10A7C, 1),
    (0x10A80, 0x10A9C, 1),
    (0x10AC0, 0x10AC7, 1),
    (0x10AC9, 0x10AE4, 1),
    (0x10B00, 0x10B35, 1),
    (0x10B40, 0x10B55, 1),
    (0x10B60, 0x10B72, 1),
    (0x10B80, 0x10B91, 1),
    (0x10C00, 0x10C48, 1),
    (0x10C80, 0x10CB2, 1),
    (0x10CC0, 0x10CF2, 1),
    (0x10D00, 0x10D23, 1),
    (0x10E80, 0x10EA9, 1),
    (0x10EB0, 0x10EB1, 1),
    (0x10F00, 0x10F1C, 1),
    (0x10F27, 0x10F30, 9),
    (0x10F31, 0x10F45, 1),
    (0x10F70, 0x10F81, 1),
    (0x10FB0, 0x10FC4, 1),
    (0x10FE0, 0x10FF6, 1),
    (0x11003, 0x11037, 1),
    (0x11071, 0x11072, 1),
    (0x11075, 0x11083, 14),
    (0x11084, 0x110AF, 1),
    (0x110D0, 0x110E8, 1),
    (0x11103, 0x11126, 1),
    (0x11144, 0x11147, 3),
    (0x11150, 0x11172, 1),
    (0x11176, 0x11183, 13),
    (0x11184, 0x111B2, 1),
    (0x111C1, 0x111C4, 1),
    (0x111DA, 0x111DC, 2),
    (0x11200, 0x11211, 1),
    (0x11213, 0x1122B, 1),
    (0x1123F, 0x11240, 1),
    (0x11280, 0x11286, 1),
    (0x11288, 0x1128A, 2),
    (0x1128B, 0x1128D, 1),
    (0x1128F, 0x1129D, 1),
    (0x1129F, 0x112A8, 1),
    (0x112B0, 0x112DE, 1),
    (0x11305, 0x1130C, 1),
    (0x1130F, 0x11310, 1),
    (0x11313, 0x11328, 1),
    (0x1132A, 0x11330, 1),
    (0x11332, 0x11333, 1),
    (0x11335, 0x11339, 1),
    (0x1133D, 0x11350, 19),
    (0x1135D, 0x11361, 1),
    (0x11400, 0x11434, 1),
    (0x11447, 0x1144A, 1),
    (0x1145F, 0x11461, 1),
    (0x11480, 0x114AF, 1),
    (0x114C4, 0x114C5, 1),
    (0x114C7, 0x11580, 185),
    (0x11581, 0x115AE, 1),
    (0x115D8, 0x115DB, 1),
    (0x11600, 0x1162F, 1),
    (0x11644, 0x11680, 60),
    (0x11681, 0x116AA, 1),
    (0x116B8, 0x11700, 72),
    (0x11701, 0x1171A, 1),
    (0x11740, 0x11746, 1),
    (0x11800, 0x1182B, 1),
    (0x118A0, 0x118DF, 1),
    (0x118FF, 0x11906, 1),
    (0x11909, 0x1190C, 3),
    (0x1190D, 0x11913, 1),
    (0x11915, 0x11916, 1),
    (0x11918, 0x1192F, 1),
    (0x1193F, 0x11941, 2),
    (0x119A0, 0x119A7, 1),
    (0x119AA, 0x119D0, 1),
    (0x119E1, 0x119E3, 2),
    (0x11A00, 0x11A0B, 11),
    (0x11A0C, 0x11A32, 1),
    (0x11A3A, 0x11A50, 22),
    (0x11A5C, 0x11A89, 1),
    (0x11A9D, 0x11AB0, 19),
    (0x11AB1, 0x11AF8, 1),
    (0x11C00, 0x11C08, 1),
    (0x11C0A, 0x11C2E, 1),
    (0x11C40, 0x11C72, 50),
    (0x11C73, 0x11C8F, 1),
    (0x11D00, 0x11D06, 1),
    (0x11D08, 0x11D09, 1),
    (0x11D0B, 0x11D30, 1),
    (0x11D46, 0x11D60, 26),
    (0x11D61, 0x11D65, 1),
    (0x11D67, 0x11D68, 1),
    (0x11D6A, 0x11D89, 1),
    (0x11D98, 0x11EE0, 328),
    (0x11EE1, 0x11EF2, 1),
    (0x11F02, 0x11F04, 2),
    (0x11F05, 0x11F10, 1),
    (0x11F12, 0x11F33, 1),
    (0x11FB0, 0x12000, 80),
    (0x12001, 0x12399, 1),
    (0x12400, 0x1246E, 1),
    (0x12480, 0x12543, 1),
    (0x12F90, 0x12FF0, 1),
    (0x13000, 0x1342F, 1),
    (0x13441, 0x13446, 1),
    (0x14400, 0x14646, 1),
    (0x16800, 0x16A38, 1),
    (0x16A40, 0x16A5E, 1),
    (0x16A70, 0x16ABE, 1),
    (0x16AD0, 0x16AED, 1),
    (0x16B00, 0x16B2F, 1),
    (0x16B40, 0x16B43, 1),
    (0x16B63, 0x16B77, 1),
    (0x16B7D, 0x16B8F, 1),
    (0x16E40, 0x16E7F, 1),
    (0x16F00, 0x16F4A, 1),
    (0x16F50, 0x16F93, 67),
    (0x16F94, 0x16F9F, 1),
    (0x16FE0, 0x16FE1, 1),
    (0x16FE3, 0x17000, 29),
    (0x17001, 0x187F7, 1),
    (0x18800, 0x18CD5, 1),
    (0x18D00, 0x18D08, 1),
    (0x1AFF0, 0x1AFF3, 1),
    (0x1AFF5, 0x1AFFB, 1),
    (0x1AFFD, 0x1AFFE, 1),
    (0x1B000, 0x1B122, 1),
    (0x1B132, 0x1B150, 30),
    (0x1B151, 0x1B152, 1),
    (0x1B155, 0x1B164, 15),
    (0x1B165, 0x1B167, 1),
    (0x1B170, 0x1B2FB, 1),
    (0x1BC00, 0x1BC6A, 1),
    (0x1BC70, 0x1BC7C, 1),
    (0x1BC80, 0x1BC88, 1),
    (0x1BC90, 0x1BC99, 1),
    (0x1D400, 0x1D454, 1),
    (0x1D456, 0x1D49C, 1),
    (0x1D49E, 0x1D49F, 1),
    (0x1D4A2, 0x1D4A5, 3),
    (0x1D4A6, 0x1D4A9, 3),
    (0x1D4AA, 0x1D4AC, 1),
    (0x1D4AE, 0x1D4B9, 1),
    (0x1D4BB, 0x1D4BD, 2),
    (0x1D4BE, 0x1D4C3, 1),
    (0x1D4C5, 0x1D505, 1),
    (0x1D507, 0x1D50A, 1),
    (0x1D50D, 0x1D514, 1),
    (0x1D516, 0x1D51C, 1),
    (0x1D51E, 0x1D539, 1),
    (0x1D53B, 0x1D53E, 1),
    (0x1D540, 0x1D544, 1),
    (0x1D546, 0x1D54A, 4),
    (0x1D54B, 0x1D550, 1),
    (0x1D552, 0x1D6A5, 1),
    (0x1D6A8, 0x1D6C0, 1),
    (0x1D6C2, 0x1D6DA, 1),
    (0x1D6DC, 0x1D6FA, 1),
    (0x1D6FC, 0x1D714, 1),
    (0x1D716, 0x1D734, 1),
    (0x1D736, 0x1D74E, 1),
    (0x1D750, 0x1D76E, 1),
    (0x1D770, 0x1D788, 1),
    (0x1D78A, 0x1D7A8, 1),
    (0x1D7AA, 0x1D7C2, 1),
    (0x1D7C4, 0x1D7CB, 1),
    (0x1DF00, 0x1DF1E, 1),
    (0x1DF25, 0x1DF2A, 1),
    (0x1E030, 0x1E06D, 1),
    (0x1E100, 0x1E12C, 1),
    (0x1E137, 0x1E13D, 1),
    (0x1E14E, 0x1E290, 322),
    (0x1E291, 0x1E2AD, 1),
    (0x1E2C0, 0x1E2EB, 1),
    (0x1E4D0, 0x1E4EB, 1),
    (0x1E7E0, 0x1E7E6, 1),
    (0x1E7E8, 0x1E7EB, 1),
    (0x1E7ED, 0x1E7EE, 1),
    (0x1E7F0, 0x1E7FE, 1),
    (0x1E800, 0x1E8C4, 1),
    (0x1E900, 0x1E943, 1),
    (0x1E94B, 0x1EE00, 1205),
    (0x1EE01, 0x1EE03, 1),
    (0x1EE05, 0x1EE1F, 1),
    (0x1EE21, 0x1EE22, 1),
    (0x1EE24, 0x1EE27, 3),
    (0x1EE29, 0x1EE32, 1),
    (0x1EE34, 0x1EE37, 1),
    (0x1EE39, 0x1EE3B, 2),
    (0x1EE42, 0x1EE47, 5),
    (0x1EE49, 0x1EE4D, 2),
    (0x1EE4E, 0x1EE4F, 1),
    (0x1EE51, 0x1EE52, 1),
    (0x1EE54, 0x1EE57, 3),
    (0x1EE59, 0x1EE61, 2),
    (0x1EE62, 0x1EE64, 2),
    (0x1EE67, 0x1EE6A, 1),
    (0x1EE6C, 0x1EE72, 1),
    (0x1EE74, 0x1EE77, 1),
    (0x1EE79, 0x1EE7C, 1),
    (0x1EE7E, 0x1EE80, 2),
    (0x1EE81, 0x1EE89, 1),
    (0x1EE8B, 0x1EE9B, 1),
    (0x1EEA1, 0x1EEA3, 1),
    (0x1EEA5, 0x1EEA9, 1),
    (0x1EEAB, 0x1EEBB, 1),
    (0x20000, 0x2A6DF, 1),
    (0x2A700, 0x2B739, 1),
    (0x2B740, 0x2B81D, 1),
    (0x2B820, 0x2CEA1, 1),
    (0x2CEB0, 0x2EBE0, 1),
    (0x2EBF0, 0x2EE5D, 1),
    (0x2F800, 0x2FA1D, 1),
    (0x30000, 0x3134A, 1),
    (0x31350, 0x323AF, 1),
];

// Go: stringutil/identifier_parts_generated.go unicodeESNextIdentifierPart (Unicode 15.1.0)
static UNICODE_ES_NEXT_IDENTIFIER_PART: &[(u32, u32, u32)] = &[
    (0x30, 0x39, 1),
    (0x41, 0x5A, 1),
    (0x5F, 0x61, 2),
    (0x62, 0x7A, 1),
    (0xAA, 0xB5, 11),
    (0xB7, 0xBA, 3),
    (0xC0, 0xD6, 1),
    (0xD8, 0xF6, 1),
    (0xF8, 0x2C1, 1),
    (0x2C6, 0x2D1, 1),
    (0x2E0, 0x2E4, 1),
    (0x2EC, 0x2EE, 2),
    (0x300, 0x374, 1),
    (0x376, 0x377, 1),
    (0x37A, 0x37D, 1),
    (0x37F, 0x386, 7),
    (0x387, 0x38A, 1),
    (0x38C, 0x38E, 2),
    (0x38F, 0x3A1, 1),
    (0x3A3, 0x3F5, 1),
    (0x3F7, 0x481, 1),
    (0x483, 0x487, 1),
    (0x48A, 0x52F, 1),
    (0x531, 0x556, 1),
    (0x559, 0x560, 7),
    (0x561, 0x588, 1),
    (0x591, 0x5BD, 1),
    (0x5BF, 0x5C1, 2),
    (0x5C2, 0x5C4, 2),
    (0x5C5, 0x5C7, 2),
    (0x5D0, 0x5EA, 1),
    (0x5EF, 0x5F2, 1),
    (0x610, 0x61A, 1),
    (0x620, 0x669, 1),
    (0x66E, 0x6D3, 1),
    (0x6D5, 0x6DC, 1),
    (0x6DF, 0x6E8, 1),
    (0x6EA, 0x6FC, 1),
    (0x6FF, 0x710, 17),
    (0x711, 0x74A, 1),
    (0x74D, 0x7B1, 1),
    (0x7C0, 0x7F5, 1),
    (0x7FA, 0x800, 3),
    (0x801, 0x82D, 1),
    (0x840, 0x85B, 1),
    (0x860, 0x86A, 1),
    (0x870, 0x887, 1),
    (0x889, 0x88E, 1),
    (0x898, 0x8E1, 1),
    (0x8E3, 0x963, 1),
    (0x966, 0x96F, 1),
    (0x971, 0x983, 1),
    (0x985, 0x98C, 1),
    (0x98F, 0x990, 1),
    (0x993, 0x9A8, 1),
    (0x9AA, 0x9B0, 1),
    (0x9B2, 0x9B6, 4),
    (0x9B7, 0x9B9, 1),
    (0x9BC, 0x9C4, 1),
    (0x9C7, 0x9C8, 1),
    (0x9CB, 0x9CE, 1),
    (0x9D7, 0x9DC, 5),
    (0x9DD, 0x9DF, 2),
    (0x9E0, 0x9E3, 1),
    (0x9E6, 0x9F1, 1),
    (0x9FC, 0x9FE, 2),
    (0xA01, 0xA03, 1),
    (0xA05, 0xA0A, 1),
    (0xA0F, 0xA10, 1),
    (0xA13, 0xA28, 1),
    (0xA2A, 0xA30, 1),
    (0xA32, 0xA33, 1),
    (0xA35, 0xA36, 1),
    (0xA38, 0xA39, 1),
    (0xA3C, 0xA3E, 2),
    (0xA3F, 0xA42, 1),
    (0xA47, 0xA48, 1),
    (0xA4B, 0xA4D, 1),
    (0xA51, 0xA59, 8),
    (0xA5A, 0xA5C, 1),
    (0xA5E, 0xA66, 8),
    (0xA67, 0xA75, 1),
    (0xA81, 0xA83, 1),
    (0xA85, 0xA8D, 1),
    (0xA8F, 0xA91, 1),
    (0xA93, 0xAA8, 1),
    (0xAAA, 0xAB0, 1),
    (0xAB2, 0xAB3, 1),
    (0xAB5, 0xAB9, 1),
    (0xABC, 0xAC5, 1),
    (0xAC7, 0xAC9, 1),
    (0xACB, 0xACD, 1),
    (0xAD0, 0xAE0, 16),
    (0xAE1, 0xAE3, 1),
    (0xAE6, 0xAEF, 1),
    (0xAF9, 0xAFF, 1),
    (0xB01, 0xB03, 1),
    (0xB05, 0xB0C, 1),
    (0xB0F, 0xB10, 1),
    (0xB13, 0xB28, 1),
    (0xB2A, 0xB30, 1),
    (0xB32, 0xB33, 1),
    (0xB35, 0xB39, 1),
    (0xB3C, 0xB44, 1),
    (0xB47, 0xB48, 1),
    (0xB4B, 0xB4D, 1),
    (0xB55, 0xB57, 1),
    (0xB5C, 0xB5D, 1),
    (0xB5F, 0xB63, 1),
    (0xB66, 0xB6F, 1),
    (0xB71, 0xB82, 17),
    (0xB83, 0xB85, 2),
    (0xB86, 0xB8A, 1),
    (0xB8E, 0xB90, 1),
    (0xB92, 0xB95, 1),
    (0xB99, 0xB9A, 1),
    (0xB9C, 0xB9E, 2),
    (0xB9F, 0xBA3, 4),
    (0xBA4, 0xBA8, 4),
    (0xBA9, 0xBAA, 1),
    (0xBAE, 0xBB9, 1),
    (0xBBE, 0xBC2, 1),
    (0xBC6, 0xBC8, 1),
    (0xBCA, 0xBCD, 1),
    (0xBD0, 0xBD7, 7),
    (0xBE6, 0xBEF, 1),
    (0xC00, 0xC0C, 1),
    (0xC0E, 0xC10, 1),
    (0xC12, 0xC28, 1),
    (0xC2A, 0xC39, 1),
    (0xC3C, 0xC44, 1),
    (0xC46, 0xC48, 1),
    (0xC4A, 0xC4D, 1),
    (0xC55, 0xC56, 1),
    (0xC58, 0xC5A, 1),
    (0xC5D, 0xC60, 3),
    (0xC61, 0xC63, 1),
    (0xC66, 0xC6F, 1),
    (0xC80, 0xC83, 1),
    (0xC85, 0xC8C, 1),
    (0xC8E, 0xC90, 1),
    (0xC92, 0xCA8, 1),
    (0xCAA, 0xCB3, 1),
    (0xCB5, 0xCB9, 1),
    (0xCBC, 0xCC4, 1),
    (0xCC6, 0xCC8, 1),
    (0xCCA, 0xCCD, 1),
    (0xCD5, 0xCD6, 1),
    (0xCDD, 0xCDE, 1),
    (0xCE0, 0xCE3, 1),
    (0xCE6, 0xCEF, 1),
    (0xCF1, 0xCF3, 1),
    (0xD00, 0xD0C, 1),
    (0xD0E, 0xD10, 1),
    (0xD12, 0xD44, 1),
    (0xD46, 0xD48, 1),
    (0xD4A, 0xD4E, 1),
    (0xD54, 0xD57, 1),
    (0xD5F, 0xD63, 1),
    (0xD66, 0xD6F, 1),
    (0xD7A, 0xD7F, 1),
    (0xD81, 0xD83, 1),
    (0xD85, 0xD96, 1),
    (0xD9A, 0xDB1, 1),
    (0xDB3, 0xDBB, 1),
    (0xDBD, 0xDC0, 3),
    (0xDC1, 0xDC6, 1),
    (0xDCA, 0xDCF, 5),
    (0xDD0, 0xDD4, 1),
    (0xDD6, 0xDD8, 2),
    (0xDD9, 0xDDF, 1),
    (0xDE6, 0xDEF, 1),
    (0xDF2, 0xDF3, 1),
    (0xE01, 0xE3A, 1),
    (0xE40, 0xE4E, 1),
    (0xE50, 0xE59, 1),
    (0xE81, 0xE82, 1),
    (0xE84, 0xE86, 2),
    (0xE87, 0xE8A, 1),
    (0xE8C, 0xEA3, 1),
    (0xEA5, 0xEA7, 2),
    (0xEA8, 0xEBD, 1),
    (0xEC0, 0xEC4, 1),
    (0xEC6, 0xEC8, 2),
    (0xEC9, 0xECE, 1),
    (0xED0, 0xED9, 1),
    (0xEDC, 0xEDF, 1),
    (0xF00, 0xF18, 24),
    (0xF19, 0xF20, 7),
    (0xF21, 0xF29, 1),
    (0xF35, 0xF39, 2),
    (0xF3E, 0xF47, 1),
    (0xF49, 0xF6C, 1),
    (0xF71, 0xF84, 1),
    (0xF86, 0xF97, 1),
    (0xF99, 0xFBC, 1),
    (0xFC6, 0x1000, 58),
    (0x1001, 0x1049, 1),
    (0x1050, 0x109D, 1),
    (0x10A0, 0x10C5, 1),
    (0x10C7, 0x10CD, 6),
    (0x10D0, 0x10FA, 1),
    (0x10FC, 0x1248, 1),
    (0x124A, 0x124D, 1),
    (0x1250, 0x1256, 1),
    (0x1258, 0x125A, 2),
    (0x125B, 0x125D, 1),
    (0x1260, 0x1288, 1),
    (0x128A, 0x128D, 1),
    (0x1290, 0x12B0, 1),
    (0x12B2, 0x12B5, 1),
    (0x12B8, 0x12BE, 1),
    (0x12C0, 0x12C2, 2),
    (0x12C3, 0x12C5, 1),
    (0x12C8, 0x12D6, 1),
    (0x12D8, 0x1310, 1),
    (0x1312, 0x1315, 1),
    (0x1318, 0x135A, 1),
    (0x135D, 0x135F, 1),
    (0x1369, 0x1371, 1),
    (0x1380, 0x138F, 1),
    (0x13A0, 0x13F5, 1),
    (0x13F8, 0x13FD, 1),
    (0x1401, 0x166C, 1),
    (0x166F, 0x167F, 1),
    (0x1681, 0x169A, 1),
    (0x16A0, 0x16EA, 1),
    (0x16EE, 0x16F8, 1),
    (0x1700, 0x1715, 1),
    (0x171F, 0x1734, 1),
    (0x1740, 0x1753, 1),
    (0x1760, 0x176C, 1),
    (0x176E, 0x1770, 1),
    (0x1772, 0x1773, 1),
    (0x1780, 0x17D3, 1),
    (0x17D7, 0x17DC, 5),
    (0x17DD, 0x17E0, 3),
    (0x17E1, 0x17E9, 1),
    (0x180B, 0x180D, 1),
    (0x180F, 0x1819, 1),
    (0x1820, 0x1878, 1),
    (0x1880, 0x18AA, 1),
    (0x18B0, 0x18F5, 1),
    (0x1900, 0x191E, 1),
    (0x1920, 0x192B, 1),
    (0x1930, 0x193B, 1),
    (0x1946, 0x196D, 1),
    (0x1970, 0x1974, 1),
    (0x1980, 0x19AB, 1),
    (0x19B0, 0x19C9, 1),
    (0x19D0, 0x19DA, 1),
    (0x1A00, 0x1A1B, 1),
    (0x1A20, 0x1A5E, 1),
    (0x1A60, 0x1A7C, 1),
    (0x1A7F, 0x1A89, 1),
    (0x1A90, 0x1A99, 1),
    (0x1AA7, 0x1AB0, 9),
    (0x1AB1, 0x1ABD, 1),
    (0x1ABF, 0x1ACE, 1),
    (0x1B00, 0x1B4C, 1),
    (0x1B50, 0x1B59, 1),
    (0x1B6B, 0x1B73, 1),
    (0x1B80, 0x1BF3, 1),
    (0x1C00, 0x1C37, 1),
    (0x1C40, 0x1C49, 1),
    (0x1C4D, 0x1C7D, 1),
    (0x1C80, 0x1C88, 1),
    (0x1C90, 0x1CBA, 1),
    (0x1CBD, 0x1CBF, 1),
    (0x1CD0, 0x1CD2, 1),
    (0x1CD4, 0x1CFA, 1),
    (0x1D00, 0x1F15, 1),
    (0x1F18, 0x1F1D, 1),
    (0x1F20, 0x1F45, 1),
    (0x1F48, 0x1F4D, 1),
    (0x1F50, 0x1F57, 1),
    (0x1F59, 0x1F5F, 2),
    (0x1F60, 0x1F7D, 1),
    (0x1F80, 0x1FB4, 1),
    (0x1FB6, 0x1FBC, 1),
    (0x1FBE, 0x1FC2, 4),
    (0x1FC3, 0x1FC4, 1),
    (0x1FC6, 0x1FCC, 1),
    (0x1FD0, 0x1FD3, 1),
    (0x1FD6, 0x1FDB, 1),
    (0x1FE0, 0x1FEC, 1),
    (0x1FF2, 0x1FF4, 1),
    (0x1FF6, 0x1FFC, 1),
    (0x200C, 0x200D, 1),
    (0x203F, 0x2040, 1),
    (0x2054, 0x2071, 29),
    (0x207F, 0x2090, 17),
    (0x2091, 0x209C, 1),
    (0x20D0, 0x20DC, 1),
    (0x20E1, 0x20E5, 4),
    (0x20E6, 0x20F0, 1),
    (0x2102, 0x2107, 5),
    (0x210A, 0x2113, 1),
    (0x2115, 0x2118, 3),
    (0x2119, 0x211D, 1),
    (0x2124, 0x212A, 2),
    (0x212B, 0x2139, 1),
    (0x213C, 0x213F, 1),
    (0x2145, 0x2149, 1),
    (0x214E, 0x2160, 18),
    (0x2161, 0x2188, 1),
    (0x2C00, 0x2CE4, 1),
    (0x2CEB, 0x2CF3, 1),
    (0x2D00, 0x2D25, 1),
    (0x2D27, 0x2D2D, 6),
    (0x2D30, 0x2D67, 1),
    (0x2D6F, 0x2D7F, 16),
    (0x2D80, 0x2D96, 1),
    (0x2DA0, 0x2DA6, 1),
    (0x2DA8, 0x2DAE, 1),
    (0x2DB0, 0x2DB6, 1),
    (0x2DB8, 0x2DBE, 1),
    (0x2DC0, 0x2DC6, 1),
    (0x2DC8, 0x2DCE, 1),
    (0x2DD0, 0x2DD6, 1),
    (0x2DD8, 0x2DDE, 1),
    (0x2DE0, 0x2DFF, 1),
    (0x3005, 0x3007, 1),
    (0x3021, 0x302F, 1),
    (0x3031, 0x3035, 1),
    (0x3038, 0x303C, 1),
    (0x3041, 0x3096, 1),
    (0x3099, 0x309F, 1),
    (0x30A1, 0x30FF, 1),
    (0x3105, 0x312F, 1),
    (0x3131, 0x318E, 1),
    (0x31A0, 0x31BF, 1),
    (0x31F0, 0x31FF, 1),
    (0x3400, 0x4DBF, 1),
    (0x4E00, 0xA48C, 1),
    (0xA4D0, 0xA4FD, 1),
    (0xA500, 0xA60C, 1),
    (0xA610, 0xA62B, 1),
    (0xA640, 0xA66F, 1),
    (0xA674, 0xA67D, 1),
    (0xA67F, 0xA6F1, 1),
    (0xA717, 0xA71F, 1),
    (0xA722, 0xA788, 1),
    (0xA78B, 0xA7CA, 1),
    (0xA7D0, 0xA7D1, 1),
    (0xA7D3, 0xA7D5, 2),
    (0xA7D6, 0xA7D9, 1),
    (0xA7F2, 0xA827, 1),
    (0xA82C, 0xA840, 20),
    (0xA841, 0xA873, 1),
    (0xA880, 0xA8C5, 1),
    (0xA8D0, 0xA8D9, 1),
    (0xA8E0, 0xA8F7, 1),
    (0xA8FB, 0xA8FD, 2),
    (0xA8FE, 0xA92D, 1),
    (0xA930, 0xA953, 1),
    (0xA960, 0xA97C, 1),
    (0xA980, 0xA9C0, 1),
    (0xA9CF, 0xA9D9, 1),
    (0xA9E0, 0xA9FE, 1),
    (0xAA00, 0xAA36, 1),
    (0xAA40, 0xAA4D, 1),
    (0xAA50, 0xAA59, 1),
    (0xAA60, 0xAA76, 1),
    (0xAA7A, 0xAAC2, 1),
    (0xAADB, 0xAADD, 1),
    (0xAAE0, 0xAAEF, 1),
    (0xAAF2, 0xAAF6, 1),
    (0xAB01, 0xAB06, 1),
    (0xAB09, 0xAB0E, 1),
    (0xAB11, 0xAB16, 1),
    (0xAB20, 0xAB26, 1),
    (0xAB28, 0xAB2E, 1),
    (0xAB30, 0xAB5A, 1),
    (0xAB5C, 0xAB69, 1),
    (0xAB70, 0xABEA, 1),
    (0xABEC, 0xABED, 1),
    (0xABF0, 0xABF9, 1),
    (0xAC00, 0xD7A3, 1),
    (0xD7B0, 0xD7C6, 1),
    (0xD7CB, 0xD7FB, 1),
    (0xF900, 0xFA6D, 1),
    (0xFA70, 0xFAD9, 1),
    (0xFB00, 0xFB06, 1),
    (0xFB13, 0xFB17, 1),
    (0xFB1D, 0xFB28, 1),
    (0xFB2A, 0xFB36, 1),
    (0xFB38, 0xFB3C, 1),
    (0xFB3E, 0xFB40, 2),
    (0xFB41, 0xFB43, 2),
    (0xFB44, 0xFB46, 2),
    (0xFB47, 0xFBB1, 1),
    (0xFBD3, 0xFD3D, 1),
    (0xFD50, 0xFD8F, 1),
    (0xFD92, 0xFDC7, 1),
    (0xFDF0, 0xFDFB, 1),
    (0xFE00, 0xFE0F, 1),
    (0xFE20, 0xFE2F, 1),
    (0xFE33, 0xFE34, 1),
    (0xFE4D, 0xFE4F, 1),
    (0xFE70, 0xFE74, 1),
    (0xFE76, 0xFEFC, 1),
    (0xFF10, 0xFF19, 1),
    (0xFF21, 0xFF3A, 1),
    (0xFF3F, 0xFF41, 2),
    (0xFF42, 0xFF5A, 1),
    (0xFF65, 0xFFBE, 1),
    (0xFFC2, 0xFFC7, 1),
    (0xFFCA, 0xFFCF, 1),
    (0xFFD2, 0xFFD7, 1),
    (0xFFDA, 0xFFDC, 1),
    (0x10000, 0x1000B, 1),
    (0x1000D, 0x10026, 1),
    (0x10028, 0x1003A, 1),
    (0x1003C, 0x1003D, 1),
    (0x1003F, 0x1004D, 1),
    (0x10050, 0x1005D, 1),
    (0x10080, 0x100FA, 1),
    (0x10140, 0x10174, 1),
    (0x101FD, 0x10280, 131),
    (0x10281, 0x1029C, 1),
    (0x102A0, 0x102D0, 1),
    (0x102E0, 0x10300, 32),
    (0x10301, 0x1031F, 1),
    (0x1032D, 0x1034A, 1),
    (0x10350, 0x1037A, 1),
    (0x10380, 0x1039D, 1),
    (0x103A0, 0x103C3, 1),
    (0x103C8, 0x103CF, 1),
    (0x103D1, 0x103D5, 1),
    (0x10400, 0x1049D, 1),
    (0x104A0, 0x104A9, 1),
    (0x104B0, 0x104D3, 1),
    (0x104D8, 0x104FB, 1),
    (0x10500, 0x10527, 1),
    (0x10530, 0x10563, 1),
    (0x10570, 0x1057A, 1),
    (0x1057C, 0x1058A, 1),
    (0x1058C, 0x10592, 1),
    (0x10594, 0x10595, 1),
    (0x10597, 0x105A1, 1),
    (0x105A3, 0x105B1, 1),
    (0x105B3, 0x105B9, 1),
    (0x105BB, 0x105BC, 1),
    (0x10600, 0x10736, 1),
    (0x10740, 0x10755, 1),
    (0x10760, 0x10767, 1),
    (0x10780, 0x10785, 1),
    (0x10787, 0x107B0, 1),
    (0x107B2, 0x107BA, 1),
    (0x10800, 0x10805, 1),
    (0x10808, 0x1080A, 2),
    (0x1080B, 0x10835, 1),
    (0x10837, 0x10838, 1),
    (0x1083C, 0x1083F, 3),
    (0x10840, 0x10855, 1),
    (0x10860, 0x10876, 1),
    (0x10880, 0x1089E, 1),
    (0x108E0, 0x108F2, 1),
    (0x108F4, 0x108F5, 1),
    (0x10900, 0x10915, 1),
    (0x10920, 0x10939, 1),
    (0x10980, 0x109B7, 1),
    (0x109BE, 0x109BF, 1),
    (0x10A00, 0x10A03, 1),
    (0x10A05, 0x10A06, 1),
    (0x10A0C, 0x10A13, 1),
    (0x10A15, 0x10A17, 1),
    (0x10A19, 0x10A35, 1),
    (0x10A38, 0x10A3A, 1),
    (0x10A3F, 0x10A60, 33),
    (0x10A61, 0x10A7C, 1),
    (0x10A80, 0x10A9C, 1),
    (0x10AC0, 0x10AC7, 1),
    (0x10AC9, 0x10AE6, 1),
    (0x10B00, 0x10B35, 1),
    (0x10B40, 0x10B55, 1),
    (0x10B60, 0x10B72, 1),
    (0x10B80, 0x10B91, 1),
    (0x10C00, 0x10C48, 1),
    (0x10C80, 0x10CB2, 1),
    (0x10CC0, 0x10CF2, 1),
    (0x10D00, 0x10D27, 1),
    (0x10D30, 0x10D39, 1),
    (0x10E80, 0x10EA9, 1),
    (0x10EAB, 0x10EAC, 1),
    (0x10EB0, 0x10EB1, 1),
    (0x10EFD, 0x10F1C, 1),
    (0x10F27, 0x10F30, 9),
    (0x10F31, 0x10F50, 1),
    (0x10F70, 0x10F85, 1),
    (0x10FB0, 0x10FC4, 1),
    (0x10FE0, 0x10FF6, 1),
    (0x11000, 0x11046, 1),
    (0x11066, 0x11075, 1),
    (0x1107F, 0x110BA, 1),
    (0x110C2, 0x110D0, 14),
    (0x110D1, 0x110E8, 1),
    (0x110F0, 0x110F9, 1),
    (0x11100, 0x11134, 1),
    (0x11136, 0x1113F, 1),
    (0x11144, 0x11147, 1),
    (0x11150, 0x11173, 1),
    (0x11176, 0x11180, 10),
    (0x11181, 0x111C4, 1),
    (0x111C9, 0x111CC, 1),
    (0x111CE, 0x111DA, 1),
    (0x111DC, 0x11200, 36),
    (0x11201, 0x11211, 1),
    (0x11213, 0x11237, 1),
    (0x1123E, 0x11241, 1),
    (0x11280, 0x11286, 1),
    (0x11288, 0x1128A, 2),
    (0x1128B, 0x1128D, 1),
    (0x1128F, 0x1129D, 1),
    (0x1129F, 0x112A8, 1),
    (0x112B0, 0x112EA, 1),
    (0x112F0, 0x112F9, 1),
    (0x11300, 0x11303, 1),
    (0x11305, 0x1130C, 1),
    (0x1130F, 0x11310, 1),
    (0x11313, 0x11328, 1),
    (0x1132A, 0x11330, 1),
    (0x11332, 0x11333, 1),
    (0x11335, 0x11339, 1),
    (0x1133B, 0x11344, 1),
    (0x11347, 0x11348, 1),
    (0x1134B, 0x1134D, 1),
    (0x11350, 0x11357, 7),
    (0x1135D, 0x11363, 1),
    (0x11366, 0x1136C, 1),
    (0x11370, 0x11374, 1),
    (0x11400, 0x1144A, 1),
    (0x11450, 0x11459, 1),
    (0x1145E, 0x11461, 1),
    (0x11480, 0x114C5, 1),
    (0x114C7, 0x114D0, 9),
    (0x114D1, 0x114D9, 1),
    (0x11580, 0x115B5, 1),
    (0x115B8, 0x115C0, 1),
    (0x115D8, 0x115DD, 1),
    (0x11600, 0x11640, 1),
    (0x11644, 0x11650, 12),
    (0x11651, 0x11659, 1),
    (0x11680, 0x116B8, 1),
    (0x116C0, 0x116C9, 1),
    (0x11700, 0x1171A, 1),
    (0x1171D, 0x1172B, 1),
    (0x11730, 0x11739, 1),
    (0x11740, 0x11746, 1),
    (0x11800, 0x1183A, 1),
    (0x118A0, 0x118E9, 1),
    (0x118FF, 0x11906, 1),
    (0x11909, 0x1190C, 3),
    (0x1190D, 0x11913, 1),
    (0x11915, 0x11916, 1),
    (0x11918, 0x11935, 1),
    (0x11937, 0x11938, 1),
    (0x1193B, 0x11943, 1),
    (0x11950, 0x11959, 1),
    (0x119A0, 0x119A7, 1),
    (0x119AA, 0x119D7, 1),
    (0x119DA, 0x119E1, 1),
    (0x119E3, 0x119E4, 1),
    (0x11A00, 0x11A3E, 1),
    (0x11A47, 0x11A50, 9),
    (0x11A51, 0x11A99, 1),
    (0x11A9D, 0x11AB0, 19),
    (0x11AB1, 0x11AF8, 1),
    (0x11C00, 0x11C08, 1),
    (0x11C0A, 0x11C36, 1),
    (0x11C38, 0x11C40, 1),
    (0x11C50, 0x11C59, 1),
    (0x11C72, 0x11C8F, 1),
    (0x11C92, 0x11CA7, 1),
    (0x11CA9, 0x11CB6, 1),
    (0x11D00, 0x11D06, 1),
    (0x11D08, 0x11D09, 1),
    (0x11D0B, 0x11D36, 1),
    (0x11D3A, 0x11D3C, 2),
    (0x11D3D, 0x11D3F, 2),
    (0x11D40, 0x11D47, 1),
    (0x11D50, 0x11D59, 1),
    (0x11D60, 0x11D65, 1),
    (0x11D67, 0x11D68, 1),
    (0x11D6A, 0x11D8E, 1),
    (0x11D90, 0x11D91, 1),
    (0x11D93, 0x11D98, 1),
    (0x11DA0, 0x11DA9, 1),
    (0x11EE0, 0x11EF6, 1),
    (0x11F00, 0x11F10, 1),
    (0x11F12, 0x11F3A, 1),
    (0x11F3E, 0x11F42, 1),
    (0x11F50, 0x11F59, 1),
    (0x11FB0, 0x12000, 80),
    (0x12001, 0x12399, 1),
    (0x12400, 0x1246E, 1),
    (0x12480, 0x12543, 1),
    (0x12F90, 0x12FF0, 1),
    (0x13000, 0x1342F, 1),
    (0x13440, 0x13455, 1),
    (0x14400, 0x14646, 1),
    (0x16800, 0x16A38, 1),
    (0x16A40, 0x16A5E, 1),
    (0x16A60, 0x16A69, 1),
    (0x16A70, 0x16ABE, 1),
    (0x16AC0, 0x16AC9, 1),
    (0x16AD0, 0x16AED, 1),
    (0x16AF0, 0x16AF4, 1),
    (0x16B00, 0x16B36, 1),
    (0x16B40, 0x16B43, 1),
    (0x16B50, 0x16B59, 1),
    (0x16B63, 0x16B77, 1),
    (0x16B7D, 0x16B8F, 1),
    (0x16E40, 0x16E7F, 1),
    (0x16F00, 0x16F4A, 1),
    (0x16F4F, 0x16F87, 1),
    (0x16F8F, 0x16F9F, 1),
    (0x16FE0, 0x16FE1, 1),
    (0x16FE3, 0x16FE4, 1),
    (0x16FF0, 0x16FF1, 1),
    (0x17000, 0x187F7, 1),
    (0x18800, 0x18CD5, 1),
    (0x18D00, 0x18D08, 1),
    (0x1AFF0, 0x1AFF3, 1),
    (0x1AFF5, 0x1AFFB, 1),
    (0x1AFFD, 0x1AFFE, 1),
    (0x1B000, 0x1B122, 1),
    (0x1B132, 0x1B150, 30),
    (0x1B151, 0x1B152, 1),
    (0x1B155, 0x1B164, 15),
    (0x1B165, 0x1B167, 1),
    (0x1B170, 0x1B2FB, 1),
    (0x1BC00, 0x1BC6A, 1),
    (0x1BC70, 0x1BC7C, 1),
    (0x1BC80, 0x1BC88, 1),
    (0x1BC90, 0x1BC99, 1),
    (0x1BC9D, 0x1BC9E, 1),
    (0x1CF00, 0x1CF2D, 1),
    (0x1CF30, 0x1CF46, 1),
    (0x1D165, 0x1D169, 1),
    (0x1D16D, 0x1D172, 1),
    (0x1D17B, 0x1D182, 1),
    (0x1D185, 0x1D18B, 1),
    (0x1D1AA, 0x1D1AD, 1),
    (0x1D242, 0x1D244, 1),
    (0x1D400, 0x1D454, 1),
    (0x1D456, 0x1D49C, 1),
    (0x1D49E, 0x1D49F, 1),
    (0x1D4A2, 0x1D4A5, 3),
    (0x1D4A6, 0x1D4A9, 3),
    (0x1D4AA, 0x1D4AC, 1),
    (0x1D4AE, 0x1D4B9, 1),
    (0x1D4BB, 0x1D4BD, 2),
    (0x1D4BE, 0x1D4C3, 1),
    (0x1D4C5, 0x1D505, 1),
    (0x1D507, 0x1D50A, 1),
    (0x1D50D, 0x1D514, 1),
    (0x1D516, 0x1D51C, 1),
    (0x1D51E, 0x1D539, 1),
    (0x1D53B, 0x1D53E, 1),
    (0x1D540, 0x1D544, 1),
    (0x1D546, 0x1D54A, 4),
    (0x1D54B, 0x1D550, 1),
    (0x1D552, 0x1D6A5, 1),
    (0x1D6A8, 0x1D6C0, 1),
    (0x1D6C2, 0x1D6DA, 1),
    (0x1D6DC, 0x1D6FA, 1),
    (0x1D6FC, 0x1D714, 1),
    (0x1D716, 0x1D734, 1),
    (0x1D736, 0x1D74E, 1),
    (0x1D750, 0x1D76E, 1),
    (0x1D770, 0x1D788, 1),
    (0x1D78A, 0x1D7A8, 1),
    (0x1D7AA, 0x1D7C2, 1),
    (0x1D7C4, 0x1D7CB, 1),
    (0x1D7CE, 0x1D7FF, 1),
    (0x1DA00, 0x1DA36, 1),
    (0x1DA3B, 0x1DA6C, 1),
    (0x1DA75, 0x1DA84, 15),
    (0x1DA9B, 0x1DA9F, 1),
    (0x1DAA1, 0x1DAAF, 1),
    (0x1DF00, 0x1DF1E, 1),
    (0x1DF25, 0x1DF2A, 1),
    (0x1E000, 0x1E006, 1),
    (0x1E008, 0x1E018, 1),
    (0x1E01B, 0x1E021, 1),
    (0x1E023, 0x1E024, 1),
    (0x1E026, 0x1E02A, 1),
    (0x1E030, 0x1E06D, 1),
    (0x1E08F, 0x1E100, 113),
    (0x1E101, 0x1E12C, 1),
    (0x1E130, 0x1E13D, 1),
    (0x1E140, 0x1E149, 1),
    (0x1E14E, 0x1E290, 322),
    (0x1E291, 0x1E2AE, 1),
    (0x1E2C0, 0x1E2F9, 1),
    (0x1E4D0, 0x1E4F9, 1),
    (0x1E7E0, 0x1E7E6, 1),
    (0x1E7E8, 0x1E7EB, 1),
    (0x1E7ED, 0x1E7EE, 1),
    (0x1E7F0, 0x1E7FE, 1),
    (0x1E800, 0x1E8C4, 1),
    (0x1E8D0, 0x1E8D6, 1),
    (0x1E900, 0x1E94B, 1),
    (0x1E950, 0x1E959, 1),
    (0x1EE00, 0x1EE03, 1),
    (0x1EE05, 0x1EE1F, 1),
    (0x1EE21, 0x1EE22, 1),
    (0x1EE24, 0x1EE27, 3),
    (0x1EE29, 0x1EE32, 1),
    (0x1EE34, 0x1EE37, 1),
    (0x1EE39, 0x1EE3B, 2),
    (0x1EE42, 0x1EE47, 5),
    (0x1EE49, 0x1EE4D, 2),
    (0x1EE4E, 0x1EE4F, 1),
    (0x1EE51, 0x1EE52, 1),
    (0x1EE54, 0x1EE57, 3),
    (0x1EE59, 0x1EE61, 2),
    (0x1EE62, 0x1EE64, 2),
    (0x1EE67, 0x1EE6A, 1),
    (0x1EE6C, 0x1EE72, 1),
    (0x1EE74, 0x1EE77, 1),
    (0x1EE79, 0x1EE7C, 1),
    (0x1EE7E, 0x1EE80, 2),
    (0x1EE81, 0x1EE89, 1),
    (0x1EE8B, 0x1EE9B, 1),
    (0x1EEA1, 0x1EEA3, 1),
    (0x1EEA5, 0x1EEA9, 1),
    (0x1EEAB, 0x1EEBB, 1),
    (0x1FBF0, 0x1FBF9, 1),
    (0x20000, 0x2A6DF, 1),
    (0x2A700, 0x2B739, 1),
    (0x2B740, 0x2B81D, 1),
    (0x2B820, 0x2CEA1, 1),
    (0x2CEB0, 0x2EBE0, 1),
    (0x2EBF0, 0x2EE5D, 1),
    (0x2F800, 0x2FA1D, 1),
    (0x30000, 0x3134A, 1),
    (0x31350, 0x323AF, 1),
    (0xE0100, 0xE01EF, 1),
];

// Go: stringutil/js_case_generated.go unicodeCasedRanges (Unicode 15.1.0)
static UNICODE_CASED_RANGES: &[(u32, u32, u32)] = &[
    (0x41, 0x5A, 1),
    (0x61, 0x7A, 1),
    (0xAA, 0xB5, 11),
    (0xBA, 0xC0, 6),
    (0xC1, 0xD6, 1),
    (0xD8, 0xF6, 1),
    (0xF8, 0x1BA, 1),
    (0x1BC, 0x1BF, 1),
    (0x1C4, 0x293, 1),
    (0x295, 0x2B8, 1),
    (0x2C0, 0x2C1, 1),
    (0x2E0, 0x2E4, 1),
    (0x345, 0x370, 43),
    (0x371, 0x373, 1),
    (0x376, 0x377, 1),
    (0x37A, 0x37D, 1),
    (0x37F, 0x386, 7),
    (0x388, 0x38A, 1),
    (0x38C, 0x38E, 2),
    (0x38F, 0x3A1, 1),
    (0x3A3, 0x3F5, 1),
    (0x3F7, 0x481, 1),
    (0x48A, 0x52F, 1),
    (0x531, 0x556, 1),
    (0x560, 0x588, 1),
    (0x10A0, 0x10C5, 1),
    (0x10C7, 0x10CD, 6),
    (0x10D0, 0x10FA, 1),
    (0x10FC, 0x10FF, 1),
    (0x13A0, 0x13F5, 1),
    (0x13F8, 0x13FD, 1),
    (0x1C80, 0x1C88, 1),
    (0x1C90, 0x1CBA, 1),
    (0x1CBD, 0x1CBF, 1),
    (0x1D00, 0x1DBF, 1),
    (0x1E00, 0x1F15, 1),
    (0x1F18, 0x1F1D, 1),
    (0x1F20, 0x1F45, 1),
    (0x1F48, 0x1F4D, 1),
    (0x1F50, 0x1F57, 1),
    (0x1F59, 0x1F5F, 2),
    (0x1F60, 0x1F7D, 1),
    (0x1F80, 0x1FB4, 1),
    (0x1FB6, 0x1FBC, 1),
    (0x1FBE, 0x1FC2, 4),
    (0x1FC3, 0x1FC4, 1),
    (0x1FC6, 0x1FCC, 1),
    (0x1FD0, 0x1FD3, 1),
    (0x1FD6, 0x1FDB, 1),
    (0x1FE0, 0x1FEC, 1),
    (0x1FF2, 0x1FF4, 1),
    (0x1FF6, 0x1FFC, 1),
    (0x2071, 0x207F, 14),
    (0x2090, 0x209C, 1),
    (0x2102, 0x2107, 5),
    (0x210A, 0x2113, 1),
    (0x2115, 0x2119, 4),
    (0x211A, 0x211D, 1),
    (0x2124, 0x212A, 2),
    (0x212B, 0x212D, 1),
    (0x212F, 0x2134, 1),
    (0x2139, 0x213C, 3),
    (0x213D, 0x213F, 1),
    (0x2145, 0x2149, 1),
    (0x214E, 0x2160, 18),
    (0x2161, 0x217F, 1),
    (0x2183, 0x2184, 1),
    (0x24B6, 0x24E9, 1),
    (0x2C00, 0x2CE4, 1),
    (0x2CEB, 0x2CEE, 1),
    (0x2CF2, 0x2CF3, 1),
    (0x2D00, 0x2D25, 1),
    (0x2D27, 0x2D2D, 6),
    (0xA640, 0xA66D, 1),
    (0xA680, 0xA69D, 1),
    (0xA722, 0xA787, 1),
    (0xA78B, 0xA78E, 1),
    (0xA790, 0xA7CA, 1),
    (0xA7D0, 0xA7D1, 1),
    (0xA7D3, 0xA7D5, 2),
    (0xA7D6, 0xA7D9, 1),
    (0xA7F2, 0xA7F6, 1),
    (0xA7F8, 0xA7FA, 1),
    (0xAB30, 0xAB5A, 1),
    (0xAB5C, 0xAB69, 1),
    (0xAB70, 0xABBF, 1),
    (0xFB00, 0xFB06, 1),
    (0xFB13, 0xFB17, 1),
    (0xFF21, 0xFF3A, 1),
    (0xFF41, 0xFF5A, 1),
    (0x10400, 0x1044F, 1),
    (0x104B0, 0x104D3, 1),
    (0x104D8, 0x104FB, 1),
    (0x10570, 0x1057A, 1),
    (0x1057C, 0x1058A, 1),
    (0x1058C, 0x10592, 1),
    (0x10594, 0x10595, 1),
    (0x10597, 0x105A1, 1),
    (0x105A3, 0x105B1, 1),
    (0x105B3, 0x105B9, 1),
    (0x105BB, 0x105BC, 1),
    (0x10780, 0x10783, 3),
    (0x10784, 0x10785, 1),
    (0x10787, 0x107B0, 1),
    (0x107B2, 0x107BA, 1),
    (0x10C80, 0x10CB2, 1),
    (0x10CC0, 0x10CF2, 1),
    (0x118A0, 0x118DF, 1),
    (0x16E40, 0x16E7F, 1),
    (0x1D400, 0x1D454, 1),
    (0x1D456, 0x1D49C, 1),
    (0x1D49E, 0x1D49F, 1),
    (0x1D4A2, 0x1D4A5, 3),
    (0x1D4A6, 0x1D4A9, 3),
    (0x1D4AA, 0x1D4AC, 1),
    (0x1D4AE, 0x1D4B9, 1),
    (0x1D4BB, 0x1D4BD, 2),
    (0x1D4BE, 0x1D4C3, 1),
    (0x1D4C5, 0x1D505, 1),
    (0x1D507, 0x1D50A, 1),
    (0x1D50D, 0x1D514, 1),
    (0x1D516, 0x1D51C, 1),
    (0x1D51E, 0x1D539, 1),
    (0x1D53B, 0x1D53E, 1),
    (0x1D540, 0x1D544, 1),
    (0x1D546, 0x1D54A, 4),
    (0x1D54B, 0x1D550, 1),
    (0x1D552, 0x1D6A5, 1),
    (0x1D6A8, 0x1D6C0, 1),
    (0x1D6C2, 0x1D6DA, 1),
    (0x1D6DC, 0x1D6FA, 1),
    (0x1D6FC, 0x1D714, 1),
    (0x1D716, 0x1D734, 1),
    (0x1D736, 0x1D74E, 1),
    (0x1D750, 0x1D76E, 1),
    (0x1D770, 0x1D788, 1),
    (0x1D78A, 0x1D7A8, 1),
    (0x1D7AA, 0x1D7C2, 1),
    (0x1D7C4, 0x1D7CB, 1),
    (0x1DF00, 0x1DF09, 1),
    (0x1DF0B, 0x1DF1E, 1),
    (0x1DF25, 0x1DF2A, 1),
    (0x1E030, 0x1E06D, 1),
    (0x1E900, 0x1E943, 1),
    (0x1F130, 0x1F149, 1),
    (0x1F150, 0x1F169, 1),
    (0x1F170, 0x1F189, 1),
];

// Go: stringutil/js_case_generated.go unicodeCaseIgnorableRanges (Unicode 15.1.0)
static UNICODE_CASE_IGNORABLE_RANGES: &[(u32, u32, u32)] = &[
    (0x27, 0x2E, 7),
    (0x3A, 0x5E, 36),
    (0x60, 0xA8, 72),
    (0xAD, 0xAF, 2),
    (0xB4, 0xB7, 3),
    (0xB8, 0x2B0, 504),
    (0x2B1, 0x36F, 1),
    (0x374, 0x375, 1),
    (0x37A, 0x384, 10),
    (0x385, 0x387, 2),
    (0x483, 0x489, 1),
    (0x559, 0x55F, 6),
    (0x591, 0x5BD, 1),
    (0x5BF, 0x5C1, 2),
    (0x5C2, 0x5C4, 2),
    (0x5C5, 0x5C7, 2),
    (0x5F4, 0x600, 12),
    (0x601, 0x605, 1),
    (0x610, 0x61A, 1),
    (0x61C, 0x640, 36),
    (0x64B, 0x65F, 1),
    (0x670, 0x6D6, 102),
    (0x6D7, 0x6DD, 1),
    (0x6DF, 0x6E8, 1),
    (0x6EA, 0x6ED, 1),
    (0x70F, 0x711, 2),
    (0x730, 0x74A, 1),
    (0x7A6, 0x7B0, 1),
    (0x7EB, 0x7F5, 1),
    (0x7FA, 0x7FD, 3),
    (0x816, 0x82D, 1),
    (0x859, 0x85B, 1),
    (0x888, 0x890, 8),
    (0x891, 0x898, 7),
    (0x899, 0x89F, 1),
    (0x8C9, 0x902, 1),
    (0x93A, 0x93C, 2),
    (0x941, 0x948, 1),
    (0x94D, 0x951, 4),
    (0x952, 0x957, 1),
    (0x962, 0x963, 1),
    (0x971, 0x981, 16),
    (0x9BC, 0x9C1, 5),
    (0x9C2, 0x9C4, 1),
    (0x9CD, 0x9E2, 21),
    (0x9E3, 0x9FE, 27),
    (0xA01, 0xA02, 1),
    (0xA3C, 0xA41, 5),
    (0xA42, 0xA47, 5),
    (0xA48, 0xA4B, 3),
    (0xA4C, 0xA4D, 1),
    (0xA51, 0xA70, 31),
    (0xA71, 0xA75, 4),
    (0xA81, 0xA82, 1),
    (0xABC, 0xAC1, 5),
    (0xAC2, 0xAC5, 1),
    (0xAC7, 0xAC8, 1),
    (0xACD, 0xAE2, 21),
    (0xAE3, 0xAFA, 23),
    (0xAFB, 0xAFF, 1),
    (0xB01, 0xB3C, 59),
    (0xB3F, 0xB41, 2),
    (0xB42, 0xB44, 1),
    (0xB4D, 0xB55, 8),
    (0xB56, 0xB62, 12),
    (0xB63, 0xB82, 31),
    (0xBC0, 0xBCD, 13),
    (0xC00, 0xC04, 4),
    (0xC3C, 0xC3E, 2),
    (0xC3F, 0xC40, 1),
    (0xC46, 0xC48, 1),
    (0xC4A, 0xC4D, 1),
    (0xC55, 0xC56, 1),
    (0xC62, 0xC63, 1),
    (0xC81, 0xCBC, 59),
    (0xCBF, 0xCC6, 7),
    (0xCCC, 0xCCD, 1),
    (0xCE2, 0xCE3, 1),
    (0xD00, 0xD01, 1),
    (0xD3B, 0xD3C, 1),
    (0xD41, 0xD44, 1),
    (0xD4D, 0xD62, 21),
    (0xD63, 0xD81, 30),
    (0xDCA, 0xDD2, 8),
    (0xDD3, 0xDD4, 1),
    (0xDD6, 0xE31, 91),
    (0xE34, 0xE3A, 1),
    (0xE46, 0xE4E, 1),
    (0xEB1, 0xEB4, 3),
    (0xEB5, 0xEBC, 1),
    (0xEC6, 0xEC8, 2),
    (0xEC9, 0xECE, 1),
    (0xF18, 0xF19, 1),
    (0xF35, 0xF39, 2),
    (0xF71, 0xF7E, 1),
    (0xF80, 0xF84, 1),
    (0xF86, 0xF87, 1),
    (0xF8D, 0xF97, 1),
    (0xF99, 0xFBC, 1),
    (0xFC6, 0x102D, 103),
    (0x102E, 0x1030, 1),
    (0x1032, 0x1037, 1),
    (0x1039, 0x103A, 1),
    (0x103D, 0x103E, 1),
    (0x1058, 0x1059, 1),
    (0x105E, 0x1060, 1),
    (0x1071, 0x1074, 1),
    (0x1082, 0x1085, 3),
    (0x1086, 0x108D, 7),
    (0x109D, 0x10FC, 95),
    (0x135D, 0x135F, 1),
    (0x1712, 0x1714, 1),
    (0x1732, 0x1733, 1),
    (0x1752, 0x1753, 1),
    (0x1772, 0x1773, 1),
    (0x17B4, 0x17B5, 1),
    (0x17B7, 0x17BD, 1),
    (0x17C6, 0x17C9, 3),
    (0x17CA, 0x17D3, 1),
    (0x17D7, 0x17DD, 6),
    (0x180B, 0x180F, 1),
    (0x1843, 0x1885, 66),
    (0x1886, 0x18A9, 35),
    (0x1920, 0x1922, 1),
    (0x1927, 0x1928, 1),
    (0x1932, 0x1939, 7),
    (0x193A, 0x193B, 1),
    (0x1A17, 0x1A18, 1),
    (0x1A1B, 0x1A56, 59),
    (0x1A58, 0x1A5E, 1),
    (0x1A60, 0x1A62, 2),
    (0x1A65, 0x1A6C, 1),
    (0x1A73, 0x1A7C, 1),
    (0x1A7F, 0x1AA7, 40),
    (0x1AB0, 0x1ACE, 1),
    (0x1B00, 0x1B03, 1),
    (0x1B34, 0x1B36, 2),
    (0x1B37, 0x1B3A, 1),
    (0x1B3C, 0x1B42, 6),
    (0x1B6B, 0x1B73, 1),
    (0x1B80, 0x1B81, 1),
    (0x1BA2, 0x1BA5, 1),
    (0x1BA8, 0x1BA9, 1),
    (0x1BAB, 0x1BAD, 1),
    (0x1BE6, 0x1BE8, 2),
    (0x1BE9, 0x1BED, 4),
    (0x1BEF, 0x1BF1, 1),
    (0x1C2C, 0x1C33, 1),
    (0x1C36, 0x1C37, 1),
    (0x1C78, 0x1C7D, 1),
    (0x1CD0, 0x1CD2, 1),
    (0x1CD4, 0x1CE0, 1),
    (0x1CE2, 0x1CE8, 1),
    (0x1CED, 0x1CF4, 7),
    (0x1CF8, 0x1CF9, 1),
    (0x1D2C, 0x1D6A, 1),
    (0x1D78, 0x1D9B, 35),
    (0x1D9C, 0x1DFF, 1),
    (0x1FBD, 0x1FBF, 2),
    (0x1FC0, 0x1FC1, 1),
    (0x1FCD, 0x1FCF, 1),
    (0x1FDD, 0x1FDF, 1),
    (0x1FED, 0x1FEF, 1),
    (0x1FFD, 0x1FFE, 1),
    (0x200B, 0x200F, 1),
    (0x2018, 0x2019, 1),
    (0x2024, 0x202A, 3),
    (0x202B, 0x202E, 1),
    (0x2060, 0x2064, 1),
    (0x2066, 0x206F, 1),
    (0x2071, 0x207F, 14),
    (0x2090, 0x209C, 1),
    (0x20D0, 0x20F0, 1),
    (0x2C7C, 0x2C7D, 1),
    (0x2CEF, 0x2CF1, 1),
    (0x2D6F, 0x2D7F, 16),
    (0x2DE0, 0x2DFF, 1),
    (0x2E2F, 0x3005, 470),
    (0x302A, 0x302D, 1),
    (0x3031, 0x3035, 1),
    (0x303B, 0x3099, 94),
    (0x309A, 0x309E, 1),
    (0x30FC, 0x30FE, 1),
    (0xA015, 0xA4F8, 1251),
    (0xA4F9, 0xA4FD, 1),
    (0xA60C, 0xA66F, 99),
    (0xA670, 0xA672, 1),
    (0xA674, 0xA67D, 1),
    (0xA67F, 0xA69C, 29),
    (0xA69D, 0xA69F, 1),
    (0xA6F0, 0xA6F1, 1),
    (0xA700, 0xA721, 1),
    (0xA770, 0xA788, 24),
    (0xA789, 0xA78A, 1),
    (0xA7F2, 0xA7F4, 1),
    (0xA7F8, 0xA7F9, 1),
    (0xA802, 0xA806, 4),
    (0xA80B, 0xA825, 26),
    (0xA826, 0xA82C, 6),
    (0xA8C4, 0xA8C5, 1),
    (0xA8E0, 0xA8F1, 1),
    (0xA8FF, 0xA926, 39),
    (0xA927, 0xA92D, 1),
    (0xA947, 0xA951, 1),
    (0xA980, 0xA982, 1),
    (0xA9B3, 0xA9B6, 3),
    (0xA9B7, 0xA9B9, 1),
    (0xA9BC, 0xA9BD, 1),
    (0xA9CF, 0xA9E5, 22),
    (0xA9E6, 0xAA29, 67),
    (0xAA2A, 0xAA2E, 1),
    (0xAA31, 0xAA32, 1),
    (0xAA35, 0xAA36, 1),
    (0xAA43, 0xAA4C, 9),
    (0xAA70, 0xAA7C, 12),
    (0xAAB0, 0xAAB2, 2),
    (0xAAB3, 0xAAB4, 1),
    (0xAAB7, 0xAAB8, 1),
    (0xAABE, 0xAABF, 1),
    (0xAAC1, 0xAADD, 28),
    (0xAAEC, 0xAAED, 1),
    (0xAAF3, 0xAAF4, 1),
    (0xAAF6, 0xAB5B, 101),
    (0xAB5C, 0xAB5F, 1),
    (0xAB69, 0xAB6B, 1),
    (0xABE5, 0xABE8, 3),
    (0xABED, 0xFB1E, 20273),
    (0xFBB2, 0xFBC2, 1),
    (0xFE00, 0xFE0F, 1),
    (0xFE13, 0xFE20, 13),
    (0xFE21, 0xFE2F, 1),
    (0xFE52, 0xFE55, 3),
    (0xFEFF, 0xFF07, 8),
    (0xFF0E, 0xFF1A, 12),
    (0xFF3E, 0xFF40, 2),
    (0xFF70, 0xFF9E, 46),
    (0xFF9F, 0xFFE3, 68),
    (0xFFF9, 0xFFFB, 1),
    (0x101FD, 0x102E0, 227),
    (0x10376, 0x1037A, 1),
    (0x10780, 0x10785, 1),
    (0x10787, 0x107B0, 1),
    (0x107B2, 0x107BA, 1),
    (0x10A01, 0x10A03, 1),
    (0x10A05, 0x10A06, 1),
    (0x10A0C, 0x10A0F, 1),
    (0x10A38, 0x10A3A, 1),
    (0x10A3F, 0x10AE5, 166),
    (0x10AE6, 0x10D24, 574),
    (0x10D25, 0x10D27, 1),
    (0x10EAB, 0x10EAC, 1),
    (0x10EFD, 0x10EFF, 1),
    (0x10F46, 0x10F50, 1),
    (0x10F82, 0x10F85, 1),
    (0x11001, 0x11038, 55),
    (0x11039, 0x11046, 1),
    (0x11070, 0x11073, 3),
    (0x11074, 0x1107F, 11),
    (0x11080, 0x11081, 1),
    (0x110B3, 0x110B6, 1),
    (0x110B9, 0x110BA, 1),
    (0x110BD, 0x110C2, 5),
    (0x110CD, 0x11100, 51),
    (0x11101, 0x11102, 1),
    (0x11127, 0x1112B, 1),
    (0x1112D, 0x11134, 1),
    (0x11173, 0x11180, 13),
    (0x11181, 0x111B6, 53),
    (0x111B7, 0x111BE, 1),
    (0x111C9, 0x111CC, 1),
    (0x111CF, 0x1122F, 96),
    (0x11230, 0x11231, 1),
    (0x11234, 0x11236, 2),
    (0x11237, 0x1123E, 7),
    (0x11241, 0x112DF, 158),
    (0x112E3, 0x112EA, 1),
    (0x11300, 0x11301, 1),
    (0x1133B, 0x1133C, 1),
    (0x11340, 0x11366, 38),
    (0x11367, 0x1136C, 1),
    (0x11370, 0x11374, 1),
    (0x11438, 0x1143F, 1),
    (0x11442, 0x11444, 1),
    (0x11446, 0x1145E, 24),
    (0x114B3, 0x114B8, 1),
    (0x114BA, 0x114BF, 5),
    (0x114C0, 0x114C2, 2),
    (0x114C3, 0x115B2, 239),
    (0x115B3, 0x115B5, 1),
    (0x115BC, 0x115BD, 1),
    (0x115BF, 0x115C0, 1),
    (0x115DC, 0x115DD, 1),
    (0x11633, 0x1163A, 1),
    (0x1163D, 0x1163F, 2),
    (0x11640, 0x116AB, 107),
    (0x116AD, 0x116B0, 3),
    (0x116B1, 0x116B5, 1),
    (0x116B7, 0x1171D, 102),
    (0x1171E, 0x1171F, 1),
    (0x11722, 0x11725, 1),
    (0x11727, 0x1172B, 1),
    (0x1182F, 0x11837, 1),
    (0x11839, 0x1183A, 1),
    (0x1193B, 0x1193C, 1),
    (0x1193E, 0x11943, 5),
    (0x119D4, 0x119D7, 1),
    (0x119DA, 0x119DB, 1),
    (0x119E0, 0x11A01, 33),
    (0x11A02, 0x11A0A, 1),
    (0x11A33, 0x11A38, 1),
    (0x11A3B, 0x11A3E, 1),
    (0x11A47, 0x11A51, 10),
    (0x11A52, 0x11A56, 1),
    (0x11A59, 0x11A5B, 1),
    (0x11A8A, 0x11A96, 1),
    (0x11A98, 0x11A99, 1),
    (0x11C30, 0x11C36, 1),
    (0x11C38, 0x11C3D, 1),
    (0x11C3F, 0x11C92, 83),
    (0x11C93, 0x11CA7, 1),
    (0x11CAA, 0x11CB0, 1),
    (0x11CB2, 0x11CB3, 1),
    (0x11CB5, 0x11CB6, 1),
    (0x11D31, 0x11D36, 1),
    (0x11D3A, 0x11D3C, 2),
    (0x11D3D, 0x11D3F, 2),
    (0x11D40, 0x11D45, 1),
    (0x11D47, 0x11D90, 73),
    (0x11D91, 0x11D95, 4),
    (0x11D97, 0x11EF3, 348),
    (0x11EF4, 0x11F00, 12),
    (0x11F01, 0x11F36, 53),
    (0x11F37, 0x11F3A, 1),
    (0x11F40, 0x11F42, 2),
    (0x13430, 0x13440, 1),
    (0x13447, 0x13455, 1),
    (0x16AF0, 0x16AF4, 1),
    (0x16B30, 0x16B36, 1),
    (0x16B40, 0x16B43, 1),
    (0x16F4F, 0x16F8F, 64),
    (0x16F90, 0x16F9F, 1),
    (0x16FE0, 0x16FE1, 1),
    (0x16FE3, 0x16FE4, 1),
    (0x1AFF0, 0x1AFF3, 1),
    (0x1AFF5, 0x1AFFB, 1),
    (0x1AFFD, 0x1AFFE, 1),
    (0x1BC9D, 0x1BC9E, 1),
    (0x1BCA0, 0x1BCA3, 1),
    (0x1CF00, 0x1CF2D, 1),
    (0x1CF30, 0x1CF46, 1),
    (0x1D167, 0x1D169, 1),
    (0x1D173, 0x1D182, 1),
    (0x1D185, 0x1D18B, 1),
    (0x1D1AA, 0x1D1AD, 1),
    (0x1D242, 0x1D244, 1),
    (0x1DA00, 0x1DA36, 1),
    (0x1DA3B, 0x1DA6C, 1),
    (0x1DA75, 0x1DA84, 15),
    (0x1DA9B, 0x1DA9F, 1),
    (0x1DAA1, 0x1DAAF, 1),
    (0x1E000, 0x1E006, 1),
    (0x1E008, 0x1E018, 1),
    (0x1E01B, 0x1E021, 1),
    (0x1E023, 0x1E024, 1),
    (0x1E026, 0x1E02A, 1),
    (0x1E030, 0x1E06D, 1),
    (0x1E08F, 0x1E130, 161),
    (0x1E131, 0x1E13D, 1),
    (0x1E2AE, 0x1E2EC, 62),
    (0x1E2ED, 0x1E2EF, 1),
    (0x1E4EB, 0x1E4EF, 1),
    (0x1E8D0, 0x1E8D6, 1),
    (0x1E944, 0x1E94B, 1),
    (0x1F3FB, 0x1F3FF, 1),
    (0xE0001, 0xE0020, 31),
    (0xE0021, 0xE007F, 1),
    (0xE0100, 0xE01EF, 1),
];

// Go: scanner/scanner_test.go tests of the private JSDoc type text helpers (tsgo#4839).
#[cfg(test)]
mod scanner_test;

#[cfg(test)]
mod tests {
    use super::{
        GoUnit, combine_surrogate_pairs, compare_go_strings, compute_ecma_line_starts,
        contains_go_string_marker, decode_js_string_rune, encode_js_string_rune,
        fuse_surrogate_bytes, go_byte_offset, go_has_suffix, go_len, go_map_runes, go_runes,
        go_slice, go_string_bytes, go_string_from_bytes, go_string_from_utf8, go_to_valid_utf8,
        go_unit_at, go_unit_before, go_unit_cut_at, go_value, go_value_from_bytes, is_line_break,
        port_byte_offset, utf16_len, utf16_len_of_range,
    };
    use super::{
        LevenshteinBuffers, get_spelling_suggestion_for_strings,
        get_spelling_suggestion_with_max_candidate_count, levenshtein_with_max, unicode_to_lower,
    };

    /// Go (WTF-8) bytes of a rune, as Go `EncodeJSStringRune` writes it.
    fn go_bytes(ch: u32) -> Vec<u8> {
        if (0xD800..0xE000).contains(&ch) {
            return vec![
                0xED,
                0x80 | ((ch >> 6) & 0x3F) as u8,
                0x80 | (ch & 0x3F) as u8,
            ];
        }
        char::from_u32(ch).unwrap().to_string().into_bytes()
    }

    // Runes near the lone surrogate escape form (see GO_STRING_MARKER).
    const RUNES: [u32; 10] = [
        0x61, 0xD800, 0xDBFF, 0xDC00, 0xDFFF, 0xFDD0, 0xFFFD, 0x10F800, 0x10FFFF, 0x1F600,
    ];

    #[test]
    fn js_string_runes_round_trip_and_sort_as_go() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut values: Vec<(String, Vec<u32>)> = Vec::new();
        for _ in 0..2_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut bits = seed;
            let mut runes = Vec::new();
            for _ in 0..(bits % 5) {
                bits = bits.rotate_right(7) ^ seed;
                runes.push(RUNES[(bits % RUNES.len() as u64) as usize]);
            }
            let text: String = runes.iter().map(|&r| encode_js_string_rune(r)).collect();
            // Each value decodes back to the same runes.
            let mut decoded = Vec::new();
            let mut i = 0;
            while i < text.len() {
                let (r, size) = decode_js_string_rune(&text[i..]);
                decoded.push(r);
                i += size as usize;
            }
            assert_eq!(decoded, runes, "{text:?}");
            values.push((text, runes));
        }
        for pair in values.windows(2) {
            let (a, ra) = &pair[0];
            let (b, rb) = &pair[1];
            let ga: Vec<u8> = ra.iter().flat_map(|&r| go_bytes(r)).collect();
            let gb: Vec<u8> = rb.iter().flat_map(|&r| go_bytes(r)).collect();
            assert_eq!(compare_go_strings(a, b), ga.cmp(&gb), "{a:?} {b:?}");
        }
    }

    /// Long text (the `memchr` scan alone) and short text (core's
    /// `contains` first) give the same answer, also when the lead byte 0xEF
    /// starts a char that is not the marker.
    #[test]
    fn go_string_marker_found_in_long_and_short_text() {
        let lone = encode_js_string_rune(0xD800);
        for len in [0, 1, 254, 255, 256, 257, 300, 4096] {
            let plain = "a".repeat(len) + "\u{F000}";
            assert!(!contains_go_string_marker(&plain), "{len}");
            assert_eq!(go_value(&plain), plain, "{len}");
            let marked = plain.clone() + &lone;
            assert!(contains_go_string_marker(&marked), "{len}");
            assert!(contains_go_string_marker(&(lone.clone() + &plain)), "{len}");
        }
    }

    #[test]
    fn combine_surrogate_pairs_keeps_real_plane_16() {
        let pair = encode_js_string_rune(0xDBFF) + &encode_js_string_rune(0xDFFF);
        assert_eq!(combine_surrogate_pairs(&pair), "\u{10FFFF}");
        let real = encode_js_string_rune(0x10FFFF) + &encode_js_string_rune(0xFDD0);
        assert_eq!(decode_js_string_rune(&real), (0x10FFFF, 4));
        assert_eq!(combine_surrogate_pairs(&real), real);
    }

    /// Pieces of Go source bytes near the port form escapes (see
    /// GO_STRING_MARKER): chars that look like units, invalid bytes, a
    /// truncated sequence and the WTF-8 bytes of a surrogate.
    const GO_PIECES: [&[u8]; 16] = [
        b"a",
        "\u{EF80}".as_bytes(),
        "\u{EFFF}".as_bytes(),
        "\u{FDD0}".as_bytes(),
        "\u{FFFE}".as_bytes(),
        b"\xFE",
        "\u{10F780}".as_bytes(),
        "\u{10F7FF}".as_bytes(),
        "\u{10FFFF}".as_bytes(),
        "\u{FFFD}".as_bytes(),
        "\u{1F600}".as_bytes(),
        b"\x80",
        b"\xFF",
        b"\xE2\x82",
        b"\xED\xA0\x80",
        b"\xED\xBF\xBF",
    ];

    /// Go `core.UTF16Len` on Go bytes: each byte that is not valid UTF-8 is
    /// one RuneError, one UTF-16 unit.
    fn go_utf16_len(bytes: &[u8]) -> i32 {
        bytes
            .utf8_chunks()
            .map(|chunk| chunk.valid().encode_utf16().count() + chunk.invalid().len())
            .sum::<usize>() as i32
    }

    fn random_go_bytes(seed: &mut u64) -> Vec<u8> {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        let mut bits = *seed;
        let mut out = Vec::new();
        for _ in 0..(bits % 7) {
            bits = bits.rotate_right(5) ^ *seed;
            out.extend_from_slice(GO_PIECES[(bits % GO_PIECES.len() as u64) as usize]);
        }
        out
    }

    // Every Go byte string has one port form that gives the same bytes back,
    // counts the same UTF-16 units and offsets, and sorts as Go.
    #[test]
    fn go_string_port_form_is_injective() {
        let mut seed = 0x51_7CC1_B727_220Au64;
        let mut values: Vec<(Vec<u8>, String)> = Vec::new();
        for _ in 0..4_000 {
            let bytes = random_go_bytes(&mut seed);
            let text = go_string_from_bytes(bytes.clone());
            assert_eq!(&*go_string_bytes(&text), &bytes[..], "{text:?}");
            assert_eq!(utf16_len(&text), go_utf16_len(&bytes), "{text:?}");
            assert_eq!(go_byte_offset(&text, text.len() as i32), bytes.len() as i32);
            // Walk the units forward, check offsets and the backward read.
            let mut boundaries = vec![0usize];
            let mut i = 0;
            while i < text.len() {
                let (unit, size) = go_unit_at(&text, i);
                i += size;
                assert_eq!(go_unit_before(&text, i), (unit, size), "{text:?} at {i}");
                boundaries.push(i);
            }
            for &at in &boundaries {
                let go = go_byte_offset(&text, at as i32);
                assert_eq!(port_byte_offset(&text, go), at as i32, "{text:?} at {at}");
                assert_eq!(&*go_string_bytes(&text[..at]), &bytes[..go as usize]);
            }
            // An offset `k` bytes into a unit of `g` Go bytes is Go offset
            // `min(k, g)` in the unit, and each Go offset maps back.
            for pos in 0..=text.len() {
                let unit_start = *boundaries.iter().rfind(|&&b| b <= pos).unwrap();
                let base = go_byte_offset(&text, unit_start as i32) as usize;
                let cut = go_unit_cut_at(&text, pos);
                let go = if unit_start == pos {
                    assert_eq!(cut, None, "{text:?} at {pos}");
                    base
                } else {
                    let (unit, size) = go_unit_at(&text, unit_start);
                    assert_eq!(cut, Some((unit_start, unit, size)), "{text:?} at {pos}");
                    base + (pos - unit_start).min(unit.go_len())
                };
                assert_eq!(
                    go_byte_offset(&text, pos as i32),
                    go as i32,
                    "{text:?} at {pos}"
                );
            }
            for go in 0..=bytes.len() as i32 {
                let pos = port_byte_offset(&text, go);
                assert_eq!(go_byte_offset(&text, pos), go, "{text:?} at Go {go}");
            }
            values.push((bytes, text));
        }
        for pair in values.windows(2) {
            let (ga, a) = &pair[0];
            let (gb, b) = &pair[1];
            assert_eq!(compare_go_strings(a, b), ga.cmp(gb), "{a:?} {b:?}");
            assert_eq!(a == b, ga == gb, "{a:?} {b:?}");
        }
    }

    /// `utf16_len_of_range` is Go `core.UTF16Len(s[start:end])` when the
    /// range cuts a char. The 9 slices and their Go counts are from the Go
    /// model of the sweepN2 11133 panic (`utf16cut/main.go`, Go 1.27.1, the
    /// pin N `UTF16Len`): the source map line of `parserSkippedTokens16.ts`
    /// with its 2-byte char, and cuts of a 3-byte and a 4-byte char. Each
    /// other range of the two texts must also count as Go counts its bytes.
    #[test]
    fn utf16_len_of_range_counts_cut_chars_as_go() {
        let line = "function Foo      () \u{AC}   { }";
        let text = "x\u{20AC}y\u{1F600}z";
        let go_counts = [
            (line, 0, 22, 22),
            (line, 21, 22, 1),
            (line, 22, 25, 3),
            (line, 0, 25, 24),
            (text, 0, 3, 3),
            (text, 2, 4, 2),
            (text, 0, 7, 5),
            (text, 5, 9, 2),
            (text, 3, 6, 3),
        ];
        for (s, start, end, go) in go_counts {
            assert_eq!(
                utf16_len_of_range(s, start, end),
                go,
                "{s:?}[{start}:{end}]"
            );
        }
        for s in [line, text] {
            for start in 0..=s.len() {
                for end in start..=s.len() {
                    let go = go_utf16_len(&s.as_bytes()[start..end]);
                    assert_eq!(
                        utf16_len_of_range(s, start, end),
                        go,
                        "{s:?}[{start}:{end}]"
                    );
                }
            }
        }
        // Port forms with marker units: a range edge in a unit is Go offset
        // `go_byte_offset` there. The texts are the srcmap1 p3 repros
        // (`switch` with an invalid byte, a WTF-8 surrogate or U+FDD0 where
        // the printer writes a token) and random Go bytes.
        let mut texts: Vec<Vec<u8>> = vec![
            b"switch\xAC (e) {".to_vec(),
            b"switch\xED\xA0\x80 (e) {".to_vec(),
            "switch\u{FDD0} (e) { case 1: }".as_bytes().to_vec(),
            "\u{FDD0}\u{FDD0}\u{FDD0}\u{10F780}\u{FDD0}\u{10F780}"
                .as_bytes()
                .to_vec(),
        ];
        let mut seed = 0x2B99_D1E0_5A73_C4F1u64;
        texts.extend((0..1_000).map(|_| random_go_bytes(&mut seed)));
        for bytes in texts {
            let s = go_string_from_bytes(bytes.clone());
            for start in 0..=s.len() {
                let go_start = go_byte_offset(&s, start as i32) as usize;
                for end in start..=s.len() + 1 {
                    let go_end = go_byte_offset(&s, end as i32) as usize;
                    let go = go_utf16_len(&bytes[go_start..go_end.min(bytes.len())])
                        + go_end.saturating_sub(bytes.len()) as i32;
                    assert_eq!(
                        utf16_len_of_range(&s, start, end),
                        go,
                        "{s:?}[{start}:{end}]"
                    );
                }
            }
        }
        // The p3 repro split, as the printer cache counts it: `[6:7]` then
        // `[7:]` against Go `[6:7]` then `[7:]` (srcmap1 skeptic: Go source
        // column 13 on l1, tsgo 17).
        let s = go_string_from_bytes(b"switch\xAC (e) {".to_vec());
        assert_eq!(
            utf16_len_of_range(&s, 0, 7) + utf16_len_of_range(&s, 7, s.len()),
            go_utf16_len(b"switch\xAC (e) {")
        );
    }

    // A WTF-8 surrogate in source text is 3 invalid byte units. A string
    // value fuses them into the lone surrogate unit that an escape gives,
    // and `decode_js_string_rune` reads both as the surrogate, as Go.
    #[test]
    fn surrogate_bytes_fuse_into_one_unit() {
        let source = go_string_from_bytes(b"a\xED\xA0\x80\xED\xB0\x80\xEDb".to_vec());
        let (unit, _) = go_unit_at(&source, 1);
        assert_eq!(unit, GoUnit::InvalidByte(0xED));
        assert_eq!(decode_js_string_rune(&source[1..]).0, 0xD800);
        let value = fuse_surrogate_bytes(&source);
        let escaped = format!(
            "a{}{}{}b",
            encode_js_string_rune(0xD800),
            encode_js_string_rune(0xDC00),
            &source[source.len() - 8..source.len() - 1]
        );
        assert_eq!(value, escaped);
        assert_eq!(go_string_bytes(&value), go_string_bytes(&source));
        assert_eq!(
            combine_surrogate_pairs(&value),
            format!("a\u{10000}{}b", &source[source.len() - 8..source.len() - 1])
        );
        assert!(matches!(
            fuse_surrogate_bytes("a\u{FDD0}\u{FDD0}"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    // Go `strings.ToValidUTF8` replaces each run of invalid bytes once and
    // keeps real U+FFFD and U+FDD0 chars.
    #[test]
    fn to_valid_utf8_replaces_runs() {
        let text = go_string_from_bytes(
            "a\u{FFFD}\u{FDD0}"
                .bytes()
                .chain(*b"\xFF\xED\xA0\x80b\x80")
                .collect(),
        );
        assert_eq!(
            go_to_valid_utf8(&text),
            "a\u{FFFD}\u{FDD0}\u{FDD0}\u{FFFD}b\u{FFFD}"
        );
        let fused = fuse_surrogate_bytes(&text);
        assert_eq!(go_to_valid_utf8(&fused), go_to_valid_utf8(&text));
    }

    // Go `strings.Map` passes each invalid byte as U+FFFD and writes the
    // result. A real U+FDD0 keeps its port form.
    #[test]
    fn map_runes_maps_invalid_bytes() {
        let text = go_string_from_bytes("A\u{FDD0}".bytes().chain(*b"\x80\xED\xA0\x80").collect());
        let lower = |ch: char| ch.to_ascii_lowercase();
        assert_eq!(
            go_map_runes(&text, lower),
            "a\u{FDD0}\u{FDD0}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"
        );
        let fused = fuse_surrogate_bytes(&text);
        assert_eq!(go_map_runes(&fused, lower), go_map_runes(&text, lower));
    }

    // Go joins, searches and slices the bytes of strings. The Go byte helpers
    // do the same on port forms, and joins give the one value form.
    #[test]
    fn go_byte_helpers_follow_go_bytes() {
        let value = |bytes: &[u8]| go_value_from_bytes(bytes).into_owned();
        // A join of invalid bytes can be a valid char or a lone surrogate.
        let joined = format!("{}{}", value(b"\xC3"), value(b"\xA9"));
        assert_eq!(go_value(&joined), "\u{E9}");
        let joined = format!("{}{}", value(b"\xED"), value(b"\xA0\x80"));
        assert_eq!(go_value(&joined), encode_js_string_rune(0xD800));
        // A search never matches inside a unit, and can match inside a char.
        let a80 = value(b"a\x80");
        assert!(!go_has_suffix(&a80, "\u{10F780}"));
        assert!(go_has_suffix(&a80, &value(b"\x80")));
        assert!(go_has_suffix("\u{E9}", &value(b"\xA9")));
        assert_eq!(go_slice("\u{E9}", 0, 1), value(b"\xC3"));
        // Go `len` and `[]rune`.
        let fdd0 = value("\u{FDD0}".as_bytes());
        assert_eq!((go_len(&a80), go_len(&fdd0)), (2, 3));
        assert_eq!(go_runes(&a80), ['a', '\u{FFFD}']);
        assert_eq!(go_runes(&fdd0), ['\u{FDD0}']);
    }

    // Go's internal symbol name prefix is the byte 0xFE. Its port form is
    // the invalid byte unit, so source text with that byte names the same
    // symbols as in Go, and a real U+FFFE stays an ordinary char.
    #[test]
    fn internal_symbol_name_prefix_is_byte_fe() {
        let prefix = crate::ast::INTERNAL_SYMBOL_NAME_PREFIX;
        assert_eq!(go_string_from_bytes(b"\xFE".to_vec()), prefix);
        assert_eq!(
            go_unit_at(prefix, 0),
            (GoUnit::InvalidByte(0xFE), prefix.len())
        );
        assert_eq!(go_runes("\u{FFFE}"), ['\u{FFFE}']);
        // Go replaces each byte 0xFE, and a U+FDD0 unit before a real
        // U+10F7FE char holds no such byte.
        let escape = crate::ast::escape_all_internal_symbol_names;
        assert_eq!(escape(&format!("{prefix}type{prefix}")), "__type__");
        let fdd0_then_char = go_string_from_utf8("\u{FDD0}\u{10F7FE}".to_string());
        assert_eq!(escape(&fdd0_then_char), fdd0_then_char);
    }

    /// `compute_ecma_line_starts` written as a plain char scan.
    fn line_starts_by_char(text: &str) -> Vec<i32> {
        let mut starts = vec![0];
        let mut chars = text.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            if !is_line_break(ch) {
                continue;
            }
            let crlf = ch == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some();
            starts.push((i + ch.len_utf8() + usize::from(crlf)) as i32);
        }
        starts
    }

    #[test]
    fn ecma_line_starts_match_char_scan() {
        const PIECES: [&str; 8] = [
            "a",
            " ",
            "\n",
            "\r",
            "\r\n",
            "\u{2028}",
            "\u{2029}",
            "é\u{2000}",
        ];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            let mut text = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let mut bits = seed;
            for _ in 0..(bits % 20) {
                bits = bits.rotate_right(3) ^ seed;
                text.push_str(PIECES[(bits % 8) as usize]);
            }
            assert_eq!(
                compute_ecma_line_starts(&text),
                line_starts_by_char(&text),
                "{text:?}"
            );
        }
    }

    /// One buffer pair used for many pairs, longer then shorter, gives the
    /// distances that new buffers give.
    #[test]
    fn levenshtein_reused_buffers_match_new_buffers() {
        let words = [
            "getSpellingSuggestion",
            "getSpelingSugestion",
            "GETSPELLINGSUGGESTION",
            "spelling",
            "Spellings",
            "x",
            "",
            "\u{c9}t\u{e9}",
            "\u{e9}t\u{c9}s",
        ];
        let mut reused = LevenshteinBuffers::default();
        for a in words {
            for b in words {
                let a: Vec<char> = a.chars().collect();
                let b: Vec<char> = b.chars().collect();
                for max_value in [0.9, 2.9, 8.9, 30.0] {
                    let fresh =
                        levenshtein_with_max(&mut LevenshteinBuffers::default(), &a, &b, max_value);
                    let again = levenshtein_with_max(&mut reused, &a, &b, max_value);
                    assert_eq!(fresh.to_bits(), again.to_bits(), "{a:?} {b:?} {max_value}");
                }
            }
        }
    }

    /// Go core.go:590 getSpellingSuggestion as written, with
    /// `strings.Compare`: a new rune slice and new Levenshtein buffers for
    /// each candidate. Only for valid UTF-8, where Go's `len` is `str::len`
    /// and `[]rune` is `chars`.
    fn go_get_spelling_suggestion(name: &str, candidates: &[&str], max_candidates: i32) -> String {
        let rune_name: Vec<char> = name.chars().collect();
        let maximum_length_difference = 2.max((rune_name.len() as f64 * 0.34) as usize);
        let mut best_distance = (rune_name.len() as f64 * 0.4).floor() + 0.9;
        let mut best_candidate = String::new();
        let mut has_best = false;
        let mut checked_candidates = 0;
        for &candidate_name in candidates {
            checked_candidates += 1;
            if max_candidates > 0 && checked_candidates > max_candidates {
                return String::new();
            }
            let max_len = candidate_name.len().max(rune_name.len());
            let min_len = candidate_name.len().min(rune_name.len());
            if !candidate_name.is_empty() && max_len - min_len <= maximum_length_difference {
                if candidate_name == name {
                    continue;
                }
                if candidate_name.len() < 3 && !go_equal_fold(candidate_name, name) {
                    continue;
                }
                let candidate_runes: Vec<char> = candidate_name.chars().collect();
                let distance = go_levenshtein_with_max(&rune_name, &candidate_runes, best_distance);
                if distance < 0.0 {
                    continue;
                }
                if distance < best_distance {
                    best_distance = distance;
                    best_candidate = candidate_name.to_string();
                    has_best = true;
                } else if !has_best || candidate_name < best_candidate.as_str() {
                    best_candidate = candidate_name.to_string();
                    has_best = true;
                }
            }
        }
        best_candidate
    }

    /// Go `strings.EqualFold` for the words of the test (no rune with a
    /// special fold).
    fn go_equal_fold(a: &str, b: &str) -> bool {
        a.chars().count() == b.chars().count()
            && a.chars()
                .zip(b.chars())
                .all(|(x, y)| x == y || x.to_lowercase().eq(y.to_lowercase()))
    }

    /// Go core.go:650 levenshteinWithMax as written, on new buffers.
    fn go_levenshtein_with_max(s1: &[char], s2: &[char], max_value: f64) -> f64 {
        let mut previous = vec![0.0; s2.len() + 1];
        let mut current = vec![0.0; s2.len() + 1];
        let big = max_value + 0.01;
        for (i, slot) in previous.iter_mut().enumerate() {
            *slot = i as f64;
        }
        for i in 1..=s1.len() {
            let c1 = s1[i - 1];
            let min_j = ((i as f64 - max_value).ceil() as usize).max(1);
            let max_j = ((max_value + i as f64).floor() as usize).min(s2.len());
            let mut col_min = i as f64;
            current[0] = col_min;
            for slot in &mut current[1..min_j] {
                *slot = big;
            }
            for j in min_j..=max_j {
                let substitution_distance =
                    if unicode_to_lower(s1[i - 1]) == unicode_to_lower(s2[j - 1]) {
                        previous[j - 1] + 0.1
                    } else {
                        previous[j - 1] + 2.0
                    };
                let dist = if c1 == s2[j - 1] {
                    previous[j - 1]
                } else {
                    (previous[j] + 1.0).min((current[j - 1] + 1.0).min(substitution_distance))
                };
                current[j] = dist;
                col_min = col_min.min(dist);
            }
            for slot in &mut current[max_j + 1..] {
                *slot = big;
            }
            if col_min > max_value {
                return -1.0;
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let res = previous[s2.len()];
        if res > max_value { -1.0 } else { res }
    }

    /// One `get_spelling_suggestion` call reuses its rune buffer
    /// (`candidate_runes`, clear and extend) and its Levenshtein buffers for
    /// every candidate. With candidates of falling, rising and mixed length,
    /// each call picks the candidate of Go's code, which makes both anew per
    /// candidate.
    #[test]
    fn spelling_suggestion_with_reused_buffers_matches_go() {
        let falling = [
            "getSpellingSuggestionWithMaxCandidateCount",
            "getSpellingSuggestionForStrings",
            "getSpellingSuggestions",
            "GETSPELLINGSUGGESTION",
            "getSpellingSuggestion",
            "getSpelingSugestion",
            "spellingSuggestion",
            "assertNevers",
            "Spellings",
            "spelings",
            "Spelling",
            "\u{e9}t\u{e9}s",
            "\u{c9}T\u{c9}",
            "spel",
            "abd",
            "Abc",
            "ab",
            "\u{c9}",
            "X",
            "x",
        ];
        let rising: Vec<&str> = falling.iter().rev().copied().collect();
        // Long and short in turn: each candidate's runes and rows are
        // longer or shorter than the ones before.
        let mixed: Vec<&str> = (0..falling.len())
            .map(|i| {
                falling[if i % 2 == 0 {
                    i / 2
                } else {
                    falling.len() - 1 - i / 2
                }]
            })
            .collect();
        let names = [
            "getSpellingSuggestion",
            "getSpelingSuggestion",
            "getspellingsuggestions",
            "spelling",
            "Spellingz",
            "assertNever",
            "abc",
            "\u{e9}t\u{e9}",
            "\u{e9}",
            "x",
        ];
        let mut found = 0;
        for candidates in [&falling[..], &rising[..], &mixed[..]] {
            for name in names {
                // 5 stops at the 6th candidate (Go returns the zero value);
                // 20 checks them all, as 0 does.
                for max_candidates in [0, 5, 20] {
                    let go = go_get_spelling_suggestion(name, candidates, max_candidates);
                    let owned = candidates.iter().map(|c| c.to_string());
                    let port = if max_candidates == 0 {
                        get_spelling_suggestion_for_strings(name, owned)
                    } else {
                        get_spelling_suggestion_with_max_candidate_count(
                            name,
                            owned,
                            |c| c.clone(),
                            |a, b| a.cmp(b) as i32,
                            max_candidates,
                        )
                    };
                    assert_eq!(port, go, "{name:?} {max_candidates} {candidates:?}");
                    found += usize::from(!go.is_empty());
                }
            }
        }
        // Every call with no limit or a limit of 20 finds a candidate, so the
        // distances decide the answers.
        assert_eq!(found, 60);
    }

    /// A candidate with a Go string marker (an invalid byte, a lone
    /// surrogate's bytes or a real U+FDD0) is measured on its Go runes
    /// (`go_runes`: one U+FFFD per invalid byte), not on the chars of its
    /// port form, and ties sort by Go bytes. The answers come from Go
    /// core.go:590 getSpellingSuggestion at pin N, run on the same Go
    /// strings (`GetSpellingSuggestion` with `strings.Compare`, from the Go
    /// test `target/continuation-r97-goport/followups10/go/
    /// spelling_marker_test.go` run with `go test -overlay`).
    #[test]
    fn spelling_suggestion_reads_go_runes_of_marked_candidates() {
        let port = |bytes: &[u8]| go_string_from_bytes(bytes.to_vec());
        // (name, candidates, Go answer), all as Go bytes.
        let cases: [(&[u8], &[&[u8]], &[u8]); 9] = [
            // ab U+FFFD cd: equal runes, distance 0. On the port chars
            // (marker, unit char) the distance is 3, over the limit 2.9.
            ("ab\u{FFFD}cd".as_bytes(), &[b"ab\xffcd"], b"ab\xffcd"),
            // A real U+FDD0 is one rune (M + M in the port form).
            (
                "ab\u{FDD0}cd".as_bytes(),
                &["ab\u{FDD0}ce".as_bytes()],
                "ab\u{FDD0}ce".as_bytes(),
            ),
            // The 3 bytes of a lone surrogate are 3 runes U+FFFD.
            (
                "ab\u{FFFD}\u{FFFD}\u{FFFD}cd".as_bytes(),
                &[b"ab\xed\xa0\x80cd"],
                b"ab\xed\xa0\x80cd",
            ),
            // A tie (distance 1): Go bytes "X" < 0xFF.
            (b"abcd", &[b"ab\xffcd", b"abXcd"], b"abXcd"),
            // A marked name and a marked candidate.
            (b"ab\xffc", &[b"ab\xfec", b"abzc"], b"ab\xfec"),
            // Go `len` counts the 3 bytes of U+FFFD: 7 - 4 is over the
            // length limit 2, so only the invalid byte is a candidate.
            (
                b"abcd",
                &[b"ab\xffcd", "ab\u{FFFD}cd".as_bytes()],
                b"ab\xffcd",
            ),
            // A tie (distance 2) between U+FFFD (EF BF BD) and the byte FF:
            // Go bytes put U+FFFD first, in both orders. The port form of
            // FF starts with the marker (EF B7 90), which `str` order puts
            // first.
            (
                b"abcdZefg",
                &[b"abcd\xffefg", "abcd\u{FFFD}efg".as_bytes()],
                "abcd\u{FFFD}efg".as_bytes(),
            ),
            (
                b"abcdZefg",
                &["abcd\u{FFFD}efg".as_bytes(), b"abcd\xffefg"],
                "abcd\u{FFFD}efg".as_bytes(),
            ),
            // No candidate is close enough: the Go zero value.
            (b"abcdZefg", &[b"\xff\xfe\xfd\xfc\xfb\xfa\xf9"], b""),
        ];
        for (name, candidates, go) in cases {
            let answer = get_spelling_suggestion_for_strings(
                &port(name),
                candidates.iter().map(|candidate| port(candidate)),
            );
            assert_eq!(answer, port(go), "{name:?} {candidates:?}");
        }
    }

    /// In a string value, the 3 bytes of a lone surrogate are one fused unit
    /// (`GoUnit::Surrogate`, `go_value_from_bytes`), not 3 invalid byte units
    /// as in source text. Go reads the 3 bytes as 3 runes U+FFFD, counts 3
    /// bytes in `len` and compares them as bytes (80 < ED < FF). The answers
    /// come from Go core.go:635 GetSpellingSuggestionForStrings at pin N, run
    /// on the same Go strings (the Go test `target/continuation-r97-goport/
    /// followups11/go/spelling_fused_test.go`, run with `go test -overlay`).
    #[test]
    fn spelling_suggestion_reads_fused_surrogate_units() {
        let value = |bytes: &[u8]| go_value_from_bytes(bytes).into_owned();
        // The value form of "ab" U+D800 "cd" has a fused unit at byte 2.
        assert_eq!(
            go_unit_at(&value(b"ab\xed\xa0\x80cd"), 2).0,
            GoUnit::Surrogate(0xD800)
        );
        let fffd3 = "ab\u{FFFD}\u{FFFD}\u{FFFD}cd".as_bytes();
        // (name, candidates, Go answer), all as Go bytes.
        let cases: [(&[u8], &[&[u8]], &[u8]); 8] = [
            // 3 runes U+FFFD on each side: distance 0.
            (fffd3, &[b"ab\xed\xa0\x80cd"], b"ab\xed\xa0\x80cd"),
            // A tie with 3 invalid bytes. Go bytes put ED before FF, in both
            // orders. In `str` order the port form of the unit (marker,
            // U+10F800) comes after that of FF (marker, U+10F7FF).
            (
                fffd3,
                &[b"ab\xff\xfe\xfdcd", b"ab\xed\xa0\x80cd"],
                b"ab\xed\xa0\x80cd",
            ),
            (
                fffd3,
                &[b"ab\xed\xa0\x80cd", b"ab\xff\xfe\xfdcd"],
                b"ab\xed\xa0\x80cd",
            ),
            // Go bytes put 80 before ED.
            (
                fffd3,
                &[b"ab\x80\x81\x82cd", b"ab\xed\xa0\x80cd"],
                b"ab\x80\x81\x82cd",
            ),
            // Go `len` of the unit is 3, as the name has 3 runes. The port
            // form has 7 bytes, which is over the length limit 2.
            (
                "\u{FFFD}\u{FFFD}\u{FFFD}".as_bytes(),
                &[b"\xed\xa0\x80"],
                b"\xed\xa0\x80",
            ),
            // A fused name: U+DC00 is also 3 runes U+FFFD, distance 0.
            (
                b"ab\xed\xa0\x80cd",
                &[b"ab\xed\xb0\x80cd", b"abXcd"],
                b"ab\xed\xb0\x80cd",
            ),
            // The name itself is not a suggestion: the Go zero value.
            (b"ab\xed\xa0\x80cd", &[b"ab\xed\xa0\x80cd"], b""),
            // Go `len` 7 against 4 runes is over the length limit 2.
            (b"abcd", &[b"ab\xed\xa0\x80cd"], b""),
        ];
        for (name, candidates, go) in cases {
            let answer = get_spelling_suggestion_for_strings(
                &value(name),
                candidates.iter().map(|candidate| value(candidate)),
            );
            assert_eq!(answer, value(go), "{name:?} {candidates:?}");
        }
    }

    /// The tables of `special_casing_mapping` (js_case_tables.rs): the runs
    /// are sorted, do not overlap and map to chars, and the other entries
    /// are not in a run. go_baselines `test_js_casing` tests the casing.
    #[test]
    fn case_tables_hold_go_special_casing_mappings() {
        use super::CaseMapping::{Char, Str};
        use super::{CASE_RANGES, SPECIAL_CASE_MAPPINGS, UPPER_LOWER, special_casing_mapping};
        let mut next = 0;
        for &(lo, hi, lower, upper) in CASE_RANGES {
            assert!(next <= lo && lo <= hi, "{lo:X}");
            assert_eq!(lower == UPPER_LOWER, upper == UPPER_LOWER, "{lo:X}");
            next = hi + 1;
            for code in lo..=hi {
                // U+FFFD is the fallback of an invalid code point; it has no case.
                let mapping = special_casing_mapping(char::from_u32(code).unwrap());
                assert!(
                    matches!(mapping, Some((Char(l), Char(u)))
                        if l != char::REPLACEMENT_CHARACTER && u != char::REPLACEMENT_CHARACTER),
                    "{code:X} {mapping:?}"
                );
            }
        }
        for &(code, ..) in SPECIAL_CASE_MAPPINGS {
            let mapping = special_casing_mapping(char::from_u32(code).unwrap());
            assert!(matches!(mapping, Some((Str(_), Str(_)))), "{code:X}");
        }
        // A delta run, both ends of an alternating run, a string entry, none.
        let cases = [
            ('A', Some((Char('a'), Char('A')))),
            ('\u{100}', Some((Char('\u{101}'), Char('\u{100}')))),
            ('\u{12F}', Some((Char('\u{12F}'), Char('\u{12E}')))),
            ('\u{DF}', Some((Str("\u{DF}"), Str("SS")))),
            ('1', None),
        ];
        for (r, expected) in cases {
            assert_eq!(special_casing_mapping(r), expected, "{r:?}");
        }
    }

    /// Every kind from FirstKeyword to LastKeyword has its text in both
    /// keyword tables, as in Go `textToKeyword`. The LS keyword completions
    /// read each of these kinds through `token_to_string`, so a kind without
    /// text gives an item with an empty label.
    #[test]
    fn every_keyword_kind_has_its_text() {
        use super::{SyntaxKind, TEXT_TO_KEYWORD, string_to_token, token_to_string};
        assert_eq!(
            TEXT_TO_KEYWORD,
            crate::frontend::scanner::scanner_p1::TEXT_TO_KEYWORD
        );
        for i in SyntaxKind::FIRST_KEYWORD as u16..=SyntaxKind::LAST_KEYWORD as u16 {
            let kind = SyntaxKind::try_from(i).expect("keyword kind");
            let text = token_to_string(kind);
            assert!(!text.is_empty(), "{kind:?}");
            assert_eq!(string_to_token(text), kind, "{text}");
        }
    }
}

#[cfg(test)]
mod debug_site_tests {
    use super::get_text_of_node_from_source_text;
    use crate::core::go_panic_text;
    use crate::prelude::*;

    // Go `GetTextOfNodeFromSourceText` (scanner/utilities.go:97) runs
    // `debug.FailBadSyntaxKind(node, "Unexpected reparser-transformed node kind")`
    // for a reparser-transformed literal that is not a string literal or an
    // identifier. The reparser makes no such node, so the test sets the flag
    // on a synthetic numeric literal (the nodes of a parsed file are frozen).
    #[test]
    fn reparser_transformed_numeric_literal_is_a_go_debug_failure() {
        let literal = NodeFactory::new().new_numeric_literal("1", TokenFlags::NONE);
        set_node_loc(literal, TextRange::new(0, 1));
        set_node_flags(
            literal,
            literal.flags() | NodeFlags::REPARSER_TRANSFORMED_LITERAL,
        );
        let got = go_panic_text(|| {
            get_text_of_node_from_source_text("1;", literal, false);
        });
        assert_eq!(
            got,
            "Debug failure. Unexpected reparser-transformed node kind\nNode KindNumericLiteral was unexpected."
        );
    }
}

#[cfg(test)]
mod position_panic_tests {
    use super::{
        compute_ecma_line_starts, ecma_line_and_utf16_character_of_text_position,
        get_ecma_line_and_utf16_character_of_position,
    };
    use crate::core::go_panic_text;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
    use crate::frontend::tspath::Path;
    use crate::prelude::*;

    // Go `GetECMALineAndUTF16CharacterOfPosition` (scanner.go:2684) on a
    // `pos` out of the text: `lineMap[-1]` and `text[lineMap[line]:pos]`
    // panic with these runtime texts (the pin N oracle gives the same for a
    // bad diagnostic pos in a `.tsbuildinfo`, tsctests::build_info_corrupt).
    // Both port forms (a file node, and a text with its line map) panic as
    // Go, not with a Rust index panic.
    #[test]
    fn a_position_out_of_the_text_panics_as_go() {
        let text = "let a = 1;\nlet é = 2;\n";
        let file = parse_source_file(
            &SourceFileParseOptions {
                file_name: "/a.ts".to_string(),
                path: Path("/a.ts".to_string()),
                ..Default::default()
            },
            text,
            ScriptKind::TS,
        )
        .root;
        let line_map = compute_ecma_line_starts(text);
        let both = |pos: i32| {
            let node = go_panic_text(move || {
                get_ecma_line_and_utf16_character_of_position(file, pos);
            });
            let line_map = line_map.clone();
            let plain = go_panic_text(move || {
                ecma_line_and_utf16_character_of_text_position(&line_map, text, pos);
            });
            assert_eq!(node, plain, "pos {pos}");
            node
        };
        assert_eq!(both(-1), "runtime error: index out of range [-1]");
        assert_eq!(
            both(30),
            "runtime error: slice bounds out of range [:30] with length 23"
        );
        assert_eq!(
            get_ecma_line_and_utf16_character_of_position(file, 23),
            (2, 0)
        );
        assert_eq!(
            ecma_line_and_utf16_character_of_text_position(&line_map, text, 17),
            (1, 5)
        );
    }
}
