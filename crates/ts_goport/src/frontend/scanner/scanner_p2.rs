//! Port of `scanner/scanner.go` lines 1237 to 2226: JSX and JSDoc scans,
//! identifiers, escapes, numbers and strings. Unit U2. The `Scanner` struct,
//! its state and the low-level character helpers (`char`, `char_at`,
//! `char_and_size`, `scan_ascii_while`, `error`, `error_at`, `scan`) are in
//! `scanner_p1.rs` (unit U1).
//!
//! Runes follow scanner_p1.rs: Go `rune` is `i32`, and `rune_to_char` maps
//! one to a `char` for the `stringutil` predicates. The token value is set
//! through `set_token_value` (interned `&'static str`, see scanner_p1.rs).

use crate::frontend::prelude::*;
use std::borrow::Cow;

use crate::scanner_util::push_js_string_rune;

use super::scanner_p1::{
    EscapeSequenceScanningFlags, IdentifierVariant, RUNE_SELF, Scanner, intern_token_value,
    rune_to_char, rune_to_string, utf8_decode_last_rune_in_string, utf8_decode_rune_in_string,
};

/// Go `strconv.ParseInt(s, base, bitSize)` with the error ignored, as the
/// scanner does. The input holds only digits of `base`.
// PORT: on overflow Go returns the largest value for `bit_size` (and an
// error the scanner drops). This keeps that value. An empty or invalid input
// returns 0, as Go does.
fn go_parse_int(s: &str, base: u32, bit_size: u32) -> i64 {
    let max: i64 = if bit_size >= 64 {
        i64::MAX
    } else {
        (1i64 << (bit_size - 1)) - 1
    };
    if s.is_empty() {
        return 0;
    }
    let mut value: i64 = 0;
    for c in s.chars() {
        let Some(digit) = c.to_digit(base) else {
            return 0;
        };
        value = match value
            .checked_mul(base as i64)
            .and_then(|v| v.checked_add(digit as i64))
        {
            Some(v) if v <= max => v,
            _ => return max,
        };
    }
    value
}

/// Go `jsnum.FromString(text).String()`.
fn js_number_string(text: &str) -> String {
    crate::jsnum::Number::from_string(text).to_string()
}

/// Whether `js_number_string(text) == text` is known without parsing: a
/// decimal integer with no leading zero and at most 15 digits. Every such
/// integer is below 2^53, so it prints as its own digits.
fn is_canonical_js_integer(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 15
        && (bytes[0] != b'0' || bytes.len() == 1)
        && bytes.iter().all(u8::is_ascii_digit)
}

/// The bytes that continue an ASCII identifier: letters, digits, `_`, `$`.
/// Bytes >= 0x80 are false, so a scan stops there like `scan_ascii_while`.
static ASCII_IDENT_PART: [bool; 256] = {
    let mut table = [false; 256];
    let mut i = 0;
    while i < 128 {
        let b = i as u8;
        table[i] = b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        i += 1;
    }
    table
};

/// The bytes that start an ASCII identifier on Go's fast path: letters, `_`,
/// `$`.
static ASCII_IDENT_START: [bool; 256] = {
    let mut table = [false; 256];
    let mut i = 0;
    while i < 128 {
        let b = i as u8;
        table[i] = b.is_ascii_alphabetic() || b == b'_' || b == b'$';
        i += 1;
    }
    table
};

/// The bytes that `Scanner::scan_ascii_identifier` reads in one window.
const IDENT_WINDOW: usize = 16;

/// A numeric fragment from `scan_number_fragment`: `prefix` followed by
/// `text[start..end]`. `prefix` holds the digits before the last separator
/// and stays empty (no allocation) when there is no separator.
pub(crate) struct NumberFragment {
    prefix: String,
    start: i32,
    end: i32,
}

impl NumberFragment {
    fn is_empty(&self) -> bool {
        self.prefix.is_empty() && self.start == self.end
    }

    /// Appends the fragment value to `out`.
    fn push_to(&self, text: &str, out: &mut String) {
        out.push_str(&self.prefix);
        out.push_str(&text[self.start as usize..self.end as usize]);
    }
}

