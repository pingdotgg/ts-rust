//! Go JSON v2 default arshalers that `frontend/json.rs` lacks, for the
//! language-service port (plan contract C2).
//!
//! Behavior reference: `github.com/go-json-experiment/json`
//! v0.0.0-20260601182631-00ed12fed2a6 (`arshal_default.go`,
//! `arshal_any.go`, `jsontext/encode.go`, `jsontext/state.go`,
//! `internal/jsonwire`). Go reflects over types; each Rust type here has a
//! hand-written `MarshalerTo` / `UnmarshalerFrom` with the same default rules.
//!
//! Impls that already exist and are not added again: `json.rs` (String,
//! bool, f64 unmarshal; `FxHashMap<String, V>` unmarshal; str, String, bool,
//! `[T]`, `Vec<T>`, `IndexMap<String, V>` marshal) and `packagejson.rs`
//! (`IndexMap<String, V>` unmarshal, Go `collections.OrderedMap` rules).
//! `IndexMap<DocumentUri, V>` lives in `lsp/lsproto/lsp.rs`.
//!
//! Unmarshal errors of the arshalers here are Go v2 `SemanticError` texts
//! (see `SemanticError`). The JSON pointer is added by `unmarshal_root`; an
//! error that reaches the caller through `json.rs` `json_unmarshal` has no
//! pointer. The `json.rs` impls keep their own texts, except that their kind
//! errors (a string, a boolean or a number of the wrong kind) use
//! `unmarshal_kind_error`.
//!
//! PORT: integer arshalers do not port the stringified form that v2 uses
//! for integer map keys (no LSP type has integer keys).

use crate::frontend::prelude::*;

use std::any::Any;
use std::cell::RefCell;

/// Go `any` holding a value that the LSP layer can marshal
/// (`RequestMessage.Params`, `ResponseMessage.Result`, `Message.msg`).
/// Read it with `downcast_ref::<T>()`. Inbound LSP params and results are a
/// raw `JsonValue`; `lsproto::unmarshal_params` and `unmarshal_result`
/// decode them.
///
/// Never box a `Box<dyn AnyValue>` again: the blanket impl makes the box
/// itself an `AnyValue`, and a downcast of the outer box fails.
pub trait AnyValue: Any + MarshalerTo + std::fmt::Debug + Send {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + MarshalerTo + std::fmt::Debug + Send> AnyValue for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl dyn AnyValue {
    /// Go type assertion `v.(T)` for a non-nil `any`.
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// Go `any` decoded from JSON (LSPAny): nil, `bool`, `float64`, `string`,
/// `[]any` and `map[string]any`.
/// PORT: `Object` keeps insertion order; Go map order is random.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum LspAny {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<LspAny>),
    Object(IndexMap<String, LspAny>),
}

/// Go `json.Value` (`jsontext.Value`): raw JSON bytes.
/// PORT: Go nil and empty are both an empty `Vec`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonValue(pub Vec<u8>);

impl AsRef<[u8]> for JsonValue {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Go `omitzero` test (`reflect.Value.IsZero`, or a type's `IsZero`
/// method).
pub trait IsZero {
    fn is_zero(&self) -> bool;
}

impl IsZero for String {
    fn is_zero(&self) -> bool {
        self.is_empty()
    }
}

impl IsZero for bool {
    fn is_zero(&self) -> bool {
        !*self
    }
}

impl IsZero for i32 {
    fn is_zero(&self) -> bool {
        *self == 0
    }
}

impl IsZero for u32 {
    fn is_zero(&self) -> bool {
        *self == 0
    }
}

impl IsZero for u64 {
    fn is_zero(&self) -> bool {
        *self == 0
    }
}

// Go reflect: a float is zero only when all bits are zero, so -0 is not.
impl IsZero for f64 {
    fn is_zero(&self) -> bool {
        self.to_bits() == 0
    }
}

// PORT: Go omits only a nil slice; Rust cannot tell nil from empty.
impl<T> IsZero for Vec<T> {
    fn is_zero(&self) -> bool {
        self.is_empty()
    }
}

impl<T> IsZero for Option<T> {
    fn is_zero(&self) -> bool {
        self.is_none()
    }
}

// PORT: Go omits only a nil map; Rust cannot tell nil from empty.
impl<K, V, S> IsZero for IndexMap<K, V, S> {
    fn is_zero(&self) -> bool {
        self.is_empty()
    }
}

// Go: a nil `any` is zero; a non-nil `any` is not.
impl IsZero for LspAny {
    fn is_zero(&self) -> bool {
        matches!(self, LspAny::Null)
    }
}

impl IsZero for JsonValue {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

// Go jsontext `Kind.normalize`.
fn normalize_kind(c: u8) -> u8 {
    match c {
        b'-' | b'0'..=b'9' => b'0',
        _ => c,
    }
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn skip_ws(data: &[u8], mut p: usize) -> usize {
    while p < data.len() && is_ws(data[p]) {
        p += 1;
    }
    p
}

fn skip_ws_back(data: &[u8], mut p: usize) -> usize {
    while p > 0 && is_ws(data[p - 1]) {
        p -= 1;
    }
    p
}

/// Go `newUnmarshalErrorAfter(dec, t, nil)`: a default arshaler error for a
/// JSON value of kind `k` ("json: cannot unmarshal JSON string into Go
/// uint32").
#[must_use]
pub fn unmarshal_kind_error(k: u8, go_type: &str) -> JsonError {
    SemanticError::after(k, "", go_type, "").into_json_error()
}

// Go `newUnmarshalErrorAfterWithValue(dec, t, err)`: Go keeps the value of a
// string or a number, and the text shows it when it is shorter than 100 bytes.
pub(crate) fn unmarshal_value_error(val: &[u8], go_type: &str, detail: &str) -> JsonError {
    let k = val.first().map_or(0, |&c| normalize_kind(c));
    let val = if k == b'"' || k == b'0' {
        String::from_utf8_lossy(val).into_owned()
    } else {
        String::new()
    };
    SemanticError::after(k, &val, go_type, detail).into_json_error()
}

// ---------------------------------------------------------------------------
// Go v2 unmarshal errors
// ---------------------------------------------------------------------------

/// Where the decoder stands when an unmarshal error is made. It gives the Go
/// `where` argument of `appendStackPointer` (the JSON pointer) and the byte
/// offset of the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorPos {
    /// Before the value (Go `newUnmarshalErrorBefore`, where = +1).
    Before,
    /// Just after the value, or after the name of an object member (Go
    /// `newUnmarshalErrorAfter`, where = -1).
    After,
    /// Just after the closing token of an object or array that was read token
    /// by token (where = -1; the offset is that of the closing token).
    AfterEnd,
    /// Inside a value that the method read in part (Go `where` = 0 in
    /// `newSemanticErrorWithPosition`): the pointer names the last member of
    /// the innermost object only while that member's value is unread. The
    /// offset is the start of the last token or value read (Go
    /// `PreviousTokenOrValue`), which the method sets in `byte_offset`.
    Ambiguous,
}

/// Go v2 `*json.SemanticError` of an unmarshal (errors.go:73).
///
/// PORT: `JsonError` holds only text. The `SemanticError` of the last error
/// made on this thread is kept beside it, so that a caller up the stack can
/// fill in the Go type or the JSON pointer and write the text again, as Go
/// fills in the fields of the same error value.
#[derive(Clone, Debug)]
pub struct SemanticError {
    /// `false` while this is a plain error that an `UnmarshalJSONFrom` method
    /// returned: its text is `err` alone. The caller of the method wraps it
    /// (`wrap_method_error`).
    pub wrapped: bool,
    /// Go `JSONKind` (0 when unknown).
    pub json_kind: u8,
    /// Go `JSONValue`: the raw string or number that failed, or empty.
    pub json_value: String,
    /// Go `GoType.String()`.
    pub go_type: String,
    /// Go `JSONPointer`. `None` until `unmarshal_root` finds it.
    pub pointer: Option<String>,
    /// Go `ByteOffset`. The text shows it only when the pointer is empty.
    pub byte_offset: usize,
    pub pos: ErrorPos,
    /// The text of Go `Err` (empty for nil).
    pub err: String,
}

thread_local! {
    static LAST_ERROR: RefCell<Option<SemanticError>> = const { RefCell::new(None) };
}

impl SemanticError {
    // Go `newUnmarshalErrorAfter` for a default arshaler.
    fn after(json_kind: u8, json_value: &str, go_type: &str, err: &str) -> SemanticError {
        SemanticError {
            wrapped: true,
            json_kind,
            json_value: json_value.to_string(),
            go_type: go_type.to_string(),
            pointer: None,
            byte_offset: 0,
            pos: ErrorPos::After,
            err: err.to_string(),
        }
    }

    /// A plain error that an `UnmarshalJSONFrom` method returns (Go
    /// `fmt.Errorf` in the method). `pos` is where the method left the
    /// decoder.
    #[must_use]
    pub fn method(pos: ErrorPos, err: impl Into<String>) -> JsonError {
        SemanticError {
            wrapped: false,
            json_kind: 0,
            json_value: String::new(),
            go_type: String::new(),
            pointer: None,
            byte_offset: 0,
            pos,
            err: err.into(),
        }
        .into_json_error()
    }

    /// The `JsonError` with the text of this error. The error is kept for
    /// `SemanticError::of`.
    #[must_use]
    pub fn into_json_error(self) -> JsonError {
        let message = self.to_string();
        LAST_ERROR.with(|last| *last.borrow_mut() = Some(self));
        JsonError { message }
    }

    /// The `SemanticError` that made `err`, if `err` is the last one made on
    /// this thread. Other errors (syntax errors, `json.rs` texts) give `None`.
    #[must_use]
    pub fn of(err: &JsonError) -> Option<SemanticError> {
        LAST_ERROR.with(|last| {
            last.borrow()
                .as_ref()
                .filter(|s| s.to_string() == err.message)
                .cloned()
        })
    }
}

// Go: errors.go:356 (*SemanticError).Error
// PORT: Go picks "cannot" or "unable to" at random once per process; the
// port always writes "cannot".
impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.wrapped {
            return f.write_str(&self.err);
        }
        let mut sb = String::from("json: cannot unmarshal");
        match self.json_kind {
            b'n' => sb.push_str(" JSON null"),
            b'f' | b't' => sb.push_str(" JSON boolean"),
            b'"' => sb.push_str(" JSON string"),
            b'0' => sb.push_str(" JSON number"),
            b'{' | b'}' => sb.push_str(" JSON object"),
            b'[' | b']' => sb.push_str(" JSON array"),
            _ => {}
        }
        if !self.json_value.is_empty() && self.json_value.len() < 100 {
            sb.push(' ');
            sb.push_str(&self.json_value);
        }
        if !self.go_type.is_empty() {
            sb.push_str(" into Go ");
            // Go prints only the kind of a type whose name is longer than 100
            // bytes (the lsproto telemetry union).
            if self.go_type.len() > 100 {
                sb.push_str(go_kind_of_type_name(&self.go_type));
            } else {
                sb.push_str(&self.go_type);
            }
        }
        match self.pointer.as_deref() {
            Some(ptr) if !ptr.is_empty() => {
                sb.push_str(" within ");
                // PORT: the pointer is Go's bytes (`json::unquote_pointer_name`).
                sb.push_str(&crate::gostd::strconv::quote_bytes(
                    truncate_pointer(ptr, 100).as_bytes(),
                ));
            }
            _ if self.byte_offset > 0 => {
                sb.push_str(" after offset ");
                sb.push_str(&self.byte_offset.to_string());
            }
            _ => {}
        }
        if !self.err.is_empty() {
            sb.push_str(": ");
            sb.push_str(&self.err);
        }
        f.write_str(&sb)
    }
}

/// Go `reflect.Kind.String()` of the type that `go_type_name` names.
// PORT: the port keeps only the type name. A named type with a long name is
// an lsproto union, a struct.
fn go_kind_of_type_name(name: &str) -> &'static str {
    match name.as_bytes() {
        [b'm', b'a', b'p', b'[', ..] => "map",
        [b'[', b']', ..] => "slice",
        [b'[', ..] => "array",
        [b'*', ..] => "ptr",
        _ => "struct",
    }
}

// Go: internal/jsonwire/wire.go:165 TruncatePointer
pub(crate) fn truncate_pointer(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut i = n / 2;
    let mut j = s.len() - n / 2;

    // Avoid truncating a name if there are multiple names present.
    if let Some(k) = s.as_bytes()[..i].iter().rposition(|&c| c == b'/')
        && k > 0
    {
        i = k;
    }
    if let Some(k) = s.as_bytes()[j..].iter().position(|&c| c == b'/') {
        j += k + 1;
    }

    // Avoid truncation in the middle of a UTF-8 rune.
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }

