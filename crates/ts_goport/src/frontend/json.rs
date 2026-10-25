//! Port of json/json.go.
//!
//! Go `internal/json` is a thin wrapper over `github.com/go-json-experiment/json`
//! (JSON v2) and its `jsontext` package. This crate has no JSON dependency, so
//! the parts of v2 that the compiler reads through are ported here by hand:
//!
//! - `JsonDecoder` is the `jsontext.Decoder` token state machine: strict RFC
//!   8259 grammar, `:` and `,` checks in `PeekKind`, a trailing comma is an
//!   error, strings reject control characters and lone surrogates, and object
//!   names are checked for duplicates unless `AllowDuplicateNames` is set or
//!   the arshaler disabled the namespace.
//! - `UnmarshalerFrom` stands in for both Go `json.UnmarshalerFrom` and the
//!   v2 default arshalers. The impls for `String`, `bool`, `f64` and
//!   `FxHashMap<String, V>` follow the v2 default arshalers (null gives the
//!   zero value, a kind mismatch is an error, maps merge into the existing map).
//! - `MarshalerTo` stands in for the v2 default marshalers of the value kinds
//!   the compiler marshals (strings, booleans, slices and ordered maps).
//!
//! PORT: without legacy flags every v2 unmarshal error is fatal
//! (`isFatalError`), so every impl returns the first error. The decoder's
//! errors are Go's `jsontext.SyntacticError` texts (prefix, JSON pointer and
//! offset), which the LSP and the API show (`SyntaxErr`). Most unmarshal
//! errors of the impls in this file are not the v2 texts; no caller shows
//! them. The exceptions are the ones the LSP and the API show: a kind
//! mismatch of a string, a boolean or a number is a v2 `SemanticError`
//! (`json_ext::unmarshal_kind_error`), and `json_unmarshal_decode` gives a
//! method's plain error the Go type (as the v2 arshaler of a type with a
//! method does). `json_ext` has the v2 `SemanticError` texts for the LSP and
//! API types.

use crate::frontend::prelude::*;
use std::borrow::Cow;

/// Go JSON v2 `*json.SemanticError` / `*jsontext.SyntacticError`.
/// PORT: one error type with a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub message: String,
}

impl JsonError {
    fn new(message: impl Into<String>) -> JsonError {
        JsonError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Go `json.Options` values that the compiler passes.
/// PORT: Go options are opaque values; this enum lists the ones used here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonOption {
    AllowDuplicateNames(bool),
    AllowInvalidUtf8(bool),
}

/// Resolved decoder and encoder flags.
// PORT: `port_form` is not a Go option. A decoded string is a Go string, so
// the decoder writes it in the port form (see
// `scanner_util::GO_STRING_MARKER`): each real U+FDD0 becomes two. With
// `port_form`, the JSON text already holds port form strings (the port's
// own build worker protocol, `append_json_quote_port_form`), and each char
// is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsonOptions {
    pub allow_duplicate_names: bool,
    pub allow_invalid_utf8: bool,
    pub port_form: bool,
}

impl JsonOptions {
    fn from_options(opts: &[JsonOption]) -> JsonOptions {
        let mut o = JsonOptions::default();
        for opt in opts {
            match *opt {
                JsonOption::AllowDuplicateNames(v) => o.allow_duplicate_names = v,
                JsonOption::AllowInvalidUtf8(v) => o.allow_invalid_utf8 = v,
            }
        }
        o
    }
}

/// Go `jsontext.Token`, as returned by `Decoder.ReadToken`.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonToken {
    Null,
    False,
    True,
    String(String),
    /// The raw number text.
    Number(String),
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}

impl JsonToken {
    /// Go `jsontext.Token.Kind`.
    #[must_use]
    pub fn kind(&self) -> u8 {
        match self {
            JsonToken::Null => b'n',
            JsonToken::False => b'f',
            JsonToken::True => b't',
            JsonToken::String(_) => b'"',
            JsonToken::Number(_) => b'0',
            JsonToken::BeginObject => b'{',
            JsonToken::EndObject => b'}',
            JsonToken::BeginArray => b'[',
            JsonToken::EndArray => b']',
        }
    }
}

// Go jsontext `maxNestingDepth`.
const MAX_NESTING_DEPTH: usize = 10000;

// One open JSON object or array (Go jsontext `stateEntry` plus the
// namespace of seen object names).
struct Frame<'a> {
    is_object: bool,
    len: usize,
    // `None` when duplicate names are allowed or the namespace is disabled.
    // PERF: a name is its unquoted text before the port form conversion
    // (`scan_string`), which is one to one, so equal names are still equal.
    // A name with no escape borrows the input.
    names: Option<FxHashSet<Cow<'a, str>>>,
    // The offset of the opening quote of the last object name read in this
    // object (Go `objectNameStack`). Only the error texts read it.
    name: usize,
}