impl<'a> Scanner<'a> {
    // Go: scanner/scanner.go:1221 ReScanJsxToken
    pub fn re_scan_jsx_token(&mut self, allow_multiline_jsx_text: bool) -> SyntaxKind {
        self.scanner_state.pos = self.scanner_state.full_start_pos;
        self.scanner_state.token_start = self.scanner_state.full_start_pos;
        self.scanner_state.token = self.scan_jsx_token_ex(allow_multiline_jsx_text);
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1228 ReScanHashToken
    pub fn re_scan_hash_token(&mut self) -> SyntaxKind {
        if self.scanner_state.token == SyntaxKind::PrivateIdentifier {
            self.scanner_state.pos = self.scanner_state.token_start + 1;
            self.scanner_state.token = SyntaxKind::HashToken;
        }
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1236 ReScanQuestionToken
    pub fn re_scan_question_token(&mut self) -> SyntaxKind {
        if self.scanner_state.token != SyntaxKind::QuestionQuestionToken {
            panic!("'reScanQuestionToken' should only be called on a '??'");
        }
        self.scanner_state.pos = self.scanner_state.token_start + 1;
        self.scanner_state.token = SyntaxKind::QuestionToken;
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1245 ScanJsxToken
    pub fn scan_jsx_token(&mut self) -> SyntaxKind {
        self.scan_jsx_token_ex(true /*allowMultilineJsxText*/)
    }

    // Go: scanner/scanner.go:1249 ScanJsxTokenEx
    pub fn scan_jsx_token_ex(&mut self, allow_multiline_jsx_text: bool) -> SyntaxKind {
        self.scanner_state.full_start_pos = self.scanner_state.pos;
        self.scanner_state.token_start = self.scanner_state.pos;
        let ch = self.char();
        if ch < 0 {
            self.scanner_state.token = SyntaxKind::EndOfFile;
        } else if ch == '<' as i32 {
            if self.char_at(1) == '/' as i32 {
                self.scanner_state.pos += 2;
                self.scanner_state.token = SyntaxKind::LessThanSlashToken;
            } else {
                self.scanner_state.pos += 1;
                self.scanner_state.token = SyntaxKind::LessThanToken;
            }
        } else if ch == '{' as i32 {
            self.scanner_state.pos += 1;
            self.scanner_state.token = SyntaxKind::OpenBraceToken;
        } else {
            // First non-whitespace character on this line.
            let mut first_non_whitespace: i32 = 0;
            // These initial values are special because the first line is:
            // firstNonWhitespace = 0 to indicate that we want leading whitespace
            loop {
                let (ch, size) = self.char_and_size();
                let ch = rune_to_char(ch);
                if size == 0 || ch == '{' {
                    break;
                }
                if ch == '<' {
                    if is_conflict_marker_trivia(&self.text, self.scanner_state.pos as usize) {
                        self.scanner_state.pos = self.scan_conflict_marker_trivia_at_pos();
                        self.scanner_state.token = SyntaxKind::ConflictMarkerTrivia;
                        return self.scanner_state.token;
                    }
                    break;
                }
                if ch == '>' {
                    self.error_at(
                        diag::Unexpected_token_Did_you_mean_or_gt,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                } else if ch == '}' {
                    self.error_at(
                        diag::Unexpected_token_Did_you_mean_or_rbrace,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                }
                // FirstNonWhitespace is 0, then we only see whitespaces so far. If we see a linebreak, we want to ignore that whitespaces.
                // i.e (- : whitespace)
                //      <div>----
                //      </div> becomes <div></div>
                //
                //      <div>----</div> becomes <div>----</div>
                if is_line_break(ch) && first_non_whitespace == 0 {
                    first_non_whitespace = -1;
                } else if !allow_multiline_jsx_text && is_line_break(ch) && first_non_whitespace > 0
                {
                    // Stop JsxText on each line during formatting. This allows the formatter to
                    // indent each line correctly.
                    break;
                } else if !is_white_space_like(ch) {
                    first_non_whitespace = self.scanner_state.pos;
                }
                self.scanner_state.pos += size;
            }
            self.scanner_state.token_value = self.text_token_value(
                self.scanner_state.full_start_pos as usize,
                self.scanner_state.pos as usize,
            );
            self.scanner_state.token = SyntaxKind::JsxText;
            if first_non_whitespace == -1 {
                self.scanner_state.token = SyntaxKind::JsxTextAllWhiteSpaces;
            }
        }
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1317 ScanJsxIdentifier
    // Scans a JSX identifier; these differ from normal identifiers in that they allow dashes
    pub fn scan_jsx_identifier(&mut self) -> SyntaxKind {
        // PORT: Go `tokenIsIdentifierOrKeyword` from scanner/utilities.go is
        // private there; this uses the same pub function from the prelude
        // (checker/utilities_p1.rs).
        if token_is_identifier_or_keyword(self.scanner_state.token) {
            // An identifier or keyword has already been parsed - check for a `-` or a single instance of `:` and then append it and
            // everything after it to the token
            // Do note that this means that `scanJsxIdentifier` effectively _mutates_ the visible token without advancing to a new token
            // Any caller should be expecting this behavior and should only read the pos or token value after calling it.
            // Here scanIdentifierParts is reused to ensure Unicode escapes are handled.
            let parts = self.scan_identifier_parts(IdentifierVariant::JSX);
            let value = format!("{}{}", self.scanner_state.token_value, parts);
            self.set_token_value(&value);
            self.scanner_state.token = get_identifier_token(self.scanner_state.token_value);
        }
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1330 ScanJsxAttributeValue
    pub fn scan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.scanner_state.full_start_pos = self.scanner_state.pos;
        // Skip whitespace between '=' and the value so tokenStart lands on the
        // opening quote, not on trivia.
        let (mut ch, mut size) = self.char_and_size();
        while size > 0 && is_white_space_like(rune_to_char(ch)) {
            self.scanner_state.pos += size;
            (ch, size) = self.char_and_size();
        }
        self.scanner_state.token_start = self.scanner_state.pos;
        let c = self.char();
        if c == '"' as i32 || c == '\'' as i32 {
            self.scanner_state.token_value = self.scan_string(true /*jsxAttributeString*/);
            self.scanner_state.token = SyntaxKind::StringLiteral;
            self.scanner_state.token
        } else {
            // If this scans anything other than `{`, it's a parse error.
            self.scan()
        }
    }

    // Go: scanner/scanner.go:1349 ReScanJsxAttributeValue
    pub fn re_scan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.scanner_state.pos = self.scanner_state.full_start_pos;
        self.scanner_state.token_start = self.scanner_state.full_start_pos;
        self.scan_jsx_attribute_value()
    }

    // Go: scanner/scanner.go:1356 ScanJSDocCommentTextToken
    /** In addition to the usual JSDoc ast.Kinds, can also return ast.KindJSDocCommentTextToken */
    pub fn scan_js_doc_comment_text_token(&mut self, in_backticks: bool) -> SyntaxKind {
        self.scanner_state.full_start_pos = self.scanner_state.pos;
        self.scanner_state.token_flags = TokenFlags::NONE;
        if self.scanner_state.pos >= self.text.len() as i32 {
            self.scanner_state.token = SyntaxKind::EndOfFile;
            return self.scanner_state.token;
        }
        self.scanner_state.token_start = self.scanner_state.pos;
        let (mut ch, mut size) = self.char_and_size();
        while self.scanner_state.pos < self.text.len() as i32
            && !is_line_break(rune_to_char(ch))
            && ch != '`' as i32
        {
            if !in_backticks {
                if ch == '{' as i32 {
                    break;
                } else if ch == '@' as i32 && self.scanner_state.pos >= 0 {
                    // @ doesn't start a new tag inside ``, and elsewhere, only after whitespace and before identifier
                    let (previous, _) = utf8_decode_last_rune_in_string(
                        &self.text,
                        self.scanner_state.pos as usize,
                    );
                    if is_white_space_single_line(rune_to_char(previous)) {
                        let (next, _) = utf8_decode_rune_in_string(
                            &self.text,
                            (self.scanner_state.pos + size) as usize,
                        );
                        if is_identifier_start(rune_to_char(next)) {
                            break;
                        }
                    }
                }
            }
            self.scanner_state.pos += size;
            (ch, size) = self.char_and_size();
        }
        if self.scanner_state.pos == self.scanner_state.token_start {
            return self.scan_js_doc_token();
        }
        self.scanner_state.token_value = self.text_token_value(
            self.scanner_state.token_start as usize,
            self.scanner_state.pos as usize,
        );
        self.scanner_state.token = SyntaxKind::JsDocCommentTextToken;
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1392 CanFollowJSDocAt
    // Peek at the character at the current scanner position (expected to be right after '@')
    // and return true if a JSDoc tag can follow. Identifier starts indicate a tag name.
    // Whitespace, newlines, and EOF are also accepted to support incomplete tags for code completion.
    pub fn can_follow_js_doc_at(&self) -> bool {
        if self.scanner_state.pos >= self.text.len() as i32 {
            return true;
        }
        let (ch, _) = utf8_decode_rune_in_string(&self.text, self.scanner_state.pos as usize);
        let ch = rune_to_char(ch);
        is_identifier_start(ch) || is_white_space_single_line(ch) || is_line_break(ch)
    }

    // Go: scanner/scanner.go:1400 ScanJSDocToken
    pub fn scan_js_doc_token(&mut self) -> SyntaxKind {
        self.scanner_state.full_start_pos = self.scanner_state.pos;
        self.scanner_state.token_flags = TokenFlags::NONE;
        if self.scanner_state.pos >= self.text.len() as i32 {
            self.scanner_state.token = SyntaxKind::EndOfFile;
            return self.scanner_state.token;
        }

        self.scanner_state.token_start = self.scanner_state.pos;
        let (ch, size) = self.char_and_size();
        let ch = rune_to_char(ch);
        self.scanner_state.pos += size;
        match ch {
            '\t' | '\u{000B}' | '\u{000C}' | ' ' => {
                let (mut ch2, mut size2) = self.char_and_size();
                while size2 > 0 && is_white_space_single_line(rune_to_char(ch2)) {
                    self.scanner_state.pos += size2;
                    (ch2, size2) = self.char_and_size();
                }
                self.scanner_state.token = SyntaxKind::WhitespaceTrivia;
                return self.scanner_state.token;
            }
            '@' => {
                self.scanner_state.token = SyntaxKind::AtToken;
                return self.scanner_state.token;
            }
            '\r' | '\n' => {
                if ch == '\r' && self.char() == '\n' as i32 {
                    self.scanner_state.pos += 1;
                }
                // Go: `case '\r'` falls through to `case '\n'`.
                self.scanner_state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                self.scanner_state.token = SyntaxKind::NewLineTrivia;
                return self.scanner_state.token;
            }
            '*' => {
                self.scanner_state.token = SyntaxKind::AsteriskToken;
                return self.scanner_state.token;
            }
            '{' => {
                self.scanner_state.token = SyntaxKind::OpenBraceToken;
                return self.scanner_state.token;
            }
            '}' => {
                self.scanner_state.token = SyntaxKind::CloseBraceToken;
                return self.scanner_state.token;
            }
            '[' => {
                self.scanner_state.token = SyntaxKind::OpenBracketToken;
                return self.scanner_state.token;
            }
            ']' => {
                self.scanner_state.token = SyntaxKind::CloseBracketToken;
                return self.scanner_state.token;
            }
            '(' => {
                self.scanner_state.token = SyntaxKind::OpenParenToken;
                return self.scanner_state.token;
            }
            ')' => {
                self.scanner_state.token = SyntaxKind::CloseParenToken;
                return self.scanner_state.token;
            }
            '<' => {
                self.scanner_state.token = SyntaxKind::LessThanToken;
                return self.scanner_state.token;
            }
            '>' => {
                self.scanner_state.token = SyntaxKind::GreaterThanToken;
                return self.scanner_state.token;
            }
            '=' => {
                self.scanner_state.token = SyntaxKind::EqualsToken;
                return self.scanner_state.token;
            }
            ',' => {
                self.scanner_state.token = SyntaxKind::CommaToken;
                return self.scanner_state.token;
            }
            '.' => {
                self.scanner_state.token = SyntaxKind::DotToken;
                return self.scanner_state.token;
            }
            '`' => {
                self.scanner_state.token = SyntaxKind::BacktickToken;
                return self.scanner_state.token;
            }
            '#' => {
                self.scanner_state.token = SyntaxKind::HashToken;
                return self.scanner_state.token;
            }
            _ => {}
        }

        self.scanner_state.pos = self.scanner_state.token_start;
        if self.scan_identifier(0, IdentifierVariant::JSX) {
            self.scanner_state.token = get_identifier_token(self.scanner_state.token_value);
            return self.scanner_state.token;
        }
        self.scanner_state.pos = self.scanner_state.token_start + size;
        self.scanner_state.token = SyntaxKind::Unknown;
        self.scanner_state.token
    }

    /// Go `scanIdentifier(0, identifierVariantStandard)` when it takes its
    /// ASCII fast path, then Go `GetIdentifierToken` of the token value: the
    /// token kind, with `pos` and the token value set as Go sets them.
    /// `None`, with nothing changed, when the fast path does not apply (the
    /// first byte is no ASCII letter, `_` or `$`, the name ends at a
    /// non-ASCII byte or `\`) or when fewer than 16 bytes are left before
    /// `end`. The caller then calls `scan_identifier`.
    // PERF: scanintern1 C1. `scan` inlines this. The first 16 bytes come from
    // one window with no end test per byte, and the work needs no saved
    // registers. Out of line, `scan_identifier` paid 92 instructions per call
    // besides its loop (VS Code load: 9.6 M calls).
    #[inline(always)]
    pub(crate) fn scan_ascii_identifier(&mut self) -> Option<SyntaxKind> {
        let start = usize::try_from(self.scanner_state.pos).ok()?;
        let end = usize::try_from(self.end).ok()?;
        let bytes = self.text.as_bytes().get(..end)?;
        let window: &[u8; IDENT_WINDOW] = bytes.get(start..)?.first_chunk()?;
        if !ASCII_IDENT_START[usize::from(window[0])] {
            return None;
        }
        let mut len = window
            .iter()
            .position(|&b| !ASCII_IDENT_PART[usize::from(b)])
            .unwrap_or(IDENT_WINDOW);
        // Go `s.char()` after the run: the byte there, or -1 at `end`.
        let next = if len < IDENT_WINDOW {
            Some(window[len])
        } else {
            let rest = &bytes[start + IDENT_WINDOW..];
            let more = rest
                .iter()
                .position(|&b| !ASCII_IDENT_PART[usize::from(b)])
                .unwrap_or(rest.len());
            len += more;
            rest.get(more).copied()
        };
        // Go: `ch < utf8.RuneSelf && ch != '\\'`.
        if next.is_some_and(|b| b >= 0x80 || b == b'\\') {
            return None;
        }
        let value = self.text_token_value(start, start + len);
        self.scanner_state.pos += len as i32;
        self.scanner_state.token_value = value;
        Some(get_identifier_token(value))
    }

    // Go: scanner/scanner.go:1484 scanIdentifier
    pub(crate) fn scan_identifier(
        &mut self,
        prefix_length: i32,
        variant: IdentifierVariant,
    ) -> bool {
        let start = self.scanner_state.pos;
        self.scanner_state.pos += prefix_length;
        let identifier_start = self.scanner_state.pos;
        let ch = self.char();
        // Fast path for simple ASCII identifiers
        if variant != IdentifierVariant::JSX
            && (is_ascii_letter(rune_to_char(ch)) || ch == '_' as i32 || ch == '$' as i32)
        {
            self.scanner_state.pos += 1;
            let rest = &self.text.as_bytes()[self.scanner_state.pos as usize..self.end as usize];
            let len = rest
                .iter()
                .position(|&b| !ASCII_IDENT_PART[usize::from(b)])
                .unwrap_or(rest.len());
            self.scanner_state.pos += len as i32;
            let ch = self.char();
            if ch < RUNE_SELF && ch != '\\' as i32 {
                self.scanner_state.token_value =
                    self.text_token_value(start as usize, self.scanner_state.pos as usize);
                return true;
            }
            self.scanner_state.pos = identifier_start;
        }
        let (mut ch, mut size) = self.char_and_size();
        if is_identifier_start(rune_to_char(ch)) {
            let mut language_variant = LanguageVariant::STANDARD;
            if variant == IdentifierVariant::JSX {
                language_variant = LanguageVariant::JSX;
            }
            loop {
                self.scanner_state.pos += size;
                (ch, size) = self.char_and_size();
                if !is_identifier_part_ex(rune_to_char(ch), language_variant) {
                    break;
                }
            }
            let mut value = self.text[start as usize..self.scanner_state.pos as usize].to_string();
            if ch == '\\' as i32 {
                let parts = self.scan_identifier_parts(variant);
                value.push_str(&parts);
            }
            self.set_token_value(&value);
            return true;
        }
        if ch == '\\' as i32
            && let Some(escaped) = self.scan_identifier_escape(
                |ch| is_identifier_start(rune_to_char(ch)),
                variant == IdentifierVariant::REG_EXP_GROUP_NAME,
            )
        {
            let mut value = self.text[start as usize..identifier_start as usize].to_string();
            value.push_str(&rune_to_string(escaped));
            let parts = self.scan_identifier_parts(variant);
            value.push_str(&parts);
            self.set_token_value(&value);
            return true;
        }
        false
    }

    // Go: scanner/scanner.go:1530 scanIdentifierParts
    pub(crate) fn scan_identifier_parts(&mut self, variant: IdentifierVariant) -> String {
        let mut sb = String::new();
        let mut start = self.scanner_state.pos;
        let mut language_variant = LanguageVariant::STANDARD;
        if variant == IdentifierVariant::JSX {
            language_variant = LanguageVariant::JSX;
        }
        loop {
            let (ch, size) = self.char_and_size();
            if is_identifier_part_ex(rune_to_char(ch), language_variant) {
                self.scanner_state.pos += size;
                continue;
            }
            if ch == '\\' as i32 {
                let escape_start = self.scanner_state.pos;
                if let Some(escaped) = self.scan_identifier_escape(
                    |ch| is_identifier_part_ex(rune_to_char(ch), language_variant),
                    variant == IdentifierVariant::REG_EXP_GROUP_NAME,
                ) {
                    sb.push_str(&self.text[start as usize..escape_start as usize]);
                    sb.push_str(&rune_to_string(escaped));
                    start = self.scanner_state.pos;
                    continue;
                }
            }
            break;
        }
        sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
        sb
    }

    // Go: scanner/scanner.go:1560 scanIdentifierEscape (ts#63996)
    // PORT: `(rune, bool)` returns `Option<i32>`.
    fn scan_identifier_escape(
        &mut self,
        is_valid: impl Fn(i32) -> bool,
        allow_surrogate_pair_escape: bool,
    ) -> Option<i32> {
        let escaped = self.peek_unicode_escape();
        if escaped >= 0 && is_valid(escaped) {
            return Some(self.scan_unicode_escape(true));
        }
        if allow_surrogate_pair_escape
            && self.char_at(2) != '{' as i32
            && is_high_surrogate(escaped as u32)
        {
            // Unlike normal identifiers, group names in regular expressions, whether in Unicode mode or not,
            // accept \u HexLeadSurrogate \u HexTrailSurrogate as part of RegExpIdentifierName.
            // See https://github.com/tc39/ecma262/pull/1869 for the change.
            let saved_pos = self.scanner_state.pos;
            let saved_token_flags = self.scanner_state.token_flags;
            self.scan_unicode_escape(false);
            // scanLowSurrogateEscape also accepts the braced form used in string literals,
            // but RegExpIdentifierName does not allow it.
            if self.char_at(2) != '{' as i32
                && let Some(code_point) = self.scan_low_surrogate_escape(escaped)
                && is_valid(code_point)
            {
                return Some(code_point);
            }
            self.scanner_state.pos = saved_pos;
            self.scanner_state.token_flags = saved_token_flags;
        }
        None
    }

    // Go: scanner/scanner.go:1585 scanString
    // PORT: returns the token value. A plain string is a slice of the source
    // text (see `text_token_value`), so it needs no copy or intern. The
    // source text is in the port form (see `scanner_util::GO_STRING_MARKER`),
    // and the value is in the value form (`string_token_value`,
    // `scanner_util::go_value`): Go joins the bytes of the parts around an
    // escape, so the invalid bytes before and after a line continuation can
    // form one char.
    pub(crate) fn scan_string(&mut self, jsx_attribute_string: bool) -> &'a str {
        let quote = self.char();
        if quote == '\'' as i32 {
            self.scanner_state.token_flags |= TokenFlags::SINGLE_QUOTE;
        }
        self.scanner_state.pos += 1;
        // Fast path for simple strings without escape sequences.
        // `quote` is `"` or `'` here, so it fits in a byte.
        let str_len: i32 = memchr::memchr(
            quote as u8,
            &self.text.as_bytes()[self.scanner_state.pos as usize..],
        )
        .map_or(-1, |i| i as i32);
        if str_len == 0 {
            self.scanner_state.pos += 1;
            return "";
        }
        if str_len > 0 {
            let from = self.scanner_state.pos as usize;
            let to = from + str_len as usize;
            if jsx_attribute_string
                || memchr::memchr3(b'\\', b'\r', b'\n', &self.text.as_bytes()[from..to]).is_none()
            {
                self.scanner_state.pos += str_len + 1;
                return self.string_token_value(from, to);
            }
        }
        let mut sb = String::new();
        let mut start = self.scanner_state.pos;
        loop {
            let ch = self.char();
            if ch < 0 {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                self.error(diag::Unterminated_string_literal);
                break;
            }
            if ch == quote {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                self.scanner_state.pos += 1;
                break;
            }
            if ch == '\\' as i32 && !jsx_attribute_string {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                self.scan_escape_sequence_into(
                    EscapeSequenceScanningFlags::STRING
                        | EscapeSequenceScanningFlags::REPORT_ERRORS,
                    &mut sb,
                );
                start = self.scanner_state.pos;
                continue;
            }
            if (ch == '\n' as i32 || ch == '\r' as i32) && !jsx_attribute_string {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                self.error(diag::Unterminated_string_literal);
                break;
            }
            self.scanner_state.pos += 1;
        }
        intern_token_value(&go_value(&sb))
    }

    /// The string value `text[from..to]`: the source slice in the value form,
    /// with the bytes of each WTF-8 surrogate fused into one unit (see
    /// `scanner_util::go_value`).
    // PORT: Go uses the slice as is. It holds the same Go bytes.
    fn string_token_value(&self, from: usize, to: usize) -> &'a str {
        if self.text_has_no_go_string_marker {
            return self.text_token_value(from, to);
        }
        match go_value(&self.text[from..to]) {
            Cow::Borrowed(_) => self.text_token_value(from, to),
            Cow::Owned(value) => intern_token_value(&value),
        }
    }

    // Go: scanner/scanner.go:1637 scanTemplateAndSetTokenValue
    pub(crate) fn scan_template_and_set_token_value(
        &mut self,
        should_emit_invalid_escape_error: bool,
    ) -> SyntaxKind {
        let started_with_backtick = self.char() == '`' as i32;
        self.scanner_state.pos += 1;
        let mut start = self.scanner_state.pos;
        // PORT: Go collects `parts` and joins them. This keeps one `String` of
        // the parts before `start`. When it stays empty (no escape and no
        // `\r`), the value is the source slice `text[start..value_end]`.
        let mut sb = String::new();
        let value_end;
        let token;
        loop {
            self.scan_ascii_while(|b: u8| b != b'`' && b != b'$' && b != b'\\' && b != b'\r');
            let ch = self.char();
            if ch < 0 || ch == '`' as i32 {
                value_end = self.scanner_state.pos;
                if ch == '`' as i32 {
                    self.scanner_state.pos += 1;
                } else {
                    self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                    self.error(diag::Unterminated_template_literal);
                }
                token = if started_with_backtick {
                    SyntaxKind::NoSubstitutionTemplateLiteral
                } else {
                    SyntaxKind::TemplateTail
                };
                break;
            }
            if ch == '$' as i32 && self.char_at(1) == '{' as i32 {
                value_end = self.scanner_state.pos;
                self.scanner_state.pos += 2;
                token = if started_with_backtick {
                    SyntaxKind::TemplateHead
                } else {
                    SyntaxKind::TemplateMiddle
                };
                break;
            }
            if ch == '\\' as i32 {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                let flags = EscapeSequenceScanningFlags::STRING
                    | if should_emit_invalid_escape_error {
                        EscapeSequenceScanningFlags::REPORT_ERRORS
                    } else {
                        EscapeSequenceScanningFlags::default()
                    };
                self.scan_escape_sequence_into(flags, &mut sb);
                start = self.scanner_state.pos;
                continue;
            }
            // Speculated ECMAScript 6 Spec 11.8.6.1:
            // <CR><LF> and <CR> LineTerminatorSequences are normalized to <LF> for Template Values
            if ch == '\r' as i32 {
                sb.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                self.scanner_state.pos += 1;
                if self.char() == '\n' as i32 {
                    self.scanner_state.pos += 1;
                }
                sb.push('\n');
                start = self.scanner_state.pos;
                continue;
            }
            self.scanner_state.pos += 1;
        }
        if sb.is_empty() {
            self.scanner_state.token_value =
                self.string_token_value(start as usize, value_end as usize);
        } else {
            sb.push_str(&self.text[start as usize..value_end as usize]);
            self.set_token_value(&go_value(&sb));
        }
        token
    }

    // Go: scanner/scanner.go:1689 scanEscapeSequence
    // PORT: the `String` form of `scan_escape_sequence_into`, for regexp.rs.
    // The string and template scanners call `scan_escape_sequence_into`.
    pub(crate) fn scan_escape_sequence(&mut self, flags: EscapeSequenceScanningFlags) -> String {
        let mut out = String::new();
        self.scan_escape_sequence_into(flags, &mut out);
        out
    }

    // Go: scanner/scanner.go:1689 scanEscapeSequence
    // PORT: Go returns strings that can hold a CESU-8 lone surrogate
    // (`EncodeJSStringRune`). A Rust `String` cannot; `push_js_string_rune`
    // writes a valid-UTF-8 escape form instead (see
    // `scanner_util::GO_STRING_MARKER`).
    // PERF: appends the Go return value to `out` (the caller's builder) so a
    // common escape such as `\n` does not allocate a `String`.
    pub(crate) fn scan_escape_sequence_into(
        &mut self,
        flags: EscapeSequenceScanningFlags,
        out: &mut String,
    ) {
        let start = self.scanner_state.pos;
        self.scanner_state.pos += 1;
        let ch = self.char();
        if ch < 0 {
            self.error(diag::Unexpected_end_of_text);
            return;
        }
        self.scanner_state.pos += 1;
        // PORT: the Go `switch` with `fallthrough` from '0' to '1'..'3' to
        // '4'..'7' becomes one arm with the same checks in order.
        let c = rune_to_char(ch);
        match c {
            '0'..='7' => {
                // Although '0' preceding any digit is treated as LegacyOctalEscapeSequence,
                // '\08' should separately be interpreted as '\0' + '8'.
                if c == '0' && !is_digit(rune_to_char(self.char())) {
                    out.push('\x00');
                    return;
                }
                // '\01', '\011'
                if c <= '3' {
                    // '\1', '\17', '\177'
                    if is_octal_digit(rune_to_char(self.char())) {
                        self.scanner_state.pos += 1;
                    }
                    // '\17', '\177'
                }
                // '\4', '\47' but not '\477'
                if is_octal_digit(rune_to_char(self.char())) {
                    self.scanner_state.pos += 1;
                }
                // '\47'
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
                    let code = go_parse_int(
                        &self.text[(start + 1) as usize..self.scanner_state.pos as usize],
                        8,
                        32,
                    );
                    if flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && !flags.intersects(EscapeSequenceScanningFlags::ATOM_ESCAPE)
                        && c != '0'
                    {
                        self.error_at(diag::Octal_escape_sequences_and_backreferences_are_not_allowed_in_a_character_class_If_this_was_intended_as_an_escape_sequence_use_the_syntax_0_instead, start, self.scanner_state.pos - start, args![format!("\\x{:02x}", code)]);
                    } else {
                        self.error_at(
                            diag::Octal_escape_sequences_are_not_allowed_Use_the_syntax_0,
                            start,
                            self.scanner_state.pos - start,
                            args![format!("\\x{:02x}", code)],
                        );
                    }
                    out.push(rune_to_char(code as i32));
                    return;
                }
                out.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
            }
            '8' | '9' => {
                // the invalid '\8' and '\9'
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                if flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS) {
                    if flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && !flags.intersects(EscapeSequenceScanningFlags::ATOM_ESCAPE)
                    {
                        self.error_at(
                            diag::Decimal_escape_sequences_and_backreferences_are_not_allowed_in_a_character_class,
                            start,
                            self.scanner_state.pos - start,
                            Vec::new(),
                        );
                    } else {
                        self.error_at(
                            diag::Escape_sequence_0_is_not_allowed,
                            start,
                            self.scanner_state.pos - start,
                            args![&self.text[start as usize..self.scanner_state.pos as usize]],
                        );
                    }
                    out.push(c);
                    return;
                }
                out.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
            }
            'b' => out.push('\u{0008}'),
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'v' => out.push('\u{000B}'),
            'f' => out.push('\u{000C}'),
            'r' => out.push('\r'),
            '\'' => out.push('\''),
            '"' => out.push('"'),
            'u' => {
                // '\uDDDD' and '\u{DDDDDD}'
                let extended = self.char() == '{' as i32;
                self.scanner_state.pos -= 2;
                let code_point = self.scan_unicode_escape(
                    flags.intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS),
                );
                if extended {
                    if !flags.intersects(EscapeSequenceScanningFlags::ALLOW_EXTENDED_UNICODE_ESCAPE)
                    {
                        self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                        if flags
                            .intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS)
                        {
                            self.error_at(diag::Unicode_escape_sequences_are_only_available_when_the_Unicode_u_flag_or_the_Unicode_Sets_v_flag_is_set, start, self.scanner_state.pos - start, Vec::new());
                        }
                    }
                    if code_point < 0 {
                        out.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                        return;
                    }
                    // In string literals, a high surrogate \u{...} followed by a low
                    // surrogate escape forms a single code point, exactly as adjacent
                    // UTF-16 code units would in a JavaScript string.
                    if !flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && is_high_surrogate(code_point as u32)
                    {
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            out.push(rune_to_char(combined));
                            return;
                        }
                    }
                    push_js_string_rune(out, code_point as u32);
                    return;
                }
                if code_point < 0 {
                    out.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                    return;
                } else if is_high_surrogate(code_point as u32) {
                    if !flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION) {
                        // Combine \uHigh followed by any low surrogate escape (\uLow or
                        // \u{Low}) into a single code point in string literals, matching
                        // how adjacent UTF-16 code units pair in a JavaScript string.
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            out.push(rune_to_char(combined));
                            return;
                        }
                    } else if flags.intersects(EscapeSequenceScanningFlags::ANY_UNICODE_MODE)
                        && self.char() == '\\' as i32
                        && self.char_at(1) == 'u' as i32
                        && self.char_at(2) != '{' as i32
                    {
                        // In regex AnyUnicodeMode, combine \uHigh\uLow so scanClassRanges
                        // can compare the pair numerically. In non-unicode regex mode they
                        // are separate atoms, and extended \u{...} escapes never combine.
                        let saved_pos = self.scanner_state.pos;
                        let next_code_point =
                            self.scan_unicode_escape(flags.intersects(
                                EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS,
                            ));
                        if next_code_point >= 0 && is_low_surrogate(next_code_point as u32) {
                            out.push(rune_to_char(surrogate_pair_to_code_point(
                                code_point as u32,
                                next_code_point as u32,
                            ) as i32));
                            return;
                        }
                        self.scanner_state.pos = saved_pos;
                    }
                }
                // Lone surrogate: encode as CESU-8 so it survives losslessly. In a
                // non-unicode regex this also lets scanClassRanges compare it numerically.
                push_js_string_rune(out, code_point as u32);
            }
            'x' => {
                // '\xDD'
                while self.scanner_state.pos < start + 4 {
                    if !is_hex_digit(rune_to_char(self.char())) {
                        self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                        if flags
                            .intersects(EscapeSequenceScanningFlags::REPORT_INVALID_ESCAPE_ERRORS)
                        {
                            self.error(diag::Hexadecimal_digit_expected);
                        }
                        out.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                        return;
                    }
                    self.scanner_state.pos += 1;
                }
                self.scanner_state.token_flags |= TokenFlags::HEX_ESCAPE;
                let escaped_value = go_parse_int(
                    &self.text[(start + 2) as usize..self.scanner_state.pos as usize],
                    16,
                    32,
                );
                out.push(rune_to_char(escaped_value as i32));
            }
            '\r' | '\n' => {
                // when encountering a LineContinuation (i.e. a backslash and a line terminator sequence),
                // the line terminator is interpreted to be "the empty code unit sequence".
                if c == '\r' && self.char() == '\n' as i32 {
                    self.scanner_state.pos += 1;
                }
                // Go: `case '\r'` falls through to `case '\n'` and returns "".
            }
            _ => {
                // ch was read as a single byte; for multi-byte UTF-8 characters,
                // we need to decode the full rune and advance past all its bytes.
                let mut c = c;
                if ch >= RUNE_SELF {
                    self.scanner_state.pos -= 1; // back up past the single-byte advance
                    let (r, size) =
                        utf8_decode_rune_in_string(&self.text, self.scanner_state.pos as usize);
                    c = rune_to_char(r);
                    self.scanner_state.pos += size;
                }
                // LineContinuation: a backslash followed by a line terminator is "the empty code unit sequence".
                if c == '\u{2028}' || c == '\u{2029}' {
                    return;
                }
                if flags.intersects(EscapeSequenceScanningFlags::ANY_UNICODE_MODE)
                    || flags.intersects(EscapeSequenceScanningFlags::REGULAR_EXPRESSION)
                        && !flags.intersects(EscapeSequenceScanningFlags::ANNEX_B)
                        && is_identifier_part(c)
                {
                    self.error_at(
                        diag::This_character_cannot_be_escaped_in_a_regular_expression,
                        start,
                        self.scanner_state.pos - start,
                        Vec::new(),
                    );
                }
                // PORT: writes the port form of the rune (see
                // `scanner_util::GO_STRING_MARKER`). An invalid source byte
                // decodes as RuneError, and Go returns "�" too.
                push_js_string_rune(out, c as u32);
            }
        }
    }

    // Go: scanner/scanner.go:1854 scanUnicodeEscape
    // Known to be at \u
    pub(crate) fn scan_unicode_escape(&mut self, should_emit_invalid_escape_error: bool) -> i32 {
        self.scanner_state.pos += 2;
        let start = self.scanner_state.pos;
        let extended = self.char() == '{' as i32;
        let hex_digits;
        if extended {
            self.scanner_state.pos += 1;
            hex_digits = self.scan_hex_digits(1, true, false);
        } else {
            self.scanner_state.token_flags |= TokenFlags::UNICODE_ESCAPE;
            hex_digits = self.scan_hex_digits(4, false, false);
        }
        if hex_digits.is_empty() {
            self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
            if should_emit_invalid_escape_error {
                self.error(diag::Hexadecimal_digit_expected);
            }
            return -1;
        }
        let hex_value = go_parse_int(&hex_digits, 16, 32);
        if extended {
            let mut is_invalid_extended_escape = false;
            if hex_value > 0x10FFFF {
                if should_emit_invalid_escape_error {
                    self.error_at(
                        diag::An_extended_Unicode_escape_value_must_be_between_0x0_and_0x10FFFF_inclusive,
                        start + 1,
                        self.scanner_state.pos - start - 1,
                        Vec::new(),
                    );
                }
                is_invalid_extended_escape = true;
            }
            if self.scanner_state.pos >= self.end {
                if should_emit_invalid_escape_error {
                    self.error(diag::Unexpected_end_of_text);
                }
                is_invalid_extended_escape = true;
            } else if self.char() == '}' as i32 {
                self.scanner_state.pos += 1;
            } else {
                if should_emit_invalid_escape_error {
                    self.error(diag::Unterminated_Unicode_escape_sequence);
                }
                is_invalid_extended_escape = true;
            }
            if is_invalid_extended_escape {
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_ESCAPE;
                return -1;
            }
            self.scanner_state.token_flags |= TokenFlags::EXTENDED_UNICODE_ESCAPE;
        }
        hex_value as i32
    }

    // Go: scanner/scanner.go:1911 scanLowSurrogateEscape
    // scanLowSurrogateEscape attempts to consume a low-surrogate Unicode escape
    // (either '\uLow' or '\u{Low}') immediately following an already-scanned high
    // surrogate and combine them into a single supplementary code point. This
    // mirrors how adjacent UTF-16 code units form a surrogate pair in a JavaScript
    // string, regardless of which escape syntax produced each half. On success it
    // returns the combined code point and true; otherwise it restores the scanner
    // position and returns false.
    // PORT: `(rune, bool)` returns `Option<i32>`.
    pub(crate) fn scan_low_surrogate_escape(&mut self, high: i32) -> Option<i32> {
        if self.char() != '\\' as i32 || self.char_at(1) != 'u' as i32 {
            return None;
        }
        let saved_pos = self.scanner_state.pos;
        let saved_token_flags = self.scanner_state.token_flags;
        // Speculatively scan the escape with diagnostics suppressed: if it isn't a
        // low surrogate we rewind below, and the caller re-scans the same escape and
        // reports any error then, so reporting here would duplicate diagnostics.
        let low = self.scan_unicode_escape(false);
        if low >= 0 && is_low_surrogate(low as u32) {
            return Some(surrogate_pair_to_code_point(high as u32, low as u32) as i32);
        }
        self.scanner_state.pos = saved_pos;
        self.scanner_state.token_flags = saved_token_flags;
        None
    }

    // Go: scanner/scanner.go:1931 peekUnicodeEscape
    // Current character is known to be a backslash. Check for Unicode escape of the form '\uXXXX'
    // or '\u{XXXXXX}' and return code point value if valid Unicode escape is found. Otherwise return -1.
    pub(crate) fn peek_unicode_escape(&mut self) -> i32 {
        if self.char_at(1) == 'u' as i32 {
            let save_pos = self.scanner_state.pos;
            let save_token_flags = self.scanner_state.token_flags;
            let code_point = self.scan_unicode_escape(false);
            self.scanner_state.pos = save_pos;
            self.scanner_state.token_flags = save_token_flags;
            return code_point;
        }
        -1
    }

    // Go: scanner/scanner.go:1943 scanNumber
    pub(crate) fn scan_number(&mut self) -> SyntaxKind {
        let mut start = self.scanner_state.pos;
        let fixed_part;
        if self.char() == '0' as i32 {
            self.scanner_state.pos += 1;
            if self.char() == '_' as i32 {
                self.scanner_state.token_flags |=
                    TokenFlags::CONTAINS_SEPARATOR | TokenFlags::CONTAINS_INVALID_SEPARATOR;
                self.error_at(
                    diag::Numeric_separators_are_not_allowed_here,
                    self.scanner_state.pos,
                    1,
                    Vec::new(),
                );
                self.scanner_state.pos = start;
                fixed_part = self.scan_number_fragment();
            } else {
                let (digits, is_octal) = self.scan_digits();
                if digits.is_empty() {
                    // The "0" at `start`.
                    fixed_part = NumberFragment {
                        prefix: String::new(),
                        start,
                        end: start + 1,
                    };
                } else if !is_octal {
                    self.scanner_state.token_flags |= TokenFlags::CONTAINS_LEADING_ZERO;
                    fixed_part = NumberFragment {
                        prefix: digits,
                        start,
                        end: start,
                    };
                } else {
                    let val = go_parse_int(&digits, 8, 64);
                    self.set_token_value(&val.to_string());
                    self.scanner_state.token_flags |= TokenFlags::OCTAL;
                    let with_minus = self.scanner_state.token == SyntaxKind::MinusToken;
                    let literal = format!("{}0o{:o}", if with_minus { "-" } else { "" }, val);
                    if with_minus {
                        start -= 1;
                    }
                    self.error_at(
                        diag::Octal_literals_are_not_allowed_Use_the_syntax_0,
                        start,
                        self.scanner_state.pos - start,
                        args![literal],
                    );
                    return SyntaxKind::NumericLiteral;
                }
            }
        } else {
            fixed_part = self.scan_number_fragment();
        }
        let fixed_part_end = self.scanner_state.pos;
        let mut fractional_part = None;
        let mut exponent_preamble = (0, 0);
        let mut exponent_part = None;
        if self.char() == '.' as i32 {
            self.scanner_state.pos += 1;
            fractional_part = Some(self.scan_number_fragment());
        }
        let mut end = self.scanner_state.pos;
        if self.char() == 'E' as i32 || self.char() == 'e' as i32 {
            self.scanner_state.pos += 1;
            self.scanner_state.token_flags |= TokenFlags::SCIENTIFIC;
            if self.char() == '+' as i32 || self.char() == '-' as i32 {
                self.scanner_state.pos += 1;
            }
            let start_numeric_part = self.scanner_state.pos;
            let part = self.scan_number_fragment();
            if part.is_empty() {
                self.error(diag::Digit_expected);
            } else {
                exponent_preamble = (end as usize, start_numeric_part as usize);
                end = self.scanner_state.pos;
                exponent_part = Some(part);
            }
        }
        if self
            .scanner_state
            .token_flags
            .intersects(TokenFlags::CONTAINS_SEPARATOR)
        {
            let mut value = String::new();
            fixed_part.push_to(&self.text, &mut value);
            if let Some(part) = fractional_part.filter(|part| !part.is_empty()) {
                value.push('.');
                part.push_to(&self.text, &mut value);
            }
            if let Some(part) = exponent_part {
                value.push_str(&self.text[exponent_preamble.0..exponent_preamble.1]);
                part.push_to(&self.text, &mut value);
            }
            self.set_token_value(&value);
        } else {
            self.scanner_state.token_value = self.text_token_value(start as usize, end as usize);
        }
        if self
            .scanner_state
            .token_flags
            .intersects(TokenFlags::CONTAINS_LEADING_ZERO)
        {
            self.error_at(
                diag::Decimals_with_leading_zeros_are_not_allowed,
                start,
                self.scanner_state.pos - start,
                Vec::new(),
            );
            self.set_token_value(&js_number_string(self.scanner_state.token_value));
            return SyntaxKind::NumericLiteral;
        }
        let result;
        if fixed_part_end == self.scanner_state.pos {
            result = self.scan_big_int_suffix();
        } else {
            self.set_token_value(&js_number_string(self.scanner_state.token_value));
            result = SyntaxKind::NumericLiteral;
        }
        let (ch, _) = self.char_and_size();
        if is_identifier_start(rune_to_char(ch)) {
            let id_start = self.scanner_state.pos;
            let id = self.scan_identifier_parts(IdentifierVariant::STANDARD);
            if result != SyntaxKind::BigIntLiteral
                && id.len() == 1
                && self.text.as_bytes()[id_start as usize] == b'n'
            {
                if self
                    .scanner_state
                    .token_flags
                    .intersects(TokenFlags::SCIENTIFIC)
                {
                    self.error_at(
                        diag::A_bigint_literal_cannot_use_exponential_notation,
                        start,
                        self.scanner_state.pos - start,
                        Vec::new(),
                    );
                    return result;
                }
                if fixed_part_end < id_start {
                    self.error_at(
                        diag::A_bigint_literal_must_be_an_integer,
                        start,
                        self.scanner_state.pos - start,
                        Vec::new(),
                    );
                    return result;
                }
            }
            self.error_at(
                diag::An_identifier_or_keyword_cannot_immediately_follow_a_numeric_literal,
                id_start,
                self.scanner_state.pos - id_start,
                Vec::new(),
            );
            self.scanner_state.pos = id_start;
        }
        result
    }

    // Go: scanner/scanner.go:2043 scanNumberFragment
    // PORT: returns a `NumberFragment` so a fragment without separators
    // needs no `String`.
    pub(crate) fn scan_number_fragment(&mut self) -> NumberFragment {
        let mut start = self.scanner_state.pos;
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        let mut result = String::new();
        loop {
            let before = self.scanner_state.pos;
            self.scan_ascii_while(|b: u8| b.is_ascii_digit());
            if self.scanner_state.pos > before {
                allow_separator = true;
                is_previous_token_separator = false;
            }
            let ch = self.char();
            if ch == '_' as i32 {
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                    result.push_str(&self.text[start as usize..self.scanner_state.pos as usize]);
                } else {
                    self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_SEPARATOR;
                    if is_previous_token_separator {
                        self.error_at(
                            diag::Multiple_consecutive_numeric_separators_are_not_permitted,
                            self.scanner_state.pos,
                            1,
                            Vec::new(),
                        );
                    } else {
                        self.error_at(
                            diag::Numeric_separators_are_not_allowed_here,
                            self.scanner_state.pos,
                            1,
                            Vec::new(),
                        );
                    }
                }
                self.scanner_state.pos += 1;
                start = self.scanner_state.pos;
                continue;
            }
            break;
        }
        if is_previous_token_separator {
            self.scanner_state.token_flags |= TokenFlags::CONTAINS_INVALID_SEPARATOR;
            self.error_at(
                diag::Numeric_separators_are_not_allowed_here,
                self.scanner_state.pos - 1,
                1,
                Vec::new(),
            );
        }
        NumberFragment {
            prefix: result,
            start,
            end: self.scanner_state.pos,
        }
    }

    // Go: scanner/scanner.go:2089 scanDigits
    // PORT: returns an owned `String` (Go returns a substring of the text).
    pub(crate) fn scan_digits(&mut self) -> (String, bool) {
        let start = self.scanner_state.pos;
        let mut is_octal = true;
        while is_digit(rune_to_char(self.char())) {
            if !is_octal_digit(rune_to_char(self.char())) {
                is_octal = false;
            }
            self.scanner_state.pos += 1;
        }
        (
            self.text[start as usize..self.scanner_state.pos as usize].to_string(),
            is_octal,
        )
    }

    // Go: scanner/scanner.go:2101 scanHexDigits
    pub(crate) fn scan_hex_digits(
        &mut self,
        min_count: i32,
        scan_as_many_as_possible: bool,
        can_have_separators: bool,
    ) -> &'static str {
        let mut digit_count = 0;
        let start = self.scanner_state.pos;
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        while digit_count < min_count || scan_as_many_as_possible {
            let ch = self.char();
            if is_hex_digit(rune_to_char(ch)) {
                allow_separator = can_have_separators;
                is_previous_token_separator = false;
                digit_count += 1;
            } else if can_have_separators && ch == '_' as i32 {
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                } else if is_previous_token_separator {
                    self.error_at(
                        diag::Multiple_consecutive_numeric_separators_are_not_permitted,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                } else {
                    self.error_at(
                        diag::Numeric_separators_are_not_allowed_here,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                }
            } else {
                break;
            }
            self.scanner_state.pos += 1;
        }
        if is_previous_token_separator {
            self.error_at(
                diag::Numeric_separators_are_not_allowed_here,
                self.scanner_state.pos - 1,
                1,
                Vec::new(),
            );
        }
        if digit_count < min_count {
            return "";
        }
        let original = &self.text[start as usize..self.scanner_state.pos as usize];
        // PORT: Go allocates `hexDigitCache` on first use; the Rust map is
        // always allocated. The lookup borrows the text; the key is copied
        // and the digits interned only on a miss.
        if let Some(&cached) = self.hex_digit_cache.get(original) {
            cached
        } else {
            let mut digits = original.to_string();
            if self
                .scanner_state
                .token_flags
                .intersects(TokenFlags::CONTAINS_SEPARATOR)
            {
                digits = digits.replace('_', "");
            }
            digits = digits.to_ascii_lowercase(); // standardize hex literals to lowercase
            let digits = intern_token_value(&digits);
            self.hex_digit_cache.insert(original.into(), digits);
            digits
        }
    }

    // Go: scanner/scanner.go:2150 scanBinaryOrOctalDigits
    pub(crate) fn scan_binary_or_octal_digits(&mut self, base: i32) -> String {
        let mut sb = String::new();
        let mut allow_separator = false;
        let mut is_previous_token_separator = false;
        loop {
            let ch = self.char();
            if is_digit(rune_to_char(ch)) && ch - ('0' as i32) < base {
                sb.push(rune_to_char(ch));
                allow_separator = true;
                is_previous_token_separator = false;
            } else if ch == '_' as i32 {
                self.scanner_state.token_flags |= TokenFlags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    is_previous_token_separator = true;
                } else if is_previous_token_separator {
                    self.error_at(
                        diag::Multiple_consecutive_numeric_separators_are_not_permitted,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                } else {
                    self.error_at(
                        diag::Numeric_separators_are_not_allowed_here,
                        self.scanner_state.pos,
                        1,
                        Vec::new(),
                    );
                }
            } else {
                break;
            }
            self.scanner_state.pos += 1;
        }
        if is_previous_token_separator {
            self.error_at(
                diag::Numeric_separators_are_not_allowed_here,
                self.scanner_state.pos - 1,
                1,
                Vec::new(),
            );
        }
        sb
    }

    // Go: scanner/scanner.go:2181 scanBigIntSuffix
    pub(crate) fn scan_big_int_suffix(&mut self) -> SyntaxKind {
        if self.char() == 'n' as i32 {
            let mut value = format!("{}n", self.scanner_state.token_value);
            if self
                .scanner_state
                .token_flags
                .intersects(TokenFlags::BINARY_OR_OCTAL_SPECIFIER)
            {
                // PORT: `crate::jsnum::parse_pseudo_big_int` strips the trailing
                // `n` that Go `ParsePseudoBigInt` also ignores.
                value = crate::jsnum::parse_pseudo_big_int(&value) + "n";
            }
            self.set_token_value(&value);
            self.scanner_state.pos += 1;
            return SyntaxKind::BigIntLiteral;
        }
        // PORT: a canonical integer keeps its token value, as Go's
        // `jsnum` round trip would give the same text.
        let token_value = self.scanner_state.token_value;
        if is_canonical_js_integer(token_value) {
            return SyntaxKind::NumericLiteral;
        }
        // PORT: Go allocates `numberCache` on first use; the Rust map is
        // always allocated.
        if let Some(&cached) = self.number_cache.get(token_value) {
            self.scanner_state.token_value = cached;
        } else {
            // Go: `if tokenValue == s.tokenValue { tokenValue = s.tokenValue }`
            // only shares the string memory; the value is the same.
            let value = intern_token_value(&js_number_string(token_value));
            self.number_cache.insert(token_value, value);
            self.scanner_state.token_value = value;
        }
        SyntaxKind::NumericLiteral
    }

    // Go: scanner/scanner.go:2206 scanInvalidCharacter
    pub(crate) fn scan_invalid_character(&mut self) {
        let (_, size) = self.char_and_size();
        self.error_at(
            diag::Invalid_character,
            self.scanner_state.pos,
            size,
            Vec::new(),
        );
        self.scanner_state.pos += size;
        self.scanner_state.token = SyntaxKind::Unknown;
    }
}