    // Determine the right middle fragment to use.
    let mid = &s[i..j];
    let mut middle = match mid.matches('/').count() {
        0 => "…",
        1 => "…/…",
        _ => "…/…/…",
    };
    if mid.starts_with('/') && middle != "…" {
        middle = middle.strip_prefix('…').unwrap_or(middle);
    }
    if mid.ends_with('/') && middle != "…" {
        middle = middle.strip_suffix('…').unwrap_or(middle);
    }
    format!("{}{middle}{}", &s[..i], &s[j..])
}

/// Go `newUnmarshalErrorAfter(dec, t, err)` and `collapseSemanticErrors`
/// for the error `message` of a v1 `UnmarshalJSON` method of the Go type
/// `go_type`, which got the raw value `val`: a plain error gets the JSON kind
/// and the Go type; a `SemanticError` that the method returned stays as it
/// is, and `unmarshal_root` gives it the pointer.
#[must_use]
pub fn unmarshal_json_method_error(val: &[u8], go_type: &str, message: String) -> JsonError {
    let err = JsonError { message };
    if SemanticError::of(&err).is_some() {
        return err;
    }
    let k = val.first().map_or(0, |&c| normalize_kind(c));
    SemanticError::after(k, "", go_type, &err.message).into_json_error()
}

/// Go `newSemanticErrorWithPosition` for the type of an `UnmarshalJSONFrom`
/// method (arshal_methods.go:343): a plain error from the method gets the Go
/// type of `rust_type` (the `std::any::type_name` of the method's type). A
/// `SemanticError` keeps its type, and other errors pass through.
///
/// PORT: Go wraps at every type that has a method. The port wraps at every
/// `json_unmarshal_decode::<T>`, so the innermost type whose method made the
/// error names it. Only method types make plain errors (`SemanticError::method`);
/// the errors of the default arshalers have their type already. Not generic,
/// so the binary holds one copy.
#[must_use]
pub fn wrap_method_error(err: JsonError, rust_type: &str) -> JsonError {
    match SemanticError::of(&err) {
        Some(mut s) if !s.wrapped => {
            s.wrapped = true;
            s.go_type = go_type_string(rust_type);
            s.into_json_error()
        }
        _ => err,
    }
}