/// PORT: not in Go (perf). A `JsonToken` whose text borrows the input
/// where it can (`JsonDecoder::read_token_ref`): a string with no escape
/// and no port form change, and each number.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonTokenRef<'a> {
    Null,
    False,
    True,
    String(Cow<'a, str>),
    /// The raw number text.
    Number(&'a str),
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}

/// A token of `JsonDecoder::next_token`: a `JsonToken` whose string is the
/// unquoted text before the port form conversion, and whose number is the
/// raw text, both borrowed from the input when they can be.
enum RawToken<'a> {
    Null,
    False,
    True,
    String(Cow<'a, str>),
    Number(&'a str),
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
}

/// Go `jsontext.Decoder` over a complete input buffer.
pub struct JsonDecoder<'a> {
    buf: &'a [u8],
    // Go `prevEnd`: the end of the last read token or value.
    pos: usize,
    // Open containers. The top-level virtual array is `top_len`.
    stack: Vec<Frame<'a>>,
    top_len: usize,
    pub options: JsonOptions,
    // PERF: the name sets of closed objects, empty, for the next objects.
    spare_names: Vec<FxHashSet<Cow<'a, str>>>,
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

// Go `jsontext.Kind.normalize`.
fn normalize_kind(c: u8) -> u8 {
    match c {
        b'-' | b'0'..=b'9' => b'0',
        _ => c,
    }
}

fn hex_val(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'a'..=b'f' => Some(u32::from(c - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(c - b'A' + 10)),
        _ => None,
    }
}

// Decodes one UTF-8 character at the start of `b`.
fn decode_utf8(b: &[u8]) -> Option<(char, usize)> {
    let n = match b.first()? {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let s = std::str::from_utf8(b.get(..n)?).ok()?;
    s.chars().next().map(|c| (c, n))
}

impl<'a> JsonDecoder<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8], options: JsonOptions) -> JsonDecoder<'a> {
        JsonDecoder {
            buf,
            pos: 0,
            stack: Vec::new(),
            top_len: 0,
            options,
            spare_names: Vec::new(),
        }
    }

    fn skip_ws(&self, mut p: usize) -> usize {
        while p < self.buf.len() && is_ws(self.buf[p]) {
            p += 1;
        }
        p
    }

    fn last_len(&self) -> usize {
        self.stack.last().map_or(self.top_len, |f| f.len)
    }

    fn last_is_object(&self) -> bool {
        self.stack.last().is_some_and(|f| f.is_object)
    }

    fn need_object_value(&self) -> bool {
        self.last_is_object() && self.last_len() % 2 == 1
    }

    fn need_object_name(&self) -> bool {
        self.last_is_object() && self.last_len().is_multiple_of(2)
    }

    fn increment(&mut self) {
        match self.stack.last_mut() {
            Some(f) => f.len += 1,
            None => self.top_len += 1,
        }
    }

    // Go jsontext `stateMachine.needDelim`.
    fn need_delim(&self, next: u8) -> u8 {
        if self.need_object_value() {
            b':'
        } else if self.last_len() > 0 && next != b'}' && next != b']' && !self.stack.is_empty() {
            b','
        } else {
            0
        }
    }

    // Go jsontext `decoderState.PeekKind` without the cache: the position and
    // normalized kind of the next token, after whitespace and one delimiter.
    // The errors are Go's (the decoder reads a complete buffer, so Go's
    // `fetch` fails at its end with `io.ErrUnexpectedEOF`).
    fn peek_pos(&self) -> Result<(usize, u8), JsonError> {
        let mut p = self.skip_ws(self.pos);
        if p >= self.buf.len() {
            return Err(self.eof_before_token(p));
        }
        let mut delim = 0;
        let c = self.buf[p];
        if c == b':' || c == b',' {
            delim = c;
            p = self.skip_ws(p + 1);
            if p >= self.buf.len() {
                return Err(self.eof_after_delim(delim, p));
            }
        }
        let next = normalize_kind(self.buf[p]);
        if self.need_delim(next) != delim {
            return Err(self.check_delim(delim, next));
        }
        Ok((p, next))
    }

    /// Go `jsontext.Decoder.PeekKind`. Returns 0 on error.
    #[must_use]
    pub fn peek_kind(&self) -> u8 {
        self.peek_pos().map_or(0, |(_, k)| k)
    }

    /// Go `jsontext.Decoder.InputOffset`: the end of the last token or value
    /// read.
    #[must_use]
    pub fn input_offset(&self) -> usize {
        self.pos
    }

    /// The whole input of the decoder.
    // PORT: not in Go. The lsproto union decoder finds the JSON pointer of
    // an error in a member that it read earlier (`json_ext::error_pointer`).
    #[must_use]
    pub fn input(&self) -> &'a [u8] {
        self.buf
    }

    /// Go `jsontext.Decoder.DisableNamespace` (the export helper
    /// `Tokens.Last.DisableNamespace`): stop duplicate-name checks for the
    /// innermost open object.
    pub fn disable_namespace(&mut self) {
        if let Some(names) = self.stack.last_mut().and_then(|f| f.names.take()) {
            self.keep_names(names);
        }
    }

    /// Keeps the emptied name set `names` for a later object.
    fn keep_names(&mut self, mut names: FxHashSet<Cow<'a, str>>) {
        names.clear();
        self.spare_names.push(names);
    }

    /// Go `jsontext.Decoder.StackDepth` plus the length of the innermost
    /// container. Used to check that an unmarshaler reads exactly one value.
    fn depth_length(&self) -> (usize, usize) {
        (self.stack.len(), self.last_len())
    }

    // Go `stateMachine.appendLiteral` / `appendNumber` for the token that
    // starts at `start`.
    fn append_value(&mut self, start: usize) -> Result<(), JsonError> {
        if self.need_object_name() {
            return Err(self.syntactic_error(SyntaxErr::NonStringName, start, 1, &[]));
        }
        self.increment();
        Ok(())
    }

    /// Go `jsontext.Decoder.ReadToken`.
    pub fn read_token(&mut self) -> Result<JsonToken, JsonError> {
        Ok(match self.next_token()? {
            RawToken::Null => JsonToken::Null,
            RawToken::False => JsonToken::False,
            RawToken::True => JsonToken::True,
            RawToken::String(s) => JsonToken::String(self.string_value(s)),
            RawToken::Number(raw) => JsonToken::Number(raw.to_string()),
            RawToken::BeginObject => JsonToken::BeginObject,
            RawToken::EndObject => JsonToken::EndObject,
            RawToken::BeginArray => JsonToken::BeginArray,
            RawToken::EndArray => JsonToken::EndArray,
        })
    }

    /// PORT: not in Go (perf). `read_token` as a `JsonTokenRef`: the same
    /// token, checks and errors, without a copy of text that it can borrow.
    pub fn read_token_ref(&mut self) -> Result<JsonTokenRef<'a>, JsonError> {
        Ok(match self.next_token()? {
            RawToken::Null => JsonTokenRef::Null,
            RawToken::False => JsonTokenRef::False,
            RawToken::True => JsonTokenRef::True,
            RawToken::String(s) => JsonTokenRef::String(
                if self.options.port_form || !crate::scanner_util::contains_go_string_marker(&s) {
                    s
                } else {
                    Cow::Owned(self.string_value(s))
                },
            ),
            RawToken::Number(raw) => JsonTokenRef::Number(raw),
            RawToken::BeginObject => JsonTokenRef::BeginObject,
            RawToken::EndObject => JsonTokenRef::EndObject,
            RawToken::BeginArray => JsonTokenRef::BeginArray,
            RawToken::EndArray => JsonTokenRef::EndArray,
        })
    }

    /// The string of a `JsonToken`: the unquoted text `s` in the port form.
    fn string_value(&self, s: Cow<'a, str>) -> String {
        if self.options.port_form {
            return s.into_owned();
        }
        crate::scanner_util::go_string_from_utf8(s.into_owned())
    }

    /// `read_token` without the `JsonToken` copies of the text
    /// (`RawToken`): the same checks, state changes and errors.
    fn next_token(&mut self) -> Result<RawToken<'a>, JsonError> {
        let (p, _) = self.peek_pos()?;
        let c = self.buf[p];
        match c {
            b'n' | b't' | b'f' => {
                let (lit, tok): (&[u8], RawToken<'a>) = match c {
                    b'n' => (b"null", RawToken::Null),
                    b't' => (b"true", RawToken::True),
                    _ => (b"false", RawToken::False),
                };
                if !self.buf[p..].starts_with(lit) {
                    let (pos, err) = consume_literal_error(self.buf, p, lit);
                    return Err(self.syntactic_error(err, pos, 1, &[]));
                }
                self.append_value(p)?;
                self.pos = p + lit.len();
                Ok(tok)
            }
            b'"' => {
                let (end, s) = self.scan_string(p)?;
                if self.need_object_name() {
                    let dup = match &mut self.stack.last_mut().expect("object frame").names {
                        Some(names) => !names.insert(s.clone()),
                        None => false,
                    };
                    if dup {
                        let name = pointer_token_of_name(&self.buf[p..end]);
                        return Err(self.syntactic_error(SyntaxErr::DuplicateName, p, 1, &[name]));
                    }
                    self.stack.last_mut().expect("object frame").name = p;
                }
                self.increment();
                self.pos = end;
                Ok(RawToken::String(s))
            }
            b'-' | b'0'..=b'9' => {
                let end = self.consume_number(p)?;
                self.append_value(p)?;
                self.pos = end;
                let raw = std::str::from_utf8(&self.buf[p..end]).expect("number is ASCII");
                Ok(RawToken::Number(raw))
            }
            b'{' | b'[' => {
                if self.need_object_name() {
                    return Err(self.syntactic_error(SyntaxErr::NonStringName, p, 1, &[]));
                }
                if self.stack.len() == MAX_NESTING_DEPTH {
                    return Err(self.syntactic_error(SyntaxErr::MaxDepth, p, 1, &[]));
                }
                self.increment();
                let is_object = c == b'{';
                let names = if is_object && !self.options.allow_duplicate_names {
                    Some(self.spare_names.pop().unwrap_or_default())
                } else {
                    None
                };
                self.stack.push(Frame {
                    is_object,
                    len: 0,
                    names,
                    name: 0,
                });
                self.pos = p + 1;
                Ok(if is_object {
                    RawToken::BeginObject
                } else {
                    RawToken::BeginArray
                })
            }
            b'}' => {
                if !self.last_is_object() {
                    return Err(self.syntactic_error(SyntaxErr::MismatchDelim, p, 1, &[]));
                }
                if self.need_object_value() {
                    return Err(self.syntactic_error(SyntaxErr::MissingValue, p, 1, &[]));
                }
                if let Some(names) = self.stack.pop().and_then(|f| f.names) {
                    self.keep_names(names);
                }
                self.pos = p + 1;
                Ok(RawToken::EndObject)
            }
            b']' => {
                if self.stack.is_empty() || self.last_is_object() {
                    return Err(self.syntactic_error(SyntaxErr::MismatchDelim, p, 1, &[]));
                }
                self.stack.pop();
                self.pos = p + 1;
                Ok(RawToken::EndArray)
            }
            _ => Err(self.syntactic_error(
                invalid_character(self.buf, p, "at start of value"),
                p,
                1,
                &[],
            )),
        }
    }

    /// Go `jsontext.Decoder.ReadValue`: the raw bytes of the next complete
    /// value (or object name).
    // PORT: the value is read token by token. On an error the state goes
    // back to where it was before the value, as in Go, and the error is the
    // one that Go's `consumeValue` finds (`read_value_error`).
    pub fn read_value(&mut self) -> Result<&'a [u8], JsonError> {
        let (start, _) = self.peek_pos()?;
        let depth = self.stack.len();
        let before = (
            self.pos,
            self.last_len(),
            self.stack.last().map_or(0, |f| f.name),
        );
        let result = (|| {
            let tok = self.next_token()?;
            if matches!(tok, RawToken::BeginObject | RawToken::BeginArray) {
                while self.stack.len() > depth {
                    self.next_token()?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(&self.buf[start..self.pos]),
            Err(err) => Err(self.read_value_error(start, depth, before, err)),
        }
    }

    /// Go `jsontext.Decoder.SkipValue`: an object or array is read token by
    /// token, any other value with `read_value`.
    pub fn skip_value(&mut self) -> Result<(), JsonError> {
        match self.peek_kind() {
            b'{' | b'[' => {
                let depth = self.stack.len();
                loop {
                    self.next_token()?;
                    if self.stack.len() <= depth {
                        return Ok(());
                    }
                }
            }
            _ => self.read_value().map(|_| ()),
        }
    }

    /// Go `jsontext` `decoderState.CheckEOF`: only whitespace may follow the
    /// top-level value.
    pub fn check_eof(&self) -> Result<(), JsonError> {
        let p = self.skip_ws(self.pos);
        if p < self.buf.len() {
            return Err(self.syntactic_error(
                invalid_character(self.buf, p, "after top-level value"),
                p,
                0,
                &[],
            ));
        }
        Ok(())
    }

    // Go `jsonwire.ConsumeString` plus unquoting. Returns the end offset and
    // the unquoted text, before the port form conversion (`string_value`).
    // PERF: a string with no escape and no control character whose bytes
    // are valid UTF-8 is its own unquoted text, so it is borrowed. Any
    // other string, and each error, goes through `consume_string_chars`.
    fn scan_string(&self, p: usize) -> Result<(usize, Cow<'a, str>), JsonError> {
        let b = self.buf;
        let start = p + 1;
        let mut i = start;
        while let Some(&c) = b.get(i) {
            if c == b'"' {
                if let Ok(text) = std::str::from_utf8(&b[start..i]) {
                    return Ok((i + 1, Cow::Borrowed(text)));
                }
                break;
            }
            if c == b'\\' || c < 0x20 {
                break;
            }
            i += 1;
        }
        let (end, out) = self.consume_string_chars(p)?;
        Ok((end, Cow::Owned(out)))
    }

    fn consume_string_chars(&self, p: usize) -> Result<(usize, String), JsonError> {
        self.consume_string_raw(p)
            .map_err(|(pos, err)| self.syntactic_error(err, pos, 1, &[]))
    }

    // Go `jsonwire.ConsumeStringResumable` (with `validateUTF8` unless
    // `allow_invalid_utf8`) plus unquoting. An error is Go's error and its
    // offset (`consumeString`: at the start of a cut escape, and at the end
    // of the input when the string is cut).
    fn consume_string_raw(&self, p: usize) -> Result<(usize, String), (usize, SyntaxErr)> {
        let b = self.buf;
        let validate = !self.options.allow_invalid_utf8;
        if b.get(p) != Some(&b'"') {
            if p >= b.len() {
                return Err((p, SyntaxErr::UnexpectedEof));
            }
            return Err((
                p,
                invalid_character(b, p, "at start of string (expecting '\"')"),
            ));
        }
        let mut out = String::new();
        let mut i = p + 1;
        loop {
            let Some(&c) = b.get(i) else {
                return Err((b.len(), SyntaxErr::UnexpectedEof));
            };
            match c {
                b'"' => return Ok((i + 1, out)),
                b'\\' => {
                    let Some(&e) = b.get(i + 1) else {
                        return Err((i, SyntaxErr::UnexpectedEof));
                    };
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let escape = i;
                            if b.len() < i + 6 {
                                if has_escaped_utf16_prefix(&b[i..], false) {
                                    return Err((escape, SyntaxErr::UnexpectedEof));
                                }
                                return Err((i, invalid_escape(&b[i..])));
                            }
                            let Some(v1) = parse_hex4(&b[i + 2..i + 6]) else {
                                return Err((i, invalid_escape(&b[i..i + 6])));
                            };
                            i += 6;
                            if (0xD800..0xE000).contains(&v1) {
                                // Go checks the pair only with `validateUTF8`.
                                let v2 = if b.len() < i + 6 {
                                    if validate && has_escaped_utf16_prefix(&b[i..], true) {
                                        return Err((escape, SyntaxErr::UnexpectedEof));
                                    }
                                    None
                                } else if b[i] == b'\\' && b[i + 1] == b'u' {
                                    parse_hex4(&b[i + 2..i + 6])
                                } else {
                                    None
                                };
                                let decoded = v2.and_then(|v2| {
                                    ((0xD800..0xDC00).contains(&v1)
                                        && (0xDC00..0xE000).contains(&v2))
                                    .then(|| {
                                        char::from_u32(
                                            0x10000 + ((v1 - 0xD800) << 10) + (v2 - 0xDC00),
                                        )
                                    })
                                    .flatten()
                                });
                                match decoded {
                                    Some(ch) => {
                                        out.push(ch);
                                        i += 6;
                                    }
                                    None if !validate => out.push('\u{FFFD}'),
                                    None => {
                                        let what = &b[escape..(escape + 12).min(b.len())];
                                        return Err((escape, invalid_escape(what)));
                                    }
                                }
                            } else {
                                out.push(char::from_u32(v1).expect("non-surrogate BMP code point"));
                            }
                            continue;
                        }
                        _ => return Err((i, invalid_escape(&b[i..i + 2]))),
                    }
                    i += 2;
                }
                0x00..=0x1F => {
                    return Err((
                        i,
                        invalid_character(b, i, "in string (expecting non-control character)"),
                    ));
                }
                0x20..=0x7F => {
                    // PERF: the run of plain ASCII bytes up to the next quote,
                    // backslash, control or non-ASCII byte in one copy.
                    let start = i;
                    i += 1;
                    while let Some(&c) = b.get(i) {
                        if c == b'"' || c == b'\\' || c < 0x20 || c >= 0x80 {
                            break;
                        }
                        i += 1;
                    }
                    out.push_str(std::str::from_utf8(&b[start..i]).expect("ASCII run"));
                }
                _ => match decode_utf8(&b[i..]) {
                    Some((ch, n)) => {
                        out.push(ch);
                        i += n;
                    }
                    None if !go_full_rune(&b[i..]) => {
                        return Err((i, SyntaxErr::UnexpectedEof));
                    }
                    None if !validate => {
                        out.push('\u{FFFD}');
                        i += 1;
                    }
                    None => return Err((i, SyntaxErr::InvalidUtf8)),
                },
            }
        }
    }

    // Go `jsonwire.ConsumeNumber`: `-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?`.
    fn consume_number(&self, p: usize) -> Result<usize, JsonError> {
        consume_number_raw(self.buf, p).map_err(|(pos, err)| self.syntactic_error(err, pos, 1, &[]))
    }
}