#[cfg(test)]
mod tests {
    use super::{is_canonical_js_integer, js_number_string};
    use crate::frontend::scanner::scanner_p1::{IdentifierVariant, new_scanner};
    use crate::scanner_util::get_identifier_token;

    /// scanintern1: where `scan_ascii_identifier` takes a name, it gives what
    /// `scan_identifier` and `get_identifier_token` give from the same state
    /// (kind, end and token value). Names of 1 to 40 bytes (keywords too),
    /// each byte kind before each kind of stop, at offsets that put the name
    /// across the 16-byte window, with the scan end at, after and before the
    /// name's end.
    #[test]
    fn ascii_identifier_fast_path_matches_scan_identifier() {
        let mut names: Vec<String> = ["as", "if", "of", "type", "async", "constructor", "x"]
            .map(String::from)
            .to_vec();
        for len in 1..=40 {
            let name: String = (0..len)
                .map(|i| char::from(b"aZ_$9qB0zA"[(i * 7 + len) % 10]))
                .collect();
            if name.as_bytes()[0].is_ascii_digit() {
                continue;
            }
            names.push(name);
        }
        names.push("instanceof1".to_string());
        names.push("\u{e9}t\u{e9}".to_string());
        names.push("9lives".to_string());
        let stops = ["", " ", "(", "\\u0061", "\u{e9}", "\u{2028}", "-x", "#"];
        let mut taken = 0;
        for name in &names {
            for stop in stops {
                for pad in 0..18 {
                    let text = format!("{}{name}{stop} tail of the text", " ".repeat(pad));
                    let name_end = (pad + name.len()) as i32;
                    for end in [text.len() as i32, name_end + 1, name_end, name_end - 1] {
                        if end < pad as i32 {
                            continue;
                        }
                        let mut fast = new_scanner();
                        fast.set_text(&text);
                        fast.end = end;
                        fast.scanner_state.pos = pad as i32;
                        let Some(kind) = fast.scan_ascii_identifier() else {
                            assert_eq!(fast.scanner_state.pos, pad as i32, "{text:?} {end}");
                            continue;
                        };
                        taken += 1;
                        let mut slow = new_scanner();
                        slow.set_text(&text);
                        slow.end = end;
                        slow.scanner_state.pos = pad as i32;
                        assert!(slow.scan_identifier(0, IdentifierVariant::STANDARD));
                        let want = get_identifier_token(slow.scanner_state.token_value);
                        assert_eq!(kind, want, "{text:?} {end}");
                        assert_eq!(
                            fast.scanner_state.pos, slow.scanner_state.pos,
                            "{text:?} {end}"
                        );
                        assert_eq!(
                            fast.scanner_state.token_value, slow.scanner_state.token_value,
                            "{text:?} {end}"
                        );
                    }
                }
            }
        }
        assert!(taken > 1000, "{taken}");
    }

    #[test]
    fn canonical_integers_round_trip() {
        let samples = (0..1_000_000u64)
            .chain((0..1000u64).map(|i| 999_999_999_999_999 - i * 7_919_007_919))
            .map(|n| n.to_string())
            .chain(["00", "01", "1e3", "0x10", "", "1234567890123456"].map(String::from));
        for text in samples {
            if is_canonical_js_integer(&text) {
                assert_eq!(js_number_string(&text), text);
            }
        }
        assert!(!is_canonical_js_integer("01"));
        assert!(!is_canonical_js_integer("1234567890123456"));
    }
}