/// Go `json.Unmarshal(data, v)` (`json.rs` `json_unmarshal`) with the Go v2
/// error text: a plain error of the method of `T` is wrapped, and a
/// `SemanticError` gets its JSON pointer in `data` (or, for the root value,
/// its byte offset).
///
/// PORT: the pointer is found again from `data` and the decoder's input
/// offset where decoding stopped (Go `InputOffset`). A method that
/// unmarshals a raw sub-value (LSP unions) uses `unmarshal_read_value`.
/// The body is `unmarshal_root_dyn`, so each `T` adds only this call.
#[inline(always)]
pub fn unmarshal_root<T: UnmarshalerFrom>(data: &[u8], v: &mut T) -> Result<(), JsonError> {
    unmarshal_root_dyn(data, v, std::any::type_name::<T>())
}

/// `unmarshal_root` of a value whose type is erased. `rust_type` is the
/// `std::any::type_name` of the value's type (errors show its Go type).
#[inline(never)]
pub fn unmarshal_root_dyn(
    data: &[u8],
    v: &mut dyn UnmarshalerFrom,
    rust_type: &'static str,
) -> Result<(), JsonError> {
    let mut dec = JsonDecoder::new(data, JsonOptions::default());
    let err = match json_unmarshal_decode_dyn(&mut dec, v, rust_type) {
        Ok(()) => return dec.check_eof(),
        Err(err) => err,
    };
    let Some(mut s) = SemanticError::of(&err) else {
        return Err(err);
    };
    if !s.wrapped || s.pointer.is_some() {
        return Err(err);
    }
    let end = dec.input_offset();
    s.pointer = Some(stack_pointer(&stack_at(data, end), s.pos));
    s.byte_offset = match s.pos {
        // Go `cmp.Or(serr.ByteOffset, offset)`: the method set the start of
        // its last token.
        ErrorPos::Ambiguous if s.byte_offset > 0 => s.byte_offset,
        pos => error_offset(data, end, pos),
    };
    Err(s.into_json_error())
}

/// The byte offset that `unmarshal_root` gives an error at `end` of `data`
/// (the text shows it only when the pointer is empty).
#[must_use]
pub fn error_offset(data: &[u8], end: usize, pos: ErrorPos) -> usize {
    match pos {
        // Go `CountNextDelimWhitespace`: the next token.
        ErrorPos::Before => {
            let p = skip_ws(data, end);
            if p < data.len() && matches!(data[p], b',' | b':') {
                skip_ws(data, p + 1)
            } else {
                p
            }
        }
        // The text shows the offset only for the root value itself.
        ErrorPos::After | ErrorPos::Ambiguous => skip_ws(data, 0),
        ErrorPos::AfterEnd => end.saturating_sub(1),
    }
}

/// The JSON pointer that `unmarshal_root` gives an error at `end` of `data`.
/// PORT: not in Go; Go keeps the decoder state. The lsproto union decoder
/// finds the pointer of an error in a member that it read earlier.
#[must_use]
pub fn error_pointer(data: &[u8], end: usize, pos: ErrorPos) -> String {
    stack_pointer(&stack_at(data, end), pos)
}

/// Go `json.Unmarshal(data, v)` inside an `UnmarshalJSONFrom` method, where
/// `data` is the value that the method read from its decoder (the LSP
/// unions). An error keeps the JSON pointer inside `data` (Go does not add
/// the pointer of `data` to it). An error at `data` itself has the empty
/// pointer, which the caller's arshaler fills in with the pointer of the
/// value the method read: here the error is given `ErrorPos::After` and no
/// pointer, so `unmarshal_root` finds that value.
#[inline(always)]
pub fn unmarshal_read_value<T: UnmarshalerFrom>(data: &[u8], v: &mut T) -> Result<(), JsonError> {
    unmarshal_read_value_dyn(data, v, std::any::type_name::<T>())
}

/// `unmarshal_read_value` of a value whose type is erased.
#[inline(never)]
pub fn unmarshal_read_value_dyn(
    data: &[u8],
    v: &mut dyn UnmarshalerFrom,
    rust_type: &'static str,
) -> Result<(), JsonError> {
    let err = match unmarshal_root_dyn(data, v, rust_type) {
        Ok(()) => return Ok(()),
        Err(err) => err,
    };
    match SemanticError::of(&err) {
        Some(mut s) if s.pointer.as_deref() == Some("") => {
            s.pointer = None;
            s.pos = ErrorPos::After;
            s.byte_offset = 0;
            Err(s.into_json_error())
        }
        _ => Err(err),
    }
}

/// Go v2 `json.Unmarshal(data, &v)` for a type `T` with a v1 `UnmarshalJSON`
/// method (arshal_methods.go:282); `unmarshal_json` is the method. The raw
/// value is read first, so a syntax error comes back as it is. An error of
/// the method is wrapped in a `SemanticError` for the JSON kind of the value
/// and the Go type of `T`, and stays in the error chain.
///
/// Used for `lsproto.Message` (lsp server.go:121 `json.Unmarshal(data, req)`).
///
/// PORT: Go collapses a `SemanticError` that the method returns into the
/// outer one; no port method returns one.
pub fn unmarshal_json_method<T: ?Sized>(
    data: &[u8],
    unmarshal_json: impl FnOnce(&[u8]) -> Result<(), crate::gostd::GoError>,
) -> Result<(), crate::gostd::GoError> {
    let mut dec = JsonDecoder::new(data, JsonOptions::default());
    let val = dec.read_value().map_err(crate::gostd::errors::from_value)?;
    if let Err(err) = unmarshal_json(val) {
        let s = SemanticError {
            pointer: Some(String::new()),
            byte_offset: skip_ws(data, 0),
            ..SemanticError::after(
                val.first().map_or(0, |&c| normalize_kind(c)),
                "",
                &go_type_name::<T>(),
                &err.error(),
            )
        };
        return Err(crate::gostd::errors::errorf(s.to_string(), vec![err]));
    }
    dec.check_eof().map_err(crate::gostd::errors::from_value)
}

// One open object or array of the Go decoder state (jsontext `stateEntry`,
// with the last object name read in it).
struct StackEntry {
    is_object: bool,
    // Names and values read so far.
    len: usize,
    name: String,
}