// ---------------------------------------------------------------------------
// Go jsontext syntactic errors
// ---------------------------------------------------------------------------

/// The `Err` of a Go `jsontext.SyntacticError` that the decoder makes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SyntaxErr {
    /// Go `jsonwire.InvalidTextError`: its `Label`, `What` (the raw input
    /// text) and `Where`.
    InvalidText {
        label: &'static str,
        what: Vec<u8>,
        at: String,
    },
    /// Go `io.ErrUnexpectedEOF`.
    UnexpectedEof,
    /// Go `jsonwire.ErrInvalidUTF8`.
    InvalidUtf8,
    /// Go `jsontext.ErrDuplicateName`.
    DuplicateName,
    /// Go `jsontext.ErrNonStringName`.
    NonStringName,
    /// Go jsontext `errMissingValue`.
    MissingValue,
    /// Go jsontext `errMismatchDelim`.
    MismatchDelim,
    /// Go jsontext `errMaxDepth`.
    MaxDepth,
}

/// A `SyntaxErr` of Go's `consumeValue`, its offset, and the pointer
/// tokens (outermost first) that Go's `pointerSuffixError` adds.
type ConsumeError = (usize, SyntaxErr, Vec<String>);

impl SyntaxErr {
    // Go `err.Error()`.
    fn text(&self) -> String {
        match self {
            SyntaxErr::InvalidText { label, what, at } => invalid_text_error(label, what, at),
            SyntaxErr::UnexpectedEof => "unexpected EOF".to_string(),
            SyntaxErr::InvalidUtf8 => "invalid UTF-8".to_string(),
            SyntaxErr::DuplicateName => "duplicate object member name".to_string(),
            SyntaxErr::NonStringName => "object member name must be a string".to_string(),
            SyntaxErr::MissingValue => "missing value after object name".to_string(),
            SyntaxErr::MismatchDelim => {
                "mismatching structural token for object or array".to_string()
            }
            SyntaxErr::MaxDepth => "exceeded max depth".to_string(),
        }
    }
}

// Go: jsonwire/wire.go:129 NewInvalidCharacterError: the first rune of
// `buf[pos..]` (one byte when it is not valid UTF-8).
fn invalid_character(buf: &[u8], pos: usize, at: impl Into<String>) -> SyntaxErr {
    let rest = &buf[pos.min(buf.len())..];
    SyntaxErr::InvalidText {
        label: "character",
        what: rest[..go_decode_rune_len(rest)].to_vec(),
        at: at.into(),
    }
}

// Go: jsonwire/wire.go:134 NewInvalidEscapeSequenceError
fn invalid_escape(what: &[u8]) -> SyntaxErr {
    SyntaxErr::InvalidText {
        label: if what.len() > 6 {
            "surrogate pair"
        } else {
            "escape sequence"
        },
        what: what.to_vec(),
        at: "in string".to_string(),
    }
}

// Go: jsonwire/wire.go:146 (*InvalidTextError).Error
fn invalid_text_error(label: &str, what: &[u8], at: &str) -> String {
    let mut runes = 0;
    let mut need_escape = false;
    let mut i = 0;
    while i < what.len() {
        let n = go_decode_rune_len(&what[i..]);
        match decode_utf8(&what[i..]) {
            Some((c, _)) => {
                need_escape |= c == '`'
                    || c == '\u{FFFD}'
                    || c.is_whitespace()
                    || !crate::gostd::strconv::is_print(c);
            }
            None => need_escape = true,
        }
        runes += 1;
        i += n;
    }
    let quoted = if runes == 1 {
        go_quote_rune_bytes(what)
    } else if need_escape {
        crate::gostd::strconv::quote_bytes(what)
    } else {
        format!("`{}`", String::from_utf8_lossy(what))
    };
    let text = format!("invalid {label} {quoted} {at}");
    match text.strip_suffix(' ') {
        Some(t) => t.to_string(),
        None => text,
    }
}

// Go `utf8.DecodeRune(b)` size: 0 for no input, 1 for a byte that does not
// start a valid rune.
fn go_decode_rune_len(b: &[u8]) -> usize {
    if b.is_empty() {
        return 0;
    }
    decode_utf8(b).map_or(1, |(_, n)| n)
}

// Go: utf8.FullRune
fn go_full_rune(b: &[u8]) -> bool {
    let Some(&c) = b.first() else {
        return false;
    };
    let (need, lo, hi) = match c {
        0xC2..=0xDF => (2, 0x80, 0xBF),
        0xE0 => (3, 0xA0, 0xBF),
        0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
        0xED => (3, 0x80, 0x9F),
        0xF0 => (4, 0x90, 0xBF),
        0xF1..=0xF3 => (4, 0x80, 0xBF),
        0xF4 => (4, 0x80, 0x8F),
        // ASCII, or a byte that cannot start a rune.
        _ => return true,
    };
    if b.len() >= need {
        return true;
    }
    // Short or invalid.
    (b.len() > 1 && !(lo..=hi).contains(&b[1])) || (b.len() > 2 && !(0x80..=0xBF).contains(&b[2]))
}

// Go: jsonwire/wire.go:62 QuoteRune
fn go_quote_rune_bytes(b: &[u8]) -> String {
    match decode_utf8(b) {
        Some((c, _)) => crate::gostd::strconv::quote_rune(c),
        None if b.is_empty() => crate::gostd::strconv::quote_rune('\u{FFFD}'),
        None => format!("'\\x{:x}'", b[0]),
    }
}

// Go: jsonwire/decode.go:299 hasEscapedUTF16Prefix
fn has_escaped_utf16_prefix(b: &[u8], lower_surrogate_half: bool) -> bool {
    for (i, &c) in b.iter().enumerate() {
        let ok = match i {
            0 => c == b'\\',
            1 => c == b'u',
            2 if lower_surrogate_half => c == b'd' || c == b'D',
            3 if lower_surrogate_half => matches!(c, b'c'..=b'f' | b'C'..=b'F'),
            _ => true,
        };
        if !ok || ((2..6).contains(&i) && hex_val(c).is_none()) {
            return false;
        }
    }
    true
}

// Go: jsonwire parseHexUint16
fn parse_hex4(b: &[u8]) -> Option<u32> {
    b.iter().try_fold(0, |v, &c| hex_val(c).map(|d| v * 16 + d))
}

// Go `ConsumeLiteral` of the literal `lit` at `p`, which is not all there:
// the offset and error of `consumeLiteral`.
fn consume_literal_error(b: &[u8], p: usize, lit: &[u8]) -> (usize, SyntaxErr) {
    let rest = &b[p..];
    for i in 0..rest.len().min(lit.len()) {
        if rest[i] != lit[i] {
            let at = format!(
                "in literal {} (expecting {})",
                std::str::from_utf8(lit).expect("ASCII literal"),
                crate::gostd::strconv::quote_rune(char::from(lit[i]))
            );
            return (p + i, invalid_character(b, p + i, at));
        }
    }
    (b.len(), SyntaxErr::UnexpectedEof)
}

// Go `jsonwire.ConsumeNumberResumable` through `consumeNumber`: the end
// offset, or the offset and error. A number that the input cuts short is
// `io.ErrUnexpectedEOF` at the start of the number.
fn consume_number_raw(b: &[u8], p: usize) -> Result<usize, (usize, SyntaxErr)> {
    fn digits(b: &[u8], mut i: usize) -> usize {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        i
    }
    fn expect_digit(b: &[u8], p: usize, i: usize) -> Result<usize, (usize, SyntaxErr)> {
        match b.get(i) {
            None => Err((p, SyntaxErr::UnexpectedEof)),
            Some(c) if c.is_ascii_digit() => Ok(digits(b, i + 1)),
            Some(_) => Err((i, invalid_character(b, i, "in number (expecting digit)"))),
        }
    }
    let mut i = p;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    i = if b.get(i) == Some(&b'0') {
        i + 1
    } else {
        expect_digit(b, p, i)?
    };
    if b.get(i) == Some(&b'.') {
        i = expect_digit(b, p, i + 1)?;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        i = expect_digit(b, p, i)?;
    }
    Ok(i)
}

// Go `appendEscapePointerName`: a JSON pointer token for an object name.
fn escape_pointer_token(name: &str) -> String {
    let mut b = String::with_capacity(name.len());
    for c in name.chars() {
        // Per RFC 6901, section 3, escape '~' and '/' characters.
        match c {
            '~' => b.push_str("~0"),
            '/' => b.push_str("~1"),
            c => b.push(c),
        }
    }
    b
}

/// The object name `quoted` (a JSON string with its quotes), unquoted as
/// Go's name stack holds it for a JSON pointer (`jsonwire.AppendUnquote`,
/// jsontext/state.go:664: invalid UTF-8 becomes U+FFFD).
// PORT: the result is Go's bytes, which are valid UTF-8, not the port form
// (a real U+FDD0 is one char). So is every JSON pointer, and its `%q` is
// `strconv::quote_bytes`.
pub(crate) fn unquote_pointer_name(quoted: &[u8]) -> String {
    let dec = JsonDecoder::new(
        quoted,
        JsonOptions {
            allow_invalid_utf8: true,
            ..JsonOptions::default()
        },
    );
    dec.consume_string_raw(0)
        .map(|(_, s)| s)
        .unwrap_or_default()
}

// The pointer token of the object name `quoted` (`unquote_pointer_name`).
fn pointer_token_of_name(quoted: &[u8]) -> String {
    escape_pointer_token(&unquote_pointer_name(quoted))
}

// Go: jsontext/errors.go:120 (*SyntacticError).Error
// PORT: the pointer is Go's bytes (`unquote_pointer_name`), so `%q` is
// `quote_bytes`.
fn syntactic_error_text(err: &SyntaxErr, mut pointer: String, mut offset: usize) -> String {
    use crate::gostd::strconv::quote_bytes;
    let mut b = String::from("jsontext: ");
    b.push_str(&err.text());
    if *err == SyntaxErr::DuplicateName {
        // Go `Pointer.LastToken` and `Pointer.Parent`.
        let slash = pointer.rfind('/').unwrap_or(0);
        let last = pointer[slash..]
            .strip_prefix('/')
            .unwrap_or(&pointer[slash..]);
        let last = last.replace("~1", "/").replace("~0", "~");
        b.push(' ');
        b.push_str(&quote_bytes(last.as_bytes()));
        pointer.truncate(slash);
        offset = 0; // not useful to print offset for duplicate names
    }
    if !pointer.is_empty() {
        b.push_str(" within ");
        b.push_str(&quote_bytes(
            crate::frontend::json_ext::truncate_pointer(&pointer, 100).as_bytes(),
        ));
    }
    if offset > 0 {
        b.push_str(" after offset ");
        b.push_str(&offset.to_string());
    }
    b
}