// The open objects and arrays below the top level after the tokens in
// `data[..end]`.
fn stack_at(data: &[u8], end: usize) -> Vec<StackEntry> {
    let mut stack: Vec<StackEntry> = Vec::new();
    let mut i = 0;
    while i < end {
        let c = data[i];
        match c {
            b'{' | b'[' => {
                if let Some(e) = stack.last_mut() {
                    e.len += 1;
                }
                stack.push(StackEntry {
                    is_object: c == b'{',
                    len: 0,
                    name: String::new(),
                });
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < data.len() && data[i] != b'"' {
                    i += if data[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(data.len());
                if let Some(e) = stack.last_mut() {
                    if e.is_object && e.len.is_multiple_of(2) {
                        e.name = crate::frontend::json::unquote_pointer_name(&data[start..i]);
                    }
                    e.len += 1;
                }
            }
            b',' | b':' => i += 1,
            c if is_ws(c) => i += 1,
            _ => {
                // A number or a literal.
                if let Some(e) = stack.last_mut() {
                    e.len += 1;
                }
                while i < data.len()
                    && !is_ws(data[i])
                    && !matches!(data[i], b',' | b':' | b'}' | b']')
                {
                    i += 1;
                }
            }
        }
    }
    stack
}

// Go: jsontext/state.go:180 appendStackPointer, with where = +1 for
// `Before`, 0 for `Ambiguous` and -1 otherwise.
fn stack_pointer(stack: &[StackEntry], pos: ErrorPos) -> String {
    let before = pos == ErrorPos::Before;
    let ambiguous = pos == ErrorPos::Ambiguous;
    let mut b = String::new();
    for (i, e) in stack.iter().enumerate() {
        // By default point to the previous array element.
        let mut index = e.len.wrapping_sub(1);
        if i == stack.len() - 1 {
            // Go `needObjectValue`: an object whose last member has no value yet.
            let need_object_value = e.is_object && !e.len.is_multiple_of(2);
            if (!before && !ambiguous && e.len == 0)
                || (ambiguous && !need_object_value)
                || (before && e.is_object && e.len.is_multiple_of(2))
            {
                return b;
            }
            if before && !e.is_object {
                // Point to the next array element.
                index = e.len;
            }
        }
        b.push('/');
        if e.is_object {
            // Per RFC 6901, section 3, escape '~' and '/' characters.
            for c in e.name.chars() {
                match c {
                    '~' => b.push_str("~0"),
                    '/' => b.push_str("~1"),
                    c => b.push(c),
                }
            }
        } else {
            b.push_str(&index.to_string());
        }
    }
    b
}

/// Go `reflect.Type.String()` of the Go type that the Rust type `T` stands
/// for: api, lsproto and project types get their package name, `Vec<T>` is `[]T`,
/// `Option<T>` is `*T`, `Box<T>` is `T`, a map is `map[K]V`, and the Rust
/// number types are the Go ones of the same size.
///
/// PORT: built from `std::any::type_name`. A Go slice of pointers (`[]*T`)
/// prints as `[]T`.
#[must_use]
pub fn go_type_name<T: ?Sized>() -> String {
    go_type_string(std::any::type_name::<T>())
}

/// `go_type_name` of the Rust type that `rust` (`std::any::type_name`) names.
/// Error paths take the name as a string so that they are not generic.
pub fn go_type_string(rust: &str) -> String {
    let (path, args) = match rust.find('<') {
        Some(i) if rust.ends_with('>') => {
            (&rust[..i], split_type_args(&rust[i + 1..rust.len() - 1]))
        }
        _ => (rust, Vec::new()),
    };
    let name = path.rsplit("::").next().unwrap_or(path);
    match (name, args.as_slice()) {
        ("Vec", [t]) => format!("[]{}", go_type_string(t)),
        ("Option", [t]) => format!("*{}", go_type_string(t)),
        ("Box", [t]) => go_type_string(t),
        ("IndexMap" | "HashMap", [k, v, ..]) => {
            format!("map[{}]{}", go_type_string(k), go_type_string(v))
        }
        ("bool", []) => "bool".to_string(),
        ("u8", []) => "uint8".to_string(),
        ("u32", []) => "uint32".to_string(),
        ("u64", []) => "uint64".to_string(),
        ("usize", []) => "uint".to_string(),
        ("i32", []) => "int32".to_string(),
        ("i64", []) => "int64".to_string(),
        ("isize", []) => "int".to_string(),
        ("f64", []) => "float64".to_string(),
        ("String" | "str", []) => "string".to_string(),
        ("LspAny", []) => "interface {}".to_string(),
        _ if path.contains("::lsproto::") => format!("lsproto.{name}"),
        _ if path.contains("::api::") => format!("api.{name}"),
        // Go package project (project.ID, project.SyntheticProjectID).
        _ if path.rsplit("::").nth(1) == Some("project") => format!("project.{name}"),
        _ => name.to_string(),
    }
}

// The top-level arguments of a generic Rust type name ("A, B<C, D>").
fn split_type_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

// Go: jsonwire/decode.go:593 ParseUint
// ParseUint parses b as a decimal unsigned integer according to
// a strict subset of the JSON number grammar, returning the value if valid.
// It returns (0, false) if there is a syntax error and
// returns (math.MaxUint64, false) if there is an overflow.
fn parse_uint(b: &[u8]) -> (u64, bool) {
    const UNSAFE_WIDTH: usize = 20; // len(fmt.Sprint(uint64(math.MaxUint64)))
    let mut v: u64 = 0;
    let mut n = 0;
    while b.len() > n && b[n].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add(u64::from(b[n] - b'0'));
        n += 1;
    }
    if n == 0 || b.len() != n || (b[0] == b'0' && b != b"0") {
        return (0, false);
    }
    if n >= UNSAFE_WIDTH && (b[0] != b'1' || v < 10_000_000_000_000_000_000 || n > UNSAFE_WIDTH) {
        return (u64::MAX, false);
    }
    (v, true)
}

// Go: arshal_default.go:492 makeIntArshaler (unmarshal)
// Returns the value to store. `null` stores 0. A JSON string is an error
// (not stringified), and so is a number with a fraction or exponent.
fn unmarshal_int(dec: &mut JsonDecoder<'_>, bits: u32, go_type: &str) -> Result<i64, JsonError> {
    let val = dec.read_value()?;
    let k = val.first().map_or(0, |&c| normalize_kind(c));
    match k {
        b'n' => Ok(0),
        b'0' => {
            let neg = val[0] == b'-';
            let neg_offset = usize::from(neg);
            let (n, ok) = parse_uint(&val[neg_offset..]);
            let max_int = 1u64 << (bits - 1);
            let mut overflow = (neg && n > max_int) || (!neg && n > max_int - 1);
            if !ok {
                if n != u64::MAX {
                    return Err(unmarshal_value_error(val, go_type, "invalid syntax"));
                }
                overflow = true;
            }
            if overflow {
                return Err(unmarshal_value_error(val, go_type, "value out of range"));
            }
            if neg {
                Ok((n as i64).wrapping_neg())
            } else {
                Ok(n as i64)
            }
        }
        _ => Err(unmarshal_kind_error(k, go_type)),
    }
}

// Go: arshal_default.go:591 makeUintArshaler (unmarshal)
fn unmarshal_uint(dec: &mut JsonDecoder<'_>, bits: u32, go_type: &str) -> Result<u64, JsonError> {
    let val = dec.read_value()?;
    let k = val.first().map_or(0, |&c| normalize_kind(c));
    match k {
        b'n' => Ok(0),
        b'0' => {
            let (n, ok) = parse_uint(val);
            // Go `uint64(1) << bits` is 0 for 64 bits, so `maxUint-1` wraps.
            let max_uint = 1u64.checked_shl(bits).unwrap_or(0);
            let mut overflow = n > max_uint.wrapping_sub(1);
            if !ok {
                if n != u64::MAX {
                    return Err(unmarshal_value_error(val, go_type, "invalid syntax"));
                }
                overflow = true;
            }
            if overflow {
                return Err(unmarshal_value_error(val, go_type, "value out of range"));
            }
            Ok(n)
        }
        _ => Err(unmarshal_kind_error(k, go_type)),
    }
}

/// Go v2 uint arshaler for a Go named uint type such as `api.SnapshotID`:
/// errors name `go_type`. The Go size is the size of `U`.
pub fn unmarshal_uint_as<U: TryFrom<u64>>(
    dec: &mut JsonDecoder<'_>,
    go_type: &str,
) -> Result<U, JsonError> {
    let bits = u32::try_from(std::mem::size_of::<U>() * 8).unwrap_or(64);
    let n = unmarshal_uint(dec, bits, go_type)?;
    // `unmarshal_uint` checked the range.
    U::try_from(n).map_err(|_| unmarshal_kind_error(b'0', go_type))
}

/// Go v2 int arshaler for a Go named int type such as `lsproto.LogVerbosity`:
/// errors name `go_type`. The Go size is the size of `I`.
pub fn unmarshal_int_as<I: TryFrom<i64>>(
    dec: &mut JsonDecoder<'_>,
    go_type: &str,
) -> Result<I, JsonError> {
    let bits = u32::try_from(std::mem::size_of::<I>() * 8).unwrap_or(64);
    let n = unmarshal_int(dec, bits, go_type)?;
    // `unmarshal_int` checked the range.
    I::try_from(n).map_err(|_| unmarshal_kind_error(b'0', go_type))
}

/// Go v2 string arshaler (arshal_default.go:257) for a Go named string type
/// such as `lsproto.DocumentUri`: null sets "", a string sets the value, and
/// any other kind is an error that names `go_type`.
pub fn unmarshal_string_as(
    dec: &mut JsonDecoder<'_>,
    s: &mut String,
    go_type: &str,
) -> Result<(), JsonError> {
    match dec.peek_kind() {
        b'n' | b'"' => s.unmarshal_json_from(dec),
        _ => {
            let val = dec.read_value()?;
            let k = val.first().map_or(0, |&c| normalize_kind(c));
            Err(unmarshal_kind_error(k, go_type))
        }
    }
}

// Go: jsonwire/encode.go:213 AppendFloat (bits == 64)
// Go formats with strconv 'f' or 'e' and precision -1 (shortest digits that
// round-trip). Rust `{}` and `{:e}` print the same shortest digits.
fn append_float(dst: &mut String, src: f64) {
    let abs = src.abs();
    let mut fmt_e = false;
    if abs != 0.0 && (abs < 1e-6 || abs >= 1e21) {
        fmt_e = true;
    }
    if !fmt_e {
        dst.push_str(&format!("{src}"));
        return;
    }
    // Go 'e' writes the exponent sign and at least two digits; then
    // "e-09" is cleaned up to "e-9".
    let s = format!("{src:e}");
    let (mantissa, exp) = s.split_once('e').expect("LowerExp output has an exponent");
    let exp: i32 = exp.parse().expect("LowerExp exponent is an integer");
    dst.push_str(mantissa);
    dst.push('e');
    if exp < 0 {
        dst.push('-');
    } else {
        dst.push('+');
        if exp < 10 {
            dst.push('0');
        }
    }
    dst.push_str(&exp.unsigned_abs().to_string());
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

// Go: arshal_default.go:470 makeIntArshaler (marshal)
impl MarshalerTo for i32 {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str(&self.to_string());
        Ok(())
    }
}

// Go: arshal_default.go:470 makeIntArshaler (marshal), for Go `int`
impl MarshalerTo for i64 {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str(&self.to_string());
        Ok(())
    }
}