impl JsonDecoder<'_> {
    // Go: jsontext/state.go:180 appendStackPointer
    fn stack_pointer(&self, at: i32) -> String {
        let mut b = String::new();
        let n = self.stack.len();
        for (i, f) in self.stack.iter().enumerate() {
            // By default point to the previous array element.
            let mut index = f.len.wrapping_sub(1);
            if i + 1 == n {
                let need_value = f.is_object && f.len % 2 == 1;
                let need_name = f.is_object && f.len.is_multiple_of(2);
                if (at < 0 && f.len == 0) || (at == 0 && !need_value) || (at > 0 && need_name) {
                    return b;
                }
                if at > 0 && !f.is_object {
                    // Point to the next array element.
                    index = f.len;
                }
            }
            b.push('/');
            if f.is_object {
                b.push_str(&pointer_token_of_name(self.quoted_name_at(f.name)));
            } else {
                b.push_str(&index.to_string());
            }
        }
        b
    }

    // The JSON string (with its quotes) that starts at `p`.
    fn quoted_name_at(&self, p: usize) -> &[u8] {
        let end = JsonDecoder::new(
            &self.buf[p..],
            JsonOptions {
                allow_invalid_utf8: true,
                ..JsonOptions::default()
            },
        )
        .consume_string_raw(0)
        .map_or(self.buf.len() - p, |(end, _)| end);
        &self.buf[p..p + end]
    }

    /// Go `wrapSyntacticError(d, err, pos, where)` and the text of the
    /// `SyntacticError`: `suffix` holds the pointer tokens (outermost first)
    /// that a `pointerSuffixError` adds to the stack pointer.
    #[cold]
    #[inline(never)]
    fn syntactic_error(&self, err: SyntaxErr, pos: usize, at: i32, suffix: &[String]) -> JsonError {
        let mut pointer = self.stack_pointer(at);
        for token in suffix {
            pointer.push('/');
            pointer.push_str(token);
        }
        let mut err = err;
        if err == SyntaxErr::MismatchDelim {
            let mut place = "at start of value";
            if !self.stack.is_empty() && self.last_len() > 0 {
                place = if self.last_is_object() {
                    "after object value (expecting ',' or '}')"
                } else {
                    "after array element (expecting ',' or ']')"
                };
                // The problem is with the parent object or array.
                pointer.truncate(pointer.rfind('/').unwrap_or(0));
            }
            err = invalid_character(self.buf, pos, place);
        }
        JsonError::new(syntactic_error_text(&err, pointer, pos))
    }

    /// Go `errNonSingularValue` of an `UnmarshalJSONFrom` method that did not
    /// read exactly one value, wrapped by `newSemanticErrorWithPosition`: the
    /// pointer points to the next value when the method read nothing
    /// (`read_nothing`), else to the parent (Go `where` 0). `rust_type` is the
    /// `std::any::type_name` of the method's type; the text shows its Go type.
    ///
    /// PORT: not generic, so the binary holds one copy of this cold path.
    #[cold]
    fn non_singular_value_error(&self, rust_type: &str, read_nothing: bool) -> JsonError {
        let at = if read_nothing { 1 } else { 0 };
        crate::frontend::json_ext::SemanticError {
            wrapped: true,
            json_kind: 0,
            json_value: String::new(),
            go_type: crate::frontend::json_ext::go_type_string(rust_type),
            pointer: Some(self.stack_pointer(at)),
            byte_offset: self.skip_ws(self.pos),
            pos: crate::frontend::json_ext::ErrorPos::Before,
            err: "must read or write exactly one value".to_string(),
        }
        .into_json_error()
    }

    /// Go `newDuplicateNameError(dec.StackPointer(), nil, offset)` of a map
    /// or `any` unmarshaler that finds a duplicate name after it read it
    /// (the namespace is disabled): the text shows the name and the pointer
    /// of its object.
    #[cold]
    #[must_use]
    pub fn duplicate_name_error(&self) -> JsonError {
        JsonError::new(syntactic_error_text(
            &SyntaxErr::DuplicateName,
            self.stack_pointer(-1),
            0,
        ))
    }

    // Go `ReadToken`/`ReadValue`/`PeekKind` when the input ends before the
    // next token. At the top level Go returns `io.EOF`, which
    // `json.Unmarshal` (`unmarshalFull`) turns into a `SyntacticError` of
    // `io.ErrUnexpectedEOF` at the end of the input, with no pointer.
    #[cold]
    fn eof_before_token(&self, p: usize) -> JsonError {
        if self.stack.is_empty() {
            return JsonError::new(syntactic_error_text(
                &SyntaxErr::UnexpectedEof,
                String::new(),
                self.buf.len(),
            ));
        }
        self.syntactic_error(SyntaxErr::UnexpectedEof, p, 0, &[])
    }

    // Go `checkDelimBeforeIOError`: the input ends after `delim`. A string
    // can always come next, so `delim` is checked against one.
    #[cold]
    fn eof_after_delim(&self, delim: u8, p: usize) -> JsonError {
        if self.need_delim(b'"') != delim {
            return self.check_delim(delim, b'"');
        }
        self.syntactic_error(SyntaxErr::UnexpectedEof, p, 0, &[])
    }

    // Go: jsontext/decode.go:391 checkDelim, for a `delim` that `next` does
    // not allow. The error is at the delimiter (or `next` when there is
    // none).
    #[cold]
    fn check_delim(&self, delim: u8, next: u8) -> JsonError {
        let place = match self.need_delim(next) {
            b':' if delim != b':' => "after object name (expecting ':')",
            b',' if delim != b',' => {
                if self.last_is_object() {
                    "after object value (expecting ',' or '}')"
                } else {
                    "after array element (expecting ',' or ']')"
                }
            }
            _ => "at start of value",
        };
        let pos = self.skip_ws(self.pos);
        self.syntactic_error(invalid_character(self.buf, pos, place), pos, 0, &[])
    }

    // The Go error of a `ReadValue` of the value at `start`, which failed
    // in the port with `err`. The state goes back to `before` (the offset,
    // the length and the last name of the innermost container at `depth`).
    // An object or array is checked again as Go's `consumeValue` does, which
    // reads it whole before the state check; a `}` or `]` is Go's
    // `consumeValue` error; any other value fails as it does in
    // `ReadToken`, so `err` is Go's.
    #[cold]
    #[inline(never)]
    fn read_value_error(
        &mut self,
        start: usize,
        depth: usize,
        before: (usize, usize, usize),
        err: JsonError,
    ) -> JsonError {
        while self.stack.len() > depth {
            if let Some(names) = self.stack.pop().and_then(|f| f.names) {
                self.keep_names(names);
            }
        }
        self.pos = before.0;
        match self.stack.last_mut() {
            Some(f) => {
                f.len = before.1;
                f.name = before.2;
            }
            None => self.top_len = before.1,
        }
        match self.buf[start] {
            b'{' | b'[' | b'}' | b']' => match self.go_consume_value(start, depth + 1) {
                Err((pos, e, suffix)) => self.syntactic_error(e, pos, 1, &suffix),
                // Go `pushObject` / `pushArray`.
                Ok(_) if self.need_object_name() => {
                    self.syntactic_error(SyntaxErr::NonStringName, start, 1, &[])
                }
                Ok(_) if self.stack.len() == MAX_NESTING_DEPTH => {
                    self.syntactic_error(SyntaxErr::MaxDepth, start, 1, &[])
                }
                Ok(_) => err,
            },
            _ => err,
        }
    }

    // Go: jsontext/decode.go:857 consumeValue with consumeObject and
    // consumeArray, for the value at `start`, whose `depth` is Go's (the
    // stack depth plus one). Gives the end of the value, or Go's error, its
    // offset and pointer suffix.
    // PORT: Go recurses; this keeps its own stack of open containers.
    fn go_consume_value(&self, start: usize, depth: usize) -> Result<usize, ConsumeError> {
        struct Open {
            is_object: bool,
            // The pointer token of the member being read (name or index).
            member: String,
            index: usize,
            names: Option<FxHashSet<String>>,
        }
        // An error at `pos`: each open container but the innermost adds its
        // member; the innermost adds it when `wrap`.
        fn fail(open: &[Open], pos: usize, err: SyntaxErr, wrap: bool) -> ConsumeError {
            let n = open.len() - usize::from(!wrap && !open.is_empty());
            let suffix = open[..n].iter().map(|o| o.member.clone()).collect();
            (pos, err, suffix)
        }
        enum Step {
            Value,
            AfterValue,
            BeforeName,
        }
        let b = self.buf;
        let mut open: Vec<Open> = Vec::new();
        let mut p = start;
        let mut step = Step::Value;
        loop {
            match step {
                Step::Value => {
                    let c = b[p];
                    match normalize_kind(c) {
                        b'n' | b't' | b'f' => {
                            let lit: &[u8] = match c {
                                b'n' => b"null",
                                b't' => b"true",
                                _ => b"false",
                            };
                            if !b[p..].starts_with(lit) {
                                let (pos, err) = consume_literal_error(b, p, lit);
                                return Err(fail(&open, pos, err, true));
                            }
                            p += lit.len();
                        }
                        b'"' => match self.consume_string_raw(p) {
                            Ok((end, _)) => p = end,
                            Err((pos, err)) => return Err(fail(&open, pos, err, true)),
                        },
                        b'0' => match consume_number_raw(b, p) {
                            Ok(end) => p = end,
                            Err((pos, err)) => return Err(fail(&open, pos, err, true)),
                        },
                        b'{' | b'[' => {
                            if depth + open.len() == MAX_NESTING_DEPTH + 1 {
                                return Err(fail(&open, p, SyntaxErr::MaxDepth, true));
                            }
                            let is_object = c == b'{';
                            open.push(Open {
                                is_object,
                                member: "0".to_string(),
                                index: 0,
                                names: (is_object && !self.options.allow_duplicate_names)
                                    .then(FxHashSet::default),
                            });
                            p = self.skip_ws(p + 1);
                            if p >= b.len() {
                                return Err(fail(&open, p, SyntaxErr::UnexpectedEof, false));
                            }
                            if b[p] == if is_object { b'}' } else { b']' } {
                                open.pop();
                                p += 1;
                            } else {
                                step = if is_object {
                                    Step::BeforeName
                                } else {
                                    Step::Value
                                };
                                continue;
                            }
                        }
                        next => {
                            let last_is_object = self.last_is_object();
                            let err = if (last_is_object && next == b']')
                                || (!last_is_object && next == b'}')
                            {
                                SyntaxErr::MismatchDelim
                            } else {
                                invalid_character(b, p, "at start of value")
                            };
                            return Err(fail(&open, p, err, true));
                        }
                    }
                    step = Step::AfterValue;
                }
                Step::BeforeName => {
                    p = self.skip_ws(p);
                    if p >= b.len() {
                        return Err(fail(&open, p, SyntaxErr::UnexpectedEof, false));
                    }
                    let name_start = p;
                    let (end, name) = self
                        .consume_string_raw(p)
                        .map_err(|(pos, err)| fail(&open, pos, err, false))?;
                    let top = open.last_mut().expect("open object");
                    top.member = escape_pointer_token(&name);
                    if let Some(names) = &mut top.names
                        && !names.insert(name)
                    {
                        return Err(fail(&open, name_start, SyntaxErr::DuplicateName, true));
                    }
                    p = self.skip_ws(end);
                    if p >= b.len() {
                        return Err(fail(&open, p, SyntaxErr::UnexpectedEof, true));
                    }
                    if b[p] != b':' {
                        let err = invalid_character(b, p, "after object name (expecting ':')");
                        return Err(fail(&open, p, err, true));
                    }
                    p = self.skip_ws(p + 1);
                    if p >= b.len() {
                        return Err(fail(&open, p, SyntaxErr::UnexpectedEof, true));
                    }
                    step = Step::Value;
                }
                Step::AfterValue => {
                    let Some(top) = open.last_mut() else {
                        return Ok(p);
                    };
                    p = self.skip_ws(p);
                    if p >= b.len() {
                        return Err(fail(&open, p, SyntaxErr::UnexpectedEof, false));
                    }
                    let is_object = top.is_object;
                    match b[p] {
                        b',' => {
                            p += 1;
                            if is_object {
                                step = Step::BeforeName;
                            } else {
                                top.index += 1;
                                top.member = top.index.to_string();
                                p = self.skip_ws(p);
                                if p >= b.len() {
                                    return Err(fail(&open, p, SyntaxErr::UnexpectedEof, false));
                                }
                                step = Step::Value;
                            }
                        }
                        c if c == if is_object { b'}' } else { b']' } => {
                            open.pop();
                            p += 1;
                        }
                        _ => {
                            let place = if is_object {
                                "after object value (expecting ',' or '}')"
                            } else {
                                "after array element (expecting ',' or ']')"
                            };
                            let err = invalid_character(b, p, place);
                            return Err(fail(&open, p, err, false));
                        }
                    }
                }
            }
        }
    }
}