// Go: arshal_default.go:569 makeUintArshaler (marshal)
impl MarshalerTo for u32 {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str(&self.to_string());
        Ok(())
    }
}

// Go: arshal_default.go:569 makeUintArshaler (marshal)
impl MarshalerTo for u64 {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str(&self.to_string());
        Ok(())
    }
}

// Go: arshal_default.go:659 makeFloatArshaler (marshal)
// NaN and infinities are an error ("nonfinite" format is not used).
impl MarshalerTo for f64 {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let fv = *self;
        if fv.is_nan() || fv.is_infinite() {
            // Go `%v` of a float: NaN, +Inf, -Inf.
            let v = if fv.is_nan() {
                "NaN"
            } else if fv > 0.0 {
                "+Inf"
            } else {
                "-Inf"
            };
            return Err(JsonError {
                message: format!("cannot marshal from Go float64: unsupported value: {v}"),
            });
        }
        append_float(enc, fv);
        Ok(())
    }
}

// Go: arshal_default.go:492 makeIntArshaler (unmarshal)
impl UnmarshalerFrom for i32 {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        *self = unmarshal_int(dec, 32, "int32")? as i32;
        Ok(())
    }
}

// Go: arshal_default.go:591 makeUintArshaler (unmarshal)
impl UnmarshalerFrom for u32 {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        *self = unmarshal_uint(dec, 32, "uint32")? as u32;
        Ok(())
    }
}

// Go: arshal_default.go:591 makeUintArshaler (unmarshal)
impl UnmarshalerFrom for u64 {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        *self = unmarshal_uint(dec, 64, "uint64")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pointers, slices, arrays
// ---------------------------------------------------------------------------

// Go: arshal_default.go:1714 makePointerArshaler (marshal): nil writes null.
impl<T: MarshalerTo> MarshalerTo for Option<T> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        match self {
            None => {
                enc.push_str("null");
                Ok(())
            }
            Some(v) => v.marshal_json_to(enc),
        }
    }
}

// Go: arshal_default.go:1742 makePointerArshaler (unmarshal): null sets nil;
// otherwise a nil pointer gets a new zero value and the value decodes into
// the pointee (merging into an existing one).
// PORT: the body is `unmarshal_option_dyn`, so each `T` adds only the two
// `OptionSink` methods.
impl<T: UnmarshalerFrom + Default> UnmarshalerFrom for Option<T> {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_option_dyn(dec, self)
    }
}

// The parts of the pointer arshaler that depend on the pointee type.
trait OptionSink {
    fn set_none(&mut self);
    // The pointee, made zero when there is none, and the
    // `std::any::type_name` of its type.
    fn get_or_insert_default(&mut self) -> (&mut dyn UnmarshalerFrom, &'static str);
}

impl<T: UnmarshalerFrom + Default> OptionSink for Option<T> {
    fn set_none(&mut self) {
        *self = None;
    }

    fn get_or_insert_default(&mut self) -> (&mut dyn UnmarshalerFrom, &'static str) {
        (
            self.get_or_insert_with(T::default),
            std::any::type_name::<T>(),
        )
    }
}

// The pointer arshaler for an `Option` whose pointee type is erased: one
// copy for all pointee types.
#[inline(never)]
fn unmarshal_option_dyn(
    dec: &mut JsonDecoder<'_>,
    v: &mut dyn OptionSink,
) -> Result<(), JsonError> {
    if dec.peek_kind() == b'n' {
        dec.read_token()?;
        v.set_none();
        return Ok(());
    }
    let (e, pointee_type) = v.get_or_insert_default();
    json_unmarshal_decode_dyn(dec, e, pointee_type)
}

// PORT: `Box<T>` is a Go pointer on a type cycle. `null` is handled by the
// enclosing `Option`, so the box only forwards.
impl<T: MarshalerTo + ?Sized> MarshalerTo for Box<T> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        (**self).marshal_json_to(enc)
    }
}

impl<T: UnmarshalerFrom + ?Sized> UnmarshalerFrom for Box<T> {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        (**self).unmarshal_json_from(dec)
    }
}

// Go: arshal_default.go:1528 makeSliceArshaler (unmarshal): null sets nil,
// each element starts from its zero value, `[]` sets an empty slice.
// PORT: nil and empty are both an empty `Vec`.
// PORT: the body is `unmarshal_vec_dyn`, so each `T` adds only the two
// `VecSink` methods.
impl<T: UnmarshalerFrom + Default> UnmarshalerFrom for Vec<T> {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_vec_dyn(dec, self, std::any::type_name::<Self>())
    }
}

// The parts of the slice arshaler that depend on the element type.
trait VecSink {
    fn clear_elements(&mut self);
    // Pushes a zero element; returns it and the `std::any::type_name` of
    // its type.
    fn push_default(&mut self) -> (&mut dyn UnmarshalerFrom, &'static str);
}

impl<T: UnmarshalerFrom + Default> VecSink for Vec<T> {
    fn clear_elements(&mut self) {
        self.clear();
    }