/// Go `json.UnmarshalerFrom` and the JSON v2 default unmarshalers.
pub trait UnmarshalerFrom {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError>;
}

/// Go `json.MarshalerTo` and the JSON v2 default marshalers.
/// PORT: the encoder is the output string. Output is compact.
pub trait MarshalerTo {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError>;
}

// Go v2 string arshaler: null gives "", a string sets the value, any other
// kind is an error after the value is read.
impl UnmarshalerFrom for String {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        match dec.peek_kind() {
            b'n' => {
                dec.read_token()?;
                self.clear();
                Ok(())
            }
            b'"' => {
                let JsonToken::String(s) = dec.read_token()? else {
                    unreachable!("peeked a string")
                };
                *self = s;
                Ok(())
            }
            _ => {
                // Go reads the value with `ReadValue`, so its syntax error
                // comes first.
                let val = dec.read_value()?;
                Err(crate::frontend::json_ext::unmarshal_kind_error(
                    normalize_kind(val[0]),
                    "string",
                ))
            }
        }
    }
}

// Go v2 bool arshaler.
impl UnmarshalerFrom for bool {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        match dec.peek_kind() {
            b'n' | b't' | b'f' => {
                *self = dec.read_token()? == JsonToken::True;
                Ok(())
            }
            // Go reads one token; it skips the rest of the value only with
            // legacy semantics.
            k => {
                dec.read_token()?;
                Err(crate::frontend::json_ext::unmarshal_kind_error(k, "bool"))
            }
        }
    }
}

// Go v2 float64 arshaler. An out-of-range number sets ±Inf and is an error,
// like Go `strconv.ParseFloat`.
impl UnmarshalerFrom for f64 {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        match dec.peek_kind() {
            b'n' => {
                dec.read_token()?;
                *self = 0.0;
                Ok(())
            }
            b'0' => {
                let JsonToken::Number(raw) = dec.read_token()? else {
                    unreachable!("peeked a number")
                };
                // The strict JSON grammar is a subset of the Rust float syntax,
                // and both round to nearest.
                let v: f64 = raw.parse().map_err(|_| JsonError::new("invalid number"))?;
                *self = v;
                if v.is_infinite() {
                    return Err(crate::frontend::json_ext::unmarshal_value_error(
                        raw.as_bytes(),
                        "float64",
                        "value out of range",
                    ));
                }
                Ok(())
            }
            _ => {
                // Go reads the value with `ReadValue`, so its syntax error
                // comes first.
                let val = dec.read_value()?;
                Err(crate::frontend::json_ext::unmarshal_kind_error(
                    normalize_kind(val[0]),
                    "float64",
                ))
            }
        }
    }
}

// Go v2 map arshaler for `map[string]V`. The map merges into the existing
// map. A name that is already in the map is a duplicate only if it came from
// this JSON object (the `seen` set), unless `AllowDuplicateNames` is set.
impl<V: UnmarshalerFrom + Default + Clone> UnmarshalerFrom for FxHashMap<String, V> {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let tok = dec.read_token()?;
        match tok {
            JsonToken::Null => {
                // PORT: Go sets a nil map. An empty map is the Rust zero value.
                self.clear();
                Ok(())
            }
            JsonToken::BeginObject => {
                // String keys have a unique representation unless invalid
                // UTF-8 is allowed, so the map does its own duplicate check.
                if !dec.options.allow_invalid_utf8 {
                    dec.disable_namespace();
                }
                let allow_dup = dec.options.allow_duplicate_names;
                let mut seen: Option<FxHashSet<String>> = if !allow_dup && !self.is_empty() {
                    Some(FxHashSet::default())
                } else {
                    None
                };
                while dec.peek_kind() != b'}' {
                    let mut k = String::new();
                    json_unmarshal_decode(dec, &mut k)?;
                    let mut v = V::default();
                    if let Some(existing) = self.get(&k) {
                        if !allow_dup && seen.as_ref().is_none_or(|s| s.contains(&k)) {
                            return Err(dec.duplicate_name_error());
                        }
                        v = existing.clone();
                    }
                    let err = json_unmarshal_decode(dec, &mut v);
                    if let Some(s) = &mut seen {
                        s.insert(k.clone());
                    }
                    self.insert(k, v);
                    err?;
                }
                dec.read_token()?;
                Ok(())
            }
            // Go `newUnmarshalErrorAfterWithSkipping` skips the rest of the
            // value only with legacy semantics.
            _ => Err(crate::frontend::json_ext::unmarshal_kind_error(
                tok.kind(),
                &crate::frontend::json_ext::go_type_name::<Self>(),
            )),
        }
    }
}

// Go v2 string marshaler: escapes `"`, `\` and control characters only.
impl MarshalerTo for str {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        append_json_quote(enc, self);
        Ok(())
    }
}

/// Go jsonwire `AppendQuote` with the flags of `internal/json` Marshal: it
/// escapes `"`, `\` and control characters only.
// PORT: `s` is the port form of a Go string (see
// `scanner_util::GO_STRING_MARKER`). Go writes U+FFFD for each byte that is
// not valid UTF-8: one for an invalid byte unit and three for a lone
// surrogate unit. Other units keep their port form.
pub fn append_json_quote(enc: &mut String, s: &str) {
    append_json_quote_with(enc, s, true);
}

/// `append_json_quote` that keeps every unit of the port form. The port's
/// own worker protocol uses it to pass Go strings between processes, and
/// `json_new_port_form_decoder` reads them back.
pub fn append_json_quote_port_form(enc: &mut String, s: &str) {
    append_json_quote_with(enc, s, false);
}

/// A JSON array of `append_json_quote_port_form` strings.
pub fn append_json_quote_port_form_list(enc: &mut String, list: &[impl AsRef<str>]) {
    enc.push('[');
    for (i, s) in list.iter().enumerate() {
        if i > 0 {
            enc.push(',');
        }
        append_json_quote_port_form(enc, s.as_ref());
    }
    enc.push(']');
}

fn append_json_quote_with(enc: &mut String, s: &str, replace_invalid: bool) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    enc.push('"');
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => enc.push_str("\\\""),
            '\\' => enc.push_str("\\\\"),
            '\u{8}' => enc.push_str("\\b"),
            '\u{C}' => enc.push_str("\\f"),
            '\n' => enc.push_str("\\n"),
            '\r' => enc.push_str("\\r"),
            '\t' => enc.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let b = c as usize;
                enc.push_str("\\u00");
                enc.push(char::from(HEX[b >> 4]));
                enc.push(char::from(HEX[b & 0xF]));
            }
            GO_STRING_MARKER => {
                let (unit, size) = go_unit_at(s, i);
                if size > c.len_utf8() {
                    // Skip the second char of the unit.
                    chars.next();
                }
                match unit {
                    GoUnit::InvalidByte(_) | GoUnit::Surrogate(_) if replace_invalid => {
                        for _ in 0..unit.go_len() {
                            enc.push(char::REPLACEMENT_CHARACTER);
                        }
                    }
                    _ => enc.push_str(&s[i..i + size]),
                }
            }
            c => enc.push(c),
        }
    }
    enc.push('"');
}

impl MarshalerTo for String {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.as_str().marshal_json_to(enc)
    }
}

impl MarshalerTo for bool {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str(if *self { "true" } else { "false" });
        Ok(())
    }
}

// Go v2 slice marshaler. PORT: Go marshals a nil slice as `[]` in v2 too
// (`FormatNilSliceAsNull` is off by default), so there is no null case.
impl<T: MarshalerTo> MarshalerTo for [T] {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        for (i, v) in self.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            v.marshal_json_to(enc)?;
        }
        enc.push(']');
        Ok(())
    }
}

impl<T: MarshalerTo> MarshalerTo for Vec<T> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.as_slice().marshal_json_to(enc)
    }
}

// Go `collections.OrderedMap.MarshalJSONTo`: members in insertion order.
impl<V: MarshalerTo> MarshalerTo for IndexMap<String, V> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('{');
        for (i, (k, v)) in self.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            k.marshal_json_to(enc)?;
            enc.push(':');
            v.marshal_json_to(enc)?;
        }
        enc.push('}');
        Ok(())
    }
}

impl<T: MarshalerTo + ?Sized> MarshalerTo for &T {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        (**self).marshal_json_to(enc)
    }
}

// Go: json/json.go:11 allowInvalid
// PORT: the Go slice of options is a constant list.
const ALLOW_INVALID: &[JsonOption] = &[JsonOption::AllowInvalidUtf8(true)];

// Go: json/json.go:13 Marshal
// PORT: named `json_marshal` so the glob export stays unambiguous. Go returns
// bytes; the output here is always UTF-8 text. With `AllowInvalidUTF8`, Go
// writes U+FFFD for each invalid byte of a string, which the string
// marshaler does for the port form (`append_json_quote`).
pub fn json_marshal<T: MarshalerTo + ?Sized>(
    input: &T,
    opts: &[JsonOption],
) -> Result<String, JsonError> {
    let mut all: Vec<JsonOption> = ALLOW_INVALID.to_vec();
    all.extend_from_slice(opts);
    let _ = JsonOptions::from_options(&all);
    let mut out = String::new();
    input.marshal_json_to(&mut out)?;
    json_check_nesting_depth(&out)?;
    Ok(out)
}