    fn push_default(&mut self) -> (&mut dyn UnmarshalerFrom, &'static str) {
        self.push(T::default());
        let v = self.last_mut().expect("element was just pushed");
        (v, std::any::type_name::<T>())
    }
}

// The slice arshaler for a `Vec` whose element type is erased: one copy for
// all element types. `rust_type` is the `std::any::type_name` of the `Vec`.
#[inline(never)]
fn unmarshal_vec_dyn(
    dec: &mut JsonDecoder<'_>,
    v: &mut dyn VecSink,
    rust_type: &'static str,
) -> Result<(), JsonError> {
    let tok = dec.read_token()?;
    match tok {
        JsonToken::Null => {
            v.clear_elements();
            Ok(())
        }
        JsonToken::BeginArray => {
            v.clear_elements();
            while dec.peek_kind() != b']' {
                let (e, elem_type) = v.push_default();
                json_unmarshal_decode_dyn(dec, e, elem_type)?;
            }
            dec.read_token()?;
            Ok(())
        }
        // Go `newUnmarshalErrorAfterWithSkipping` skips the rest of the
        // value only with legacy semantics.
        _ => Err(unmarshal_kind_error(tok.kind(), &go_type_string(rust_type))),
    }
}

// Go: arshal_default.go:1612 makeArrayArshaler (marshal)
impl MarshalerTo for [u32; 2] {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.as_slice().marshal_json_to(enc)
    }
}

// Go: arshal_default.go:1640 makeArrayArshaler (unmarshal): null sets the
// zero array; the JSON array must have exactly 2 elements.
impl UnmarshalerFrom for [u32; 2] {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let tok = dec.read_token()?;
        match tok {
            JsonToken::Null => {
                *self = [0, 0];
                Ok(())
            }
            JsonToken::BeginArray => {
                let n = self.len();
                let mut i = 0;
                let mut err: Option<&str> = None;
                while dec.peek_kind() != b']' {
                    if i >= n {
                        dec.skip_value()?;
                        err = Some("too many array elements");
                        continue;
                    }
                    self[i] = 0;
                    json_unmarshal_decode(dec, &mut self[i])?;
                    i += 1;
                }
                while i < n {
                    self[i] = 0;
                    err = Some("too few array elements");
                    i += 1;
                }
                dec.read_token()?;
                if let Some(err) = err {
                    // Go `newUnmarshalErrorAfter` after the closing `]`.
                    let s = SemanticError {
                        pos: ErrorPos::AfterEnd,
                        ..SemanticError::after(b']', "", "[2]uint32", err)
                    };
                    return Err(s.into_json_error());
                }
                Ok(())
            }
            // Go `newUnmarshalErrorAfterWithSkipping` skips the rest of the
            // value only with legacy semantics.
            _ => Err(unmarshal_kind_error(tok.kind(), "[2]uint32")),
        }
    }
}

// ---------------------------------------------------------------------------
// any (LspAny)
// ---------------------------------------------------------------------------

// Go: arshal_any.go:32 marshalValueAny
// PORT: objects write in insertion order (Go map order is random).
impl MarshalerTo for LspAny {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        match self {
            LspAny::Null => {
                enc.push_str("null");
                Ok(())
            }
            LspAny::Bool(v) => v.marshal_json_to(enc),
            LspAny::String(v) => v.marshal_json_to(enc),
            LspAny::Number(v) => v.marshal_json_to(enc),
            LspAny::Object(v) => v.marshal_json_to(enc),
            LspAny::Array(v) => v.marshal_json_to(enc),
        }
    }
}

// Go: arshal_default.go:1847 makeInterfaceArshaler (unmarshal, `any`)
// `null` sets nil. A nil `any` decodes a fresh value (`unmarshalValueAny`,
// or the typed arshalers when duplicate names are allowed). A non-nil `any`
// decodes into a copy of its dynamic value.
// PORT: `IndexMap<String, V>` decodes with the `packagejson.rs` impl
// (OrderedMap rules), so merging into an existing object with
// `AllowDuplicateNames` replaces values instead of merging them.
impl UnmarshalerFrom for LspAny {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            *self = LspAny::Null;
            return Ok(());
        }
        if matches!(self, LspAny::Null) {
            if !dec.options.allow_duplicate_names {
                *self = unmarshal_value_any(dec)?;
                return Ok(());
            }
            *self = match dec.peek_kind() {
                b'f' | b't' => LspAny::Bool(false),
                b'"' => LspAny::String(String::new()),
                b'0' => LspAny::Number(0.0),
                b'{' => LspAny::Object(IndexMap::new()),
                b'[' => LspAny::Array(Vec::new()),
                _ => {
                    // An invalid kind: ReadValue reports the error.
                    dec.read_value()?;
                    return Ok(());
                }
            };
        }
        match self {
            LspAny::Null => unreachable!("null was replaced above"),
            LspAny::Bool(v) => v.unmarshal_json_from(dec),
            LspAny::String(v) => v.unmarshal_json_from(dec),
            LspAny::Number(v) => v.unmarshal_json_from(dec),
            LspAny::Object(v) => v.unmarshal_json_from(dec),
            LspAny::Array(v) => v.unmarshal_json_from(dec),
        }
    }
}

// Go: arshal_any.go:64 unmarshalValueAny
// PORT: Go also returns the partial value with an error; callers here stop
// on the first error, so only the error is returned.
pub fn unmarshal_value_any(dec: &mut JsonDecoder<'_>) -> Result<LspAny, JsonError> {
    match dec.peek_kind() {
        b'{' => unmarshal_object_any(dec).map(LspAny::Object),
        b'[' => unmarshal_array_any(dec).map(LspAny::Array),
        // Go reads any other value with `ReadValue`, which fails at a `}` or
        // `]` with its own error. For the other kinds `read_token` is the
        // same read.
        b'}' | b']' => {
            dec.read_value()?;
            unreachable!("a value cannot start with '}}' or ']'")
        }
        _ => match dec.read_token()? {
            JsonToken::Null => Ok(LspAny::Null),
            JsonToken::False => Ok(LspAny::Bool(false)),
            JsonToken::True => Ok(LspAny::Bool(true)),
            JsonToken::String(s) => Ok(LspAny::String(s)),
            JsonToken::Number(raw) => {
                // The strict JSON grammar is a subset of the Rust float
                // syntax; both round to nearest, like strconv.ParseFloat.
                let fv: f64 = raw.parse().map_err(|_| JsonError {
                    message: "invalid number".to_string(),
                })?;
                if fv.is_infinite() {
                    return Err(unmarshal_value_error(
                        raw.as_bytes(),
                        "float64",
                        "value out of range",
                    ));
                }
                Ok(LspAny::Number(fv))
            }
            tok => panic!("BUG: invalid kind: {}", char::from(tok.kind())),
        },
    }
}

// Go: arshal_any.go:177 unmarshalObjectAny
fn unmarshal_object_any(dec: &mut JsonDecoder<'_>) -> Result<IndexMap<String, LspAny>, JsonError> {
    let tok = dec.read_token()?;
    if tok != JsonToken::BeginObject {
        panic!("BUG: invalid kind: {}", char::from(tok.kind()));
    }
    let mut obj = IndexMap::new();
    // A Go map guarantees that each entry has a unique key.
    // The only possibility of duplicates is due to invalid UTF-8.
    if !dec.options.allow_invalid_utf8 {
        dec.disable_namespace();
    }
    while dec.peek_kind() != b'}' {
        let JsonToken::String(name) = dec.read_token()? else {
            unreachable!("object member names are strings")
        };

        // Manually check for duplicate names.
        if obj.contains_key(&name) {
            return Err(dec.duplicate_name_error());
        }

        let val = unmarshal_value_any(dec)?;
        obj.insert(name, val);
    }
    dec.read_token()?;
    Ok(obj)
}

// Go: arshal_any.go:266 unmarshalArrayAny
fn unmarshal_array_any(dec: &mut JsonDecoder<'_>) -> Result<Vec<LspAny>, JsonError> {
    let tok = dec.read_token()?;
    if tok != JsonToken::BeginArray {
        panic!("BUG: invalid kind: {}", char::from(tok.kind()));
    }
    let mut arr = Vec::new();
    while dec.peek_kind() != b']' {
        let val = unmarshal_value_any(dec)?;
        arr.push(val);
    }
    dec.read_token()?;
    Ok(arr)
}

// Go: arshal_default.go:1847 makeInterfaceArshaler (unmarshal) for a Go
// `any` field held as `Option<Box<dyn AnyValue>>` (`RequestMessage.Params`,
// `ResponseMessage.Result`). `null` sets nil; a nil `any` gets a fresh
// `LspAny`; an existing `LspAny` decodes in place.
pub fn unmarshal_any_interface(
    dec: &mut JsonDecoder<'_>,
    va: &mut Option<Box<dyn AnyValue>>,
) -> Result<(), JsonError> {
    if dec.peek_kind() == b'n' {
        dec.read_token()?;
        *va = None;
        return Ok(());
    }
    if let Some(existing) = va {
        let existing: &mut dyn Any = &mut **existing;
        return match existing.downcast_mut::<LspAny>() {
            Some(v) => v.unmarshal_json_from(dec),
            None => unported!("json.Unmarshal into a non-nil any of a typed value"),
        };
    }
    let mut v = LspAny::Null;
    v.unmarshal_json_from(dec)?;
    *va = Some(Box::new(v));
    Ok(())
}

// ---------------------------------------------------------------------------
// json.Value
// ---------------------------------------------------------------------------

// Go: jsontext/value.go:238 Value.MarshalJSON, then the encoder writes the
// value with `WriteValue` (compact, canonical strings).
impl MarshalerTo for JsonValue {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        // NOTE: This matches the behavior of v1 json.RawMessage.MarshalJSON.
        if self.0.is_empty() {
            enc.push_str("null");
            return Ok(());
        }
        write_value(enc, &self.0)
    }
}

// Go: jsontext/value.go:248 Value.UnmarshalJSON: a copy of the raw input,
// `null` included.
impl UnmarshalerFrom for JsonValue {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let val = dec.read_value()?;
        self.0.clear();
        self.0.extend_from_slice(val);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoder whitespace (compact and multiline)
// ---------------------------------------------------------------------------

// Go jsontext encoder state for `WriteToken` / `reformatValue`: one entry
// per open object or array (is object, number of names and values).
struct TokenWriter<'i> {
    stack: Vec<(bool, usize)>,
    top_len: usize,
    // Some((prefix, indent)) when `Multiline` is set.
    multiline: Option<(&'i str, &'i str)>,
}

impl TokenWriter<'_> {
    fn last(&self) -> (bool, usize) {
        self.stack.last().copied().unwrap_or((false, self.top_len))
    }

    fn increment(&mut self) {
        match self.stack.last_mut() {
            Some(e) => e.1 += 1,
            None => self.top_len += 1,
        }
    }

    // Go: jsontext/state.go:372 NeedIndent
    fn need_indent(&self, next: u8) -> usize {
        let will_end = next == b'}' || next == b']';
        let depth = self.stack.len() + 1;
        let (is_object, len) = self.last();
        let need_object_value = is_object && len % 2 == 1;
        let need_implicit_comma = !need_object_value && len > 0 && !will_end;
        if depth == 1 {
            0 // top-level values are never indented
        } else if len == 0 && will_end {
            0 // an empty object or array is never indented
        } else if len == 0 || need_implicit_comma {
            depth
        } else if will_end {
            depth - 1
        } else {
            0
        }
    }

    // Go: jsontext/encode.go:346 WriteToken with MayAppendDelim
    // (state.go:389), appendWhitespace (encode.go:634) and AppendIndent
    // (encode.go:652).
    fn write_token(&mut self, out: &mut String, tok: &JsonToken) -> Result<(), JsonError> {
        let k = tok.kind();
        let will_end = k == b'}' || k == b']';
        let (is_object, len) = self.last();
        let need_colon = is_object && len % 2 == 1;
        let need_comma = !need_colon && len > 0 && !will_end && !self.stack.is_empty();
        if need_colon {
            out.push(':');
        } else if need_comma {
            out.push(',');
        }
        if let Some((prefix, indent)) = self.multiline {
            if need_colon {
                // SpaceAfterColon is implied by Multiline.
                out.push(' ');
            } else {
                let n = self.need_indent(k);
                if n > 0 {
                    out.push('\n');
                    out.push_str(prefix);
                    for _ in 1..n {
                        out.push_str(indent);
                    }
                }
            }
        }
        match tok {
            JsonToken::Null => out.push_str("null"),
            JsonToken::False => out.push_str("false"),
            JsonToken::True => out.push_str("true"),
            JsonToken::String(s) => s.marshal_json_to(out)?,
            JsonToken::Number(raw) => out.push_str(raw),
            JsonToken::BeginObject => out.push('{'),
            JsonToken::EndObject => out.push('}'),
            JsonToken::BeginArray => out.push('['),
            JsonToken::EndArray => out.push(']'),
        }
        match tok {
            JsonToken::BeginObject | JsonToken::BeginArray => {
                self.increment();
                self.stack.push((k == b'{', 0));
            }
            JsonToken::EndObject | JsonToken::EndArray => {
                self.stack.pop();
            }
            _ => self.increment(),
        }
        Ok(())
    }

    // Reads one complete value from `dec` and writes it.
    fn write_value_from(
        &mut self,
        out: &mut String,
        dec: &mut JsonDecoder<'_>,
    ) -> Result<(), JsonError> {
        let depth = self.stack.len();
        loop {
            let tok = dec.read_token()?;
            self.write_token(out, &tok)?;
            if self.stack.len() == depth {
                return Ok(());
            }
        }
    }
}

// Go: jsontext/encode.go:526 WriteValue (compact output)
// Writes one raw JSON value: whitespace is removed, names must be unique,
// strings are re-quoted in canonical form (invalid UTF-8 becomes U+FFFD, as
// `internal/json.Marshal` sets AllowInvalidUTF8), numbers are copied
// verbatim, and only whitespace may follow the value.
pub fn write_value(enc: &mut String, raw: &[u8]) -> Result<(), JsonError> {
    let mut dec = JsonDecoder::new(
        raw,
        JsonOptions {
            allow_duplicate_names: false,
            allow_invalid_utf8: true,
            port_form: false,
        },
    );
    let mut w = TokenWriter {
        stack: Vec::new(),
        top_len: 0,
        multiline: None,
    };
    w.write_value_from(enc, &mut dec)?;
    dec.check_eof()
}

// Go: jsonwire/wire.go:62 QuoteRune
// QuoteRune quotes the first rune in the input.
pub fn quote_rune(b: &[u8]) -> String {
    // Go utf8.DecodeRune: (RuneError, 0) for empty input, (RuneError, 1)
    // for an invalid or truncated sequence.
    let Some(&first) = b.first() else {
        return crate::gostd::strconv::quote_rune('\u{FFFD}');
    };
    let n = match first {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    };
    let r = b
        .get(..n)
        .and_then(|s| std::str::from_utf8(s).ok())
        .and_then(|s| s.chars().next());
    match r {
        Some(r) => crate::gostd::strconv::quote_rune(r),
        None => format!("'\\x{first:x}'"),
    }
}

// ---------------------------------------------------------------------------
// Struct helpers (for hand-written and generated arshalers)
// ---------------------------------------------------------------------------

/// Go v2 struct marshal: `{`.
pub fn write_object_start(enc: &mut String) {
    enc.push('{');
}