// Go: jsontext/state.go:302 pushObject and :337 pushArray, the
// `len(m.Stack) == maxNestingDepth` case (errMaxDepth, tsgo pin B
// go-json-experiment/json v0.0.0-20260623181947-01eb4420fa68).
// PORT: the Go encoder checks each `{` and `[` as it writes it, so a value
// nested more than `maxNestingDepth` (10000) levels deep fails to marshal.
// The Rust marshalers write the compact text with no state, so this checks
// the finished text instead: the result is the same error and no output.
// A value that deep has more than 10000 `{` and `[` bytes, so a text with
// fewer is not scanned. The error text is Go's `errMaxDepth` text without
// the v2 wrapping (see the file header).
fn json_check_nesting_depth(compact: &str) -> Result<(), JsonError> {
    if compact.len() <= 2 * MAX_NESTING_DEPTH
        || memchr::memchr2_iter(b'{', b'[', compact.as_bytes()).count() <= MAX_NESTING_DEPTH
    {
        return Ok(());
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &c in compact.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => {
                if depth == MAX_NESTING_DEPTH {
                    return Err(JsonError::new("exceeded max depth"));
                }
                depth += 1;
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

// Go: json/json.go:22 MarshalEncode
// PORT: not ported. Only the LSP, API and build-info writers use it.

// Go: json/json.go:31 MarshalWrite
// PORT: Go writes through a streaming `jsontext.Encoder` that flushes as it
// goes, so a marshal error can leave partial output. This marshals the whole
// value first (`json_marshal`) and writes nothing on a marshal error. Like Go
// (`jsonflags.OmitTopLevelNewline` in v2 `MarshalWrite`), the output has no
// trailing newline. A write error is returned as a `JsonError`.
pub fn json_marshal_write<T: MarshalerTo + ?Sized>(
    out: &mut dyn std::io::Write,
    input: &T,
    opts: &[JsonOption],
) -> Result<(), JsonError> {
    let b = json_marshal(input, opts)?;
    out.write_all(b.as_bytes())
        .map_err(|err| JsonError::new(err.to_string()))
}

// Go: json/json.go:40 MarshalIndent
// PORT: the Rust marshalers write compact output. Go passes
// `jsontext.WithIndentPrefix` and `jsontext.WithIndent`, which make the
// encoder add whitespace before each token as it writes it
// (`encoderState.WriteToken` and `WriteValue` call `MayAppendDelim` and
// `appendWhitespace`). That whitespace depends only on the token sequence,
// so this marshals compact output and replays its tokens through the same
// rules (`json_append_multiline`).
pub fn json_marshal_indent<T: MarshalerTo + ?Sized>(
    input: &T,
    prefix: &str,
    indent: &str,
) -> Result<String, JsonError> {
    if prefix.is_empty() && indent.is_empty() {
        // WithIndentPrefix and WithIndent imply multiline output, so skip them.
        return json_marshal(input, &[]);
    }
    json_check_indent(prefix, " in indent prefix");
    json_check_indent(indent, " in indent");
    let compact = json_marshal(input, &[])?;
    json_append_multiline(&compact, prefix, indent)
}

// Go: jsontext/options.go:232 WithIndent and :265 WithIndentPrefix
// (the character checks). Go panics when the value holds anything but
// spaces and tabs.
// PORT: Go quotes the first bad rune with `jsonwire.QuoteRune`
// (`strconv.QuoteRune`); Rust `{:?}` gives the same `'x'` form for
// printable characters.
fn json_check_indent(value: &str, what: &str) {
    if let Some(c) = value.trim_matches([' ', '\t']).chars().next() {
        panic!("json: invalid character {c:?}{what}");
    }
}

/// Go `jsontext.stateEntry`, reduced to what the encoder whitespace reads:
/// the nesting type and the number of tokens written at this level (object
/// names and values both count).
#[derive(Clone, Copy, Debug)]
struct JsonMultilineEntry {
    is_object: bool,
    length: i64,
}

impl JsonMultilineEntry {
    // Go: jsontext/state.go:480 needImplicitColon
    fn need_implicit_colon(self) -> bool {
        self.need_object_value()
    }

    // Go: jsontext/state.go:486 needObjectValue
    fn need_object_value(self) -> bool {
        self.is_object && self.length % 2 == 1
    }

    // Go: jsontext/state.go:493 needImplicitComma
    fn need_implicit_comma(self, next: u8) -> bool {
        !self.need_object_value() && self.length > 0 && next != b'}' && next != b']'
    }
}

/// Go `jsontext.stateMachine`. `last` starts as the top-level virtual array
/// (Go `stateMachine.reset`).
#[derive(Debug)]
struct JsonMultilineState {
    stack: Vec<JsonMultilineEntry>,
    last: JsonMultilineEntry,
}

impl JsonMultilineState {
    // Go: jsontext/state.go:249 Depth
    fn depth(&self) -> usize {
        self.stack.len() + 1
    }

    // Go: jsontext/state.go:372 NeedIndent
    /// NeedIndent reports whether indent whitespace should be injected.
    /// A zero value means that no whitespace should be injected.
    /// A positive value means '\n', indentPrefix, and (n-1) copies of indentBody
    /// should be appended to the output immediately before the next token.
    fn need_indent(&self, next: u8) -> usize {
        let will_end = next == b'}' || next == b']';
        if self.depth() == 1 {
            0 // top-level values are never indented
        } else if self.last.length == 0 && will_end {
            0 // an empty object or array is never indented
        } else if self.last.length == 0 || self.last.need_implicit_comma(next) {
            self.depth()
        } else if will_end {
            self.depth() - 1
        } else {
            0
        }
    }

    // Go: jsontext/state.go:389 MayAppendDelim
    /// MayAppendDelim appends a colon or comma that may precede the next token.
    fn may_append_delim(&self, b: &mut String, next: u8) {
        if self.last.need_implicit_colon() {
            b.push(':');
        } else if self.last.need_implicit_comma(next) && !self.stack.is_empty() {
            // comma not needed for top-level values
            b.push(',');
        }
    }

    // Go: jsontext/state.go:403 needDelim
    /// needDelim reports whether a colon or comma token should be implicitly emitted
    /// before the next token of the specified kind.
    /// A zero value means no delimiter should be emitted.
    fn need_delim(&self, next: u8) -> u8 {
        if self.last.need_implicit_colon() {
            b':'
        } else if self.last.need_implicit_comma(next) && !self.stack.is_empty() {
            // comma not needed for top-level values
            b','
        } else {
            0
        }
    }

    // Go: jsontext/state.go:270 appendLiteral, :284 appendString and :296
    // appendNumber
    fn increment(&mut self) {
        self.last.length += 1;
    }

    // Go: jsontext/state.go:302 pushObject and :337 pushArray
    fn push(&mut self, is_object: bool) {
        self.last.length += 1;
        self.stack.push(self.last);
        self.last = JsonMultilineEntry {
            is_object,
            length: 0,
        };
    }

    // Go: jsontext/state.go:320 popObject and :355 popArray
    fn pop(&mut self) -> Result<(), JsonError> {
        self.last = self
            .stack
            .pop()
            .ok_or_else(|| JsonError::new("mismatching structural token"))?;
        Ok(())
    }
}

// Go: jsontext/encode.go:634 appendWhitespace
/// appendWhitespace appends whitespace that immediately precedes the next token.
// PORT: the flags are the ones `jsonopts.Struct.InitializeMultiline` sets for
// `WithIndentPrefix`/`WithIndent`: `SpaceAfterColon` on, `SpaceAfterComma`
// off, `Multiline` on.
fn json_append_whitespace(
    m: &JsonMultilineState,
    b: &mut String,
    next: u8,
    prefix: &str,
    indent: &str,
) {
    if m.need_delim(next) == b':' {
        // SpaceAfterColon
        b.push(' ');
    } else {
        // SpaceAfterComma is off; Multiline is on.
        json_append_indent(b, m.need_indent(next), prefix, indent);
    }
}

// Go: jsontext/encode.go:652 AppendIndent
/// AppendIndent appends the appropriate number of indentation characters
/// for the current nested level, n.
fn json_append_indent(b: &mut String, mut n: usize, prefix: &str, indent: &str) {
    if n == 0 {
        return;
    }
    b.push('\n');
    b.push_str(prefix);
    while n > 1 {
        b.push_str(indent);
        n -= 1;
    }
}

// PORT: the multiline encoder pass. It reads each token of the compact
// output, drops the compact `:` and `,` (and any whitespace, like Go
// `reformatValue`), and writes the token the way Go `WriteToken` does:
// `MayAppendDelim`, then `appendWhitespace`, then the token bytes, then the
// state machine update. Top-level output has no trailing newline, as in Go
// `json.Marshal`.
fn json_append_multiline(compact: &str, prefix: &str, indent: &str) -> Result<String, JsonError> {
    json_append_multiline_pieces(compact, prefix, indent, None)
}

/// The size classes of the Go allocator (internal/runtime/gc/sizeclasses.go
/// `SizeClassToSize`, go1.27.1).
const GO_SIZE_CLASSES: [usize; 67] = [
    8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320, 352,
    384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792, 2048,
    2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472, 9728,
    10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264, 28672,
    32768,
];

// Go: runtime/slice.go growslice (`nextslicecap`) and runtime/msize.go
// roundupsize, for a `[]byte` (noscan)
/// The capacity of a Go `[]byte` with capacity `old_cap` after an `append`
/// that needs `new_len` bytes.
fn go_byte_slice_grow(old_cap: usize, new_len: usize) -> usize {
    let mut new_cap = old_cap;
    if new_len > 2 * old_cap {
        new_cap = new_len;
    } else if old_cap < 256 {
        new_cap = 2 * old_cap;
    } else {
        while new_cap < new_len {
            new_cap += (new_cap + 3 * 256) >> 2;
        }
    }
    match GO_SIZE_CLASSES.iter().find(|&&size| size >= new_cap) {
        Some(&size) => size,
        // A large object takes whole pages of 8 KiB.
        None => new_cap.next_multiple_of(8192),
    }
}

/// Where Go's streaming `jsontext.Encoder` (v2 `MarshalWrite` to an
/// `io.Writer` that is not a `bytes.Buffer`) writes its buffer out, for
/// `json_marshal_indent_write`. Its buffer starts empty and grows by Go
/// `append`. After each token it flushes when the value is complete or the
/// buffer is more than 3/4 full (`NeedFlush`), except where it may have to
/// take back an empty member (`avoidFlush`). After a write it doubles a
/// buffer of up to 2 KiB that is less than half the output so far.
// Go: jsontext/encode.go NeedFlush, Flush and avoidFlush, the appends of
// WriteToken, AppendIndent and jsonwire.AppendQuote (go1.27.1).
#[derive(Default)]
struct JsonStreamPieces {
    /// `cap(e.Buf)`.
    cap: usize,
    /// The output offset where `e.Buf` starts.
    start: usize,
    /// `len(e.Buf)` while a token is appended.
    len: usize,
    /// The output offset where each write ends.
    ends: Vec<usize>,
}

impl JsonStreamPieces {
    /// Go `append` of `n` bytes to `e.Buf`.
    fn append(&mut self, n: usize) {
        self.reserve(n);
        self.len += n;
    }

    /// Go `slices.Grow(e.Buf, n)`.
    fn reserve(&mut self, n: usize) {
        if n > 0 && self.len + n > self.cap {
            self.cap = go_byte_slice_grow(self.cap, self.len + n);
        }
    }

    /// The appends of Go `WriteToken` before a token of kind `k`: the
    /// delimiter and the whitespace. `out_len` is the output length so far
    /// and `m` the state before the token.
    fn before_token(
        &mut self,
        out_len: usize,
        m: &JsonMultilineState,
        k: u8,
        prefix: &str,
        indent: &str,
    ) {
        self.len = out_len - self.start;
        let delim = m.need_delim(k);
        if delim != 0 {
            self.append(1);
        }
        if delim == b':' {
            self.append(1);
        } else {
            let n = m.need_indent(k);
            if n > 0 {
                self.append(1);
                self.append(prefix.len());
                for _ in 1..n {
                    self.append(indent.len());
                }
            }
        }
    }

    /// The appends of Go `WriteToken` for the token `token` of kind `k`.
    fn token(&mut self, k: u8, token: &[u8]) {
        if k != b'"' {
            self.append(token.len());
            return;
        }
        // Go `jsonwire.AppendQuote`: room for the quotes and the unescaped
        // bytes, then the quote, each run and each escape, and the quote.
        // The port output escapes what Go escapes; each escape stands for
        // one byte (`\uXXXX` only below U+0020).
        let body = &token[1..token.len() - 1];
        let mut escapes = Vec::new();
        let mut i = 0;
        while i < body.len() {
            if body[i] == b'\\' {
                let len = if body.get(i + 1) == Some(&b'u') { 6 } else { 2 };
                escapes.push((i, len));
                i += len;
            } else {
                i += 1;
            }
        }
        let escaped: usize = escapes.iter().map(|&(_, len)| len - 1).sum();
        self.reserve(token.len() - escaped);
        self.append(1);
        let mut run_start = 0;
        for (at, len) in escapes {
            self.append(at - run_start);
            self.append(len);
            run_start = at + len;
        }
        self.append(body.len() - run_start);
        self.append(1);
    }

    /// Go `if e.NeedFlush() { e.Flush() }` after a token, with `b` the output
    /// so far.
    fn after_token(&mut self, b: &str, m: &JsonMultilineState) {
        let len = b.len() - self.start;
        if m.depth() != 1 && len <= 3 * self.cap / 4 {
            return;
        }
        let last = m.last;
        let avoid = last.length == 0
            || last.need_object_value()
            || (last.is_object
                && last.length % 2 == 0
                && len >= 2
                && matches!(
                    &b.as_bytes()[b.len() - 2..],
                    b"ll" | b"\"\"" | b"{}" | b"[]"
                ));
        if avoid {
            return;
        }
        self.ends.push(b.len());
        self.start = b.len();
        if self.cap <= 2048 && self.cap < b.len() / 2 {
            self.cap *= 2;
        }
    }
}

/// `json_append_multiline`, which also records Go's writes in `pieces`.
fn json_append_multiline_pieces(
    compact: &str,
    prefix: &str,
    indent: &str,
    mut pieces: Option<&mut JsonStreamPieces>,
) -> Result<String, JsonError> {
    let src = compact.as_bytes();
    let mut b = String::with_capacity(compact.len() * 2);
    let mut m = JsonMultilineState {
        stack: Vec::new(),
        last: JsonMultilineEntry {
            is_object: false,
            length: 0,
        },
    };
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        if c == b',' || c == b':' || is_ws(c) {
            i += 1;
            continue;
        }
        let k = normalize_kind(c);
        if let Some(pieces) = pieces.as_deref_mut() {
            pieces.before_token(b.len(), &m, k, prefix, indent);
        }
        m.may_append_delim(&mut b, k);
        json_append_whitespace(&m, &mut b, k, prefix, indent);
        let start = i;
        match k {
            b'"' => {
                i += 1;
                while i < src.len() && src[i] != b'"' {
                    if src[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i >= src.len() {
                    return Err(JsonError::new("unexpected EOF within string"));
                }
                i += 1;
                m.increment();
            }
            b'0' => {
                while i < src.len()
                    && matches!(src[i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    i += 1;
                }
                m.increment();
            }
            b'n' | b't' | b'f' => {
                while i < src.len() && src[i].is_ascii_lowercase() {
                    i += 1;
                }
                m.increment();
            }
            b'{' | b'[' => {
                i += 1;
                m.push(k == b'{');
            }
            b'}' | b']' => {
                i += 1;
                m.pop()?;
            }
            _ => {
                return Err(JsonError::new(format!(
                    "invalid character {:?} at start of value",
                    char::from(c)
                )));
            }
        }
        if let Some(pieces) = pieces.as_deref_mut() {
            pieces.token(k, &src[start..i]);
        }
        b.push_str(&compact[start..i]);
        if let Some(pieces) = pieces.as_deref_mut() {
            pieces.after_token(&b, &m);
        }
    }
    Ok(b)
}

// Go: json/json.go:48 MarshalIndentWrite
// Used by execute/tsc.go:405 showConfig (prefix "", indent four spaces).
// PORT: the indented output comes from `json_marshal_indent` (see the
// PORT notes there and on `json_marshal_write`). There is no trailing
// newline. It goes out in the writes of Go's streaming encoder
// (`JsonStreamPieces`), after the whole value is marshaled.
pub fn json_marshal_indent_write<T: MarshalerTo + ?Sized>(
    out: &mut dyn std::io::Write,
    input: &T,
    prefix: &str,
    indent: &str,
) -> Result<(), JsonError> {
    if prefix.is_empty() && indent.is_empty() {
        // WithIndentPrefix and WithIndent imply multiline output, so skip them.
        return json_marshal_write(out, input, &[]);
    }
    json_check_indent(prefix, " in indent prefix");
    json_check_indent(indent, " in indent");
    let compact = json_marshal(input, &[])?;
    let mut pieces = JsonStreamPieces::default();
    let b = json_append_multiline_pieces(&compact, prefix, indent, Some(&mut pieces))?;
    let mut start = 0;
    for end in pieces.ends.into_iter().chain([b.len()]) {
        if end > start {
            out.write_all(&b.as_bytes()[start..end])
                .map_err(|err| JsonError::new(err.to_string()))?;
            start = end;
        }
    }
    Ok(())
}

// Go: json/json.go:56 Unmarshal
// PORT: `out` implements `UnmarshalerFrom` in place of Go reflection. Like
// Go, `out` is not reset on error, and the input must hold exactly one value.
pub fn json_unmarshal<T: UnmarshalerFrom + ?Sized>(
    input: &[u8],
    out: &mut T,
    opts: &[JsonOption],
) -> Result<(), JsonError> {
    let mut dec = JsonDecoder::new(input, JsonOptions::from_options(opts));
    json_unmarshal_decode(&mut dec, out)?;
    dec.check_eof()
}

// Go: json/json.go:60 UnmarshalDecode
// PORT: Go merges `opts` into the decoder options; callers here pass none,
// so the decoder options apply. The v2 method arshaler is kept: a plain
// error of the `UnmarshalerFrom` of `T` gets the Go type of `T`
// (`json_ext::wrap_method_error`), as the v2 arshaler of a type with an
// `UnmarshalJSONFrom` method does, and an error that already has its type
// keeps it. The method must read exactly one value.
pub fn json_unmarshal_decode<T: UnmarshalerFrom + ?Sized>(
    dec: &mut JsonDecoder<'_>,
    out: &mut T,
) -> Result<(), JsonError> {
    let (prev_depth, prev_len) = dec.depth_length();
    out.unmarshal_json_from(dec).map_err(|err| {
        crate::frontend::json_ext::wrap_method_error(err, std::any::type_name::<T>())
    })?;
    let (curr_depth, curr_len) = dec.depth_length();
    if prev_depth != curr_depth || prev_len + 1 != curr_len {
        return Err(dec.non_singular_value_error(
            std::any::type_name::<T>(),
            prev_depth == curr_depth && prev_len == curr_len,
        ));
    }
    Ok(())
}

// Go: json/json.go:64 UnmarshalRead
// PORT: not ported. Only the LSP and API readers use it.

// Go: json/json.go:68 AllowDuplicateNames
#[must_use]
pub fn json_allow_duplicate_names(allow: bool) -> JsonOption {
    JsonOption::AllowDuplicateNames(allow)
}

// Go: json/json.go:72 Deterministic
// Go: json/json.go:76 WithIndent
// PORT: not ported. Map output order is only needed by build-info and
// baseline writers. Indentation is available through `json_marshal_indent`.

// Go: json/json.go:80 NewDecoder
// PORT: Go reads from an `io.Reader`; the Rust decoder reads a complete
// buffer with default options.
#[must_use]
pub fn json_new_decoder(r: &[u8]) -> JsonDecoder<'_> {
    JsonDecoder::new(r, JsonOptions::default())
}

/// A decoder for the port's own protocol text, whose strings are already
/// in the port form (see `JsonOptions::port_form`).
#[must_use]
pub fn json_new_port_form_decoder(r: &[u8]) -> JsonDecoder<'_> {
    JsonDecoder::new(
        r,
        JsonOptions {
            port_form: true,
            ..JsonOptions::default()
        },
    )
}

// Go: json/json.go:84 type aliases (Value, Kind, UnmarshalerFrom,
// MarshalerTo, Decoder, Encoder) and json/json.go:94 token values
// (BeginObject, EndObject, Null, BeginArray, EndArray).
// PORT: `JsonToken` variants and `JsonToken::kind` stand in for the token
// values. `Value` is `&[u8]`, `Kind` is `u8`, `Decoder` is `JsonDecoder`.

#[cfg(test)]
mod marshal_depth_tests {
    use super::*;

    /// A value that writes `self.0` nested arrays, with a string that holds
    /// brackets in the middle.
    struct Nested(usize);

    impl MarshalerTo for Nested {
        fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
            for _ in 0..self.0 {
                enc.push('[');
            }
            enc.push_str(r#""[{\"[""#);
            for _ in 0..self.0 {
                enc.push(']');
            }
            Ok(())
        }
    }

    // Go JSON v2 `json.Marshal` accepts 10000 nested arrays and fails with
    // `exceeded max depth` for 10001 (jsontext `maxNestingDepth`).
    #[test]
    fn marshal_fails_past_max_nesting_depth() {
        assert!(json_marshal(&Nested(MAX_NESTING_DEPTH), &[]).is_ok());
        let err = json_marshal(&Nested(MAX_NESTING_DEPTH + 1), &[]).unwrap_err();
        assert_eq!(err.message, "exceeded max depth");
        let mut out: Vec<u8> = Vec::new();
        assert!(json_marshal_write(&mut out, &Nested(MAX_NESTING_DEPTH + 1), &[]).is_err());
        assert!(out.is_empty());
    }

    // Go jsontext decodes 10000 nested arrays and fails at the next `[`
    // with the pointer "/0" x 10000 and the offset 10000 (coder_test.go
    // "ArraysInvalid"). The text cuts the pointer to about 100 bytes
    // (jsonwire.TruncatePointer).
    #[test]
    fn decode_fails_past_max_nesting_depth() {
        let depth = MAX_NESTING_DEPTH + 1;
        let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let mut dec = JsonDecoder::new(input.as_bytes(), JsonOptions::default());
        let err = dec.skip_value().unwrap_err();
        let pointer = format!("{}/…/0{}", "/0".repeat(24), "/0".repeat(24));
        assert_eq!(
            err.message,
            format!("jsontext: exceeded max depth within \"{pointer}\" after offset 10000")
        );
    }
}

#[cfg(test)]
mod syntax_error_tests {
    use super::*;

    // Go N texts (encoding/json/v2 of Go 1.27, tsgo pin 673a5f17d713):
    // `json.Unmarshal` into a `jsontext.Value` (Go `ReadValue`, then the end
    // of input check) and `jsontext.Decoder.SkipValue` (Go `ReadToken` for
    // an object or array). `None` is the same text as `ReadValue`.
    #[test]
    fn syntax_errors_match_go() {
        let cases: [(&[u8], &str, Option<&str>); 18] = [
            (
                b"nulx",
                "jsontext: invalid character 'x' in literal null (expecting 'l') after offset 3",
                None,
            ),
            (
                b"1.x",
                "jsontext: invalid character 'x' in number (expecting digit) after offset 2",
                None,
            ),
            (b"-", "jsontext: unexpected EOF", None),
            (
                br#""a\u12x4""#,
                "jsontext: invalid escape sequence `\\u12x4` in string after offset 2",
                None,
            ),
            (
                br#""\ud800\u0041""#,
                "jsontext: invalid surrogate pair `\\ud800\\u0041` in string after offset 1",
                None,
            ),
            (
                b"\"a\x01\"",
                "jsontext: invalid character '\\x01' in string (expecting non-control character) after offset 2",
                None,
            ),
            (b"\"\xff\"", "jsontext: invalid UTF-8 after offset 1", None),
            (
                br#"{"a":1,}"#,
                "jsontext: invalid character '}' at start of string (expecting '\"') after offset 7",
                Some("jsontext: invalid character ',' at start of value after offset 6"),
            ),
            (
                br#"{"a" 1}"#,
                "jsontext: invalid character '1' after object name (expecting ':') within \"/a\" after offset 5",
                None,
            ),
            (
                br#"{"a":1:2}"#,
                "jsontext: invalid character ':' after object value (expecting ',' or '}') after offset 6",
                None,
            ),
            (
                b"[1,]",
                "jsontext: invalid character ']' at start of value within \"/1\" after offset 3",
                Some("jsontext: invalid character ',' at start of value after offset 2"),
            ),
            (
                br#"{"a":[}"#,
                "jsontext: invalid character '}' at start of value within \"/a/0\" after offset 6",
                None,
            ),
            (
                br#"{1:2}"#,
                "jsontext: invalid character '1' at start of string (expecting '\"') after offset 1",
                Some("jsontext: object member name must be a string after offset 1"),
            ),
            (
                br#"{"a":}"#,
                "jsontext: invalid character '}' at start of value within \"/a\" after offset 5",
                Some("jsontext: missing value after object name within \"/a\" after offset 5"),
            ),
            (
                br#"{"a":1} x"#,
                "jsontext: invalid character 'x' after top-level value after offset 8",
                None,
            ),
            (b" ", "jsontext: unexpected EOF after offset 1", None),
            (
                br#"{"a/b~c":{"d":[1,2,x]}}"#,
                "jsontext: invalid character 'x' at start of value within \"/a~1b~0c/d/2\" after offset 19",
                None,
            ),
            (
                b"[1,\xe2\x82",
                "jsontext: invalid character '\\xe2' at start of value within \"/1\" after offset 3",
                None,
            ),
        ];
        for (input, value, tokens) in cases {
            let mut dec = JsonDecoder::new(input, JsonOptions::default());
            let err = dec.read_value().and_then(|_| dec.check_eof()).unwrap_err();
            assert_eq!(err.message, value, "ReadValue of {input:?}");
            if input.ends_with(b" x") {
                continue;
            }
            let mut dec = JsonDecoder::new(input, JsonOptions::default());
            let err = dec.skip_value().unwrap_err();
            assert_eq!(
                err.message,
                tokens.unwrap_or(value),
                "SkipValue of {input:?}"
            );
        }
    }
}

#[cfg(test)]
mod marshal_indent_tests {
    use super::*;

    // Expected values come from Go JSON v2 `json.Marshal` with
    // `jsontext.WithIndentPrefix` and `jsontext.WithIndent`.
    #[test]
    fn multiline_output_matches_go() {
        assert_eq!(
            json_append_multiline(r#"["x",true,[],{},["a",["b"]]]"#, "", "  ").unwrap(),
            "[\n  \"x\",\n  true,\n  [],\n  {},\n  [\n    \"a\",\n    [\n      \"b\"\n    ]\n  ]\n]"
        );
        assert_eq!(
            json_append_multiline(r#"{"a":["x","y:{"],"b":[],"c":[[true],[]]}"#, " ", "\t")
                .unwrap(),
            "{\n \t\"a\": [\n \t\t\"x\",\n \t\t\"y:{\"\n \t],\n \t\"b\": [],\n \t\"c\": [\n \t\t[\n \t\t\ttrue\n \t\t],\n \t\t[]\n \t]\n }"
        );
        assert_eq!(json_marshal_indent("s", "", "  ").unwrap(), "\"s\"");
    }

    /// The compact `--showConfig` value of a config with 31 files (one name
    /// needs an escape).
    const SHOW_CONFIG_31: &str = r#"{"compilerOptions":{"lib":["es2022","dom"],"module":"nodenext","outDir":"./out","paths":{"@a/*":["./src/*"]},"strict":true,"target":"es2022","moduleResolution":"nodenext","moduleDetection":"force"},"files":["./m1.ts","./m10.ts","./m11.ts","./m12.ts","./m13.ts","./m14.ts","./m15.ts","./m16.ts","./m17.ts","./m18.ts","./m19.ts","./m2.ts","./m20.ts","./m21.ts","./m22.ts","./m23.ts","./m24.ts","./m25.ts","./m26.ts","./m27.ts","./m28.ts","./m29.ts","./m3.ts","./m30.ts","./m4.ts","./m5.ts","./m6.ts","./m7.ts","./m8.ts","./m9.ts","./q\"x.ts"],"exclude":["out"]}"#;

    // PORT: not in Go. The writes of Go's streaming encoder for
    // `SHOW_CONFIG_31`, recorded with strace from the pin N oracle: 83 149
    // 178 299 294 46 bytes.
    #[test]
    fn json_stream_pieces_match_go_show_config_writes() {
        let mut pieces = JsonStreamPieces::default();
        let out =
            json_append_multiline_pieces(SHOW_CONFIG_31, "", "    ", Some(&mut pieces)).unwrap();
        assert_eq!(out.len(), 1049);
        assert_eq!(pieces.ends, [83, 232, 410, 709, 1003, 1049]);
    }

    // The write loop of `json_marshal_indent_write`: one write of the
    // writer per piece, so `--showConfig` writes fd 1 in Go's pieces (the
    // system writer writes each one at once, `stdio::CliStdout`).
    #[test]
    fn json_marshal_indent_write_writes_go_pieces() {
        /// A value whose compact text is given.
        struct Compact(&'static str);
        impl MarshalerTo for Compact {
            fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
                enc.push_str(self.0);
                Ok(())
            }
        }
        /// Keeps each write as one piece, an empty one too, as the system
        /// writer writes each one (`GoOutput`).
        #[derive(Default)]
        struct Writes(Vec<Vec<u8>>);
        impl std::io::Write for Writes {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.write_all(buf)?;
                Ok(buf.len())
            }
            fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
                self.0.push(buf.to_vec());
                Ok(())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writes = Writes::default();
        json_marshal_indent_write(&mut writes, &Compact(SHOW_CONFIG_31), "", "    ").unwrap();
        let sizes: Vec<usize> = writes.0.iter().map(Vec::len).collect();
        assert_eq!(sizes, [83, 149, 178, 299, 294, 46]);
        let text = json_append_multiline(SHOW_CONFIG_31, "", "    ").unwrap();
        assert_eq!(writes.0.concat(), text.into_bytes());
    }
}

#[cfg(test)]
mod decode_string_tests {
    use super::*;

    fn token_ref_of(token: &JsonToken) -> JsonTokenRef<'static> {
        match token {
            JsonToken::Null => JsonTokenRef::Null,
            JsonToken::False => JsonTokenRef::False,
            JsonToken::True => JsonTokenRef::True,
            JsonToken::BeginObject => JsonTokenRef::BeginObject,
            JsonToken::EndObject => JsonTokenRef::EndObject,
            JsonToken::BeginArray => JsonTokenRef::BeginArray,
            JsonToken::EndArray => JsonTokenRef::EndArray,
            JsonToken::String(_) | JsonToken::Number(_) => unreachable!("compared by text"),
        }
    }

    // `scan_string` borrows only where `consume_string_chars` gives the
    // same text, and gives its end offset and errors everywhere.
    #[test]
    fn scan_string_matches_consume_string_chars() {
        let bodies: [&[u8]; 16] = [
            b"",
            b"plain ascii",
            b"tab\there",
            b"quote \\\" and \\\\ and \\/",
            b"\\u0041\\u00e9\\ud83d\\ude00",
            b"\\ud800 lone",
            "caf\u{e9} \u{1f600} \u{fdd0}".as_bytes(),
            b"bad \xff byte",
            b"overlong \xc0\x80",
            b"surrogate \xed\xa0\x80",
            b"cut \xe2\x82",
            b"ctl \x01",
            b"del \x7f",
            b"\\x bad escape",
            b"\\u12 short",
            b"unterminated",
        ];
        for allow_invalid_utf8 in [false, true] {
            let options = JsonOptions {
                allow_invalid_utf8,
                ..JsonOptions::default()
            };
            for body in bodies {
                for terminated in [true, false] {
                    let mut input = vec![b'"'];
                    input.extend_from_slice(body);
                    if terminated {
                        input.extend_from_slice(b"\" ,");
                    }
                    let dec = JsonDecoder::new(&input, options);
                    let fast = dec.scan_string(0).map(|(end, s)| (end, s.into_owned()));
                    let slow = dec.consume_string_chars(0);
                    assert_eq!(
                        fast, slow,
                        "{input:?} allow_invalid_utf8={allow_invalid_utf8}"
                    );
                }
            }
        }
    }

    // A JSON pointer holds Go's bytes of the names, where a real U+FDD0 is
    // one char, and `%q` quotes them (jsontext/errors.go:127 and :135). Texts
    // from Go N (encoding/json/v2 of Go 1.27.1, followups25 round b
    // tools-b/gojsonptr).
    #[test]
    fn pointer_names_are_quoted_from_go_bytes() {
        let read_all = |text: &[u8]| {
            let mut dec = JsonDecoder::new(
                text,
                JsonOptions {
                    allow_invalid_utf8: true,
                    ..JsonOptions::default()
                },
            );
            dec.skip_value().and_then(|()| dec.check_eof())
        };
        assert_eq!(
            read_all("{\"\\ufdd0\\ufdd0\":1,\"\u{fdd0}\u{fdd0}\":2}".as_bytes())
                .unwrap_err()
                .message,
            r#"jsontext: duplicate object member name "\ufdd0\ufdd0""#
        );
        assert_eq!(
            read_all("{\"\u{fdd0}\u{fdd0}\":{\"a\":1,\"a\":2}}".as_bytes())
                .unwrap_err()
                .message,
            r#"jsontext: duplicate object member name "a" within "/\ufdd0\ufdd0""#
        );
        assert_eq!(
            read_all(b"{\"x\xffy\":{\"a\":1,\"a\":2}}")
                .unwrap_err()
                .message,
            "jsontext: duplicate object member name \"a\" within \"/x\u{fffd}y\""
        );
    }

    // Names with and without escapes share one namespace per object, and a
    // later object starts with an empty one.
    #[test]
    fn duplicate_names() {
        let read_all = |text: &str| {
            let mut dec = JsonDecoder::new(text.as_bytes(), JsonOptions::default());
            dec.skip_value().and_then(|()| dec.check_eof())
        };
        // Go N (encoding/json/v2 of Go 1.27, tsgo pin 673a5f17d713) texts.
        assert_eq!(
            read_all(r#"{"a":1,"a":2}"#).unwrap_err().message,
            r#"jsontext: duplicate object member name "a""#
        );
        assert_eq!(
            read_all(r#"{"x":{"é":1,"é":2}}"#).unwrap_err().message,
            r#"jsontext: duplicate object member name "é" within "/x""#
        );
        assert!(read_all(r#"[{"a":1,"b":{"a":2}},{"a":3,"b":4}]"#).is_ok());
        let mut dec = JsonDecoder::new(br#"{"k":"v","n":1}"#, JsonOptions::default());
        let tokens: Vec<JsonToken> = std::iter::from_fn(|| dec.read_token().ok()).collect();
        let mut dec = JsonDecoder::new(
            "{\"k\\n\":\"\u{fdd0}\",\"n\":-1.5e3}".as_bytes(),
            JsonOptions::default(),
        );
        let refs: Vec<JsonTokenRef<'_>> =
            std::iter::from_fn(|| dec.read_token_ref().ok()).collect();
        let mut dec = JsonDecoder::new(
            "{\"k\\n\":\"\u{fdd0}\",\"n\":-1.5e3}".as_bytes(),
            JsonOptions::default(),
        );
        let owned: Vec<JsonToken> = std::iter::from_fn(|| dec.read_token().ok()).collect();
        assert_eq!(refs.len(), owned.len());
        for (token_ref, token) in refs.iter().zip(&owned) {
            match (token_ref, token) {
                (JsonTokenRef::String(a), JsonToken::String(b)) => assert_eq!(a, b),
                (JsonTokenRef::Number(a), JsonToken::Number(b)) => assert_eq!(a, b),
                (a, b) => assert_eq!(a.clone(), token_ref_of(b)),
            }
        }
        assert!(matches!(&refs[2], JsonTokenRef::String(Cow::Owned(_))));
        assert!(matches!(&refs[3], JsonTokenRef::String(Cow::Borrowed("n"))));
        assert_eq!(
            tokens,
            [
                JsonToken::BeginObject,
                JsonToken::String("k".into()),
                JsonToken::String("v".into()),
                JsonToken::String("n".into()),
                JsonToken::Number("1".into()),
                JsonToken::EndObject,
            ]
        );
    }
}