/// Go v2 struct marshal: `}`.
pub fn write_object_end(enc: &mut String) {
    enc.push('}');
}

/// Writes one struct field (`"name":value`), with a comma before every
/// field but the first. The body is `marshal_field_dyn`: the marshaler of
/// `T` is one function that every struct shares, not a copy inlined into
/// each struct.
#[inline(always)]
pub fn marshal_field<T: MarshalerTo>(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: &T,
) -> Result<(), JsonError> {
    marshal_field_dyn(enc, first, name, value)
}

/// `marshal_field` of a value whose type is erased: one copy for all types.
#[inline(never)]
pub fn marshal_field_dyn(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: &dyn MarshalerTo,
) -> Result<(), JsonError> {
    if !*first {
        enc.push(',');
    }
    *first = false;
    name.marshal_json_to(enc)?;
    enc.push(':');
    value.marshal_json_to(enc)
}

/// A field with Go `omitzero`: skipped when the value is zero.
pub fn marshal_field_omitzero<T: MarshalerTo + IsZero>(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: &T,
) -> Result<(), JsonError> {
    if value.is_zero() {
        return Ok(());
    }
    marshal_field_dyn(enc, first, name, value)
}

/// An optional field (Go `*T` with `omitzero`): skipped when `None`.
pub fn marshal_opt_field<T: MarshalerTo>(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: &Option<T>,
) -> Result<(), JsonError> {
    match value {
        None => Ok(()),
        Some(v) => marshal_field_dyn(enc, first, name, v),
    }
}

/// Go v2 default struct unmarshal (arshal_default.go:1283
/// makeStructArshaler) without reflection. `null` returns `Ok(false)` and
/// the caller sets the zero value. An object calls `field` for each member
/// name in input order; `field` decodes the value and returns `true`, or
/// returns `false` for an unknown name, which is then skipped. Names match
/// exactly. Any other kind is an error after its first token.
/// The body is `unmarshal_struct_fields_dyn`, so each struct adds only its
/// `field` function.
#[inline(always)]
pub fn unmarshal_struct_fields(
    dec: &mut JsonDecoder<'_>,
    go_type: &str,
    mut field: impl FnMut(&str, &mut JsonDecoder<'_>) -> Result<bool, JsonError>,
) -> Result<bool, JsonError> {
    unmarshal_struct_fields_dyn(dec, go_type, &mut field)
}

/// `unmarshal_struct_fields` with a `field` function whose type is erased:
/// one copy for all structs.
#[inline(never)]
pub fn unmarshal_struct_fields_dyn(
    dec: &mut JsonDecoder<'_>,
    go_type: &str,
    field: &mut dyn FnMut(&str, &mut JsonDecoder<'_>) -> Result<bool, JsonError>,
) -> Result<bool, JsonError> {
    let tok = dec.read_token()?;
    match tok {
        JsonToken::Null => Ok(false),
        JsonToken::BeginObject => {
            while dec.peek_kind() != b'}' {
                let JsonToken::String(name) = dec.read_token()? else {
                    unreachable!("object member names are strings")
                };
                if !field(&name, dec)? {
                    dec.skip_value()?;
                }
            }
            dec.read_token()?;
            Ok(true)
        }
        // Go `newUnmarshalErrorAfterWithSkipping` skips the rest of the
        // value only with legacy semantics.
        _ => Err(unmarshal_kind_error(tok.kind(), go_type)),
    }
}

// Go: internal/json/json.go:40 MarshalIndent with
// jsontext.WithIndentPrefix(prefix) and jsontext.WithIndent(indent):
// multiline output, a space after each colon, no space after commas, no
// newline inside an empty object or array, no trailing newline.
#[inline(always)]
pub fn marshal_indent<T: MarshalerTo>(
    value: &T,
    prefix: &str,
    indent: &str,
) -> Result<String, JsonError> {
    marshal_indent_dyn(value, prefix, indent)
}

/// `marshal_indent` of a value whose type is erased.
#[inline(never)]
pub fn marshal_indent_dyn(
    value: &dyn MarshalerTo,
    prefix: &str,
    indent: &str,
) -> Result<String, JsonError> {
    if prefix.is_empty() && indent.is_empty() {
        // WithIndentPrefix and WithIndent imply multiline output, so skip them.
        return json_marshal_dyn(value, &[]);
    }
    // Go: jsontext/options.go:265 WithIndentPrefix, :232 WithIndent.
    let s = prefix.trim_matches(|c| c == ' ' || c == '\t');
    if !s.is_empty() {
        panic!(
            "json: invalid character {} in indent prefix",
            quote_rune(s.as_bytes())
        );
    }
    let s = indent.trim_matches(|c| c == ' ' || c == '\t');
    if !s.is_empty() {
        panic!(
            "json: invalid character {} in indent",
            quote_rune(s.as_bytes())
        );
    }
    let compact = json_marshal_dyn(value, &[])?;
    let mut dec = JsonDecoder::new(
        compact.as_bytes(),
        JsonOptions {
            allow_duplicate_names: false,
            allow_invalid_utf8: true,
            port_form: false,
        },
    );
    let mut w = TokenWriter {
        stack: Vec::new(),
        top_len: 0,
        multiline: Some((prefix, indent)),
    };
    let mut out = String::with_capacity(compact.len() * 2);
    w.write_value_from(&mut out, &mut dec)?;
    dec.check_eof()?;
    Ok(out)
}

#[cfg(test)]
mod unmarshal_error_tests {
    use super::*;

    fn err_text<T: UnmarshalerFrom + Default>(data: &str) -> String {
        let mut v = T::default();
        unmarshal_root(data.as_bytes(), &mut v)
            .expect_err("want an error")
            .to_string()
    }

    // Expected texts come from Go JSON v2 `json.Unmarshal`
    // (go-json-experiment v0.0.0-20260601182631-00ed12fed2a6). Go writes
    // "cannot" or "unable to"; the port writes "cannot".
    #[test]
    fn unmarshal_root_errors_match_go() {
        assert_eq!(
            err_text::<Vec<u32>>(r#"[1,"x"]"#),
            r#"json: cannot unmarshal JSON string into Go uint32 within "/1""#
        );
        assert_eq!(
            err_text::<Vec<u32>>(" {}"),
            "json: cannot unmarshal JSON object into Go []uint32 after offset 1"
        );
        assert_eq!(
            err_text::<[u32; 2]>("[1]"),
            "json: cannot unmarshal JSON array into Go [2]uint32 after offset 2: too few array elements"
        );
        assert_eq!(
            err_text::<LspAny>(r#"{"a":[1,1e999]}"#),
            r#"json: cannot unmarshal JSON number 1e999 into Go float64 within "/a/1": value out of range"#
        );
        assert_eq!(
            err_text::<FxHashMap<String, Vec<u32>>>(r#"{"a/b":[0,-1]}"#),
            r#"json: cannot unmarshal JSON number -1 into Go uint32 within "/a~1b/1": invalid syntax"#
        );
    }

    // The pointer holds Go's bytes of the names (a real U+FDD0 is one
    // char), and `%q` quotes them. Text from Go N (encoding/json/v2 of Go
    // 1.27.1, followups25 round b tools-b/gojsonptr).
    #[test]
    fn pointer_names_are_quoted_from_go_bytes() {
        assert_eq!(
            err_text::<FxHashMap<String, Vec<u32>>>("{\"\u{fdd0}\u{fdd0}\":[0,-1]}"),
            r#"json: cannot unmarshal JSON number -1 into Go uint32 within "/\ufdd0\ufdd0/1": invalid syntax"#
        );
    }
}
