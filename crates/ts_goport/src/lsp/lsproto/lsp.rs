//! Port of internal/lsp/lsproto/lsp.go.
//!
//! PORT: `DocumentUri`, `URI` and `Method` are Go string types; their JSON
//! impls are the v2 string arshaler. `IndexMap<DocumentUri, V>` (Go
//! `map[DocumentUri]V`) gets the v2 map arshaler here, through the
//! goport_util trait `JsonMapKey`.
//!
//! The `err*` helpers make the plain errors that generated `UnmarshalJSONFrom`
//! methods return. Each one records where the method left the decoder, so
//! that `unmarshal_root` can give the error its Go v2 JSON pointer.

use crate::lsp::lsproto::prelude::*;

use crate::frontend::bundled::is_bundled;
use crate::frontend::json_indexmap::JsonMapKey;
use std::marker::PhantomData;

// Go: lsp.go:17 DocumentUri
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentUri(pub String); // !!!

impl DocumentUri {
    // Go: lsp.go:19 Path (ts#64159: N's FileName; FileName at :77 is the
    // same text with file intent)
    // ts#64159: a bundled or file URI gives a rooted, normalized path
    // (`RootedPathFromAbsolute`; a relative path panics), so "/a/../b.ts"
    // is "/b.ts" and a trailing separator goes.
    // PORT: Go also validates the dynamic name with
    // `RootedPathFromNormalized`; the encoding above never gives one that
    // fails it.
    pub fn file_name(&self) -> String {
        let uri = self.0.as_str();
        if is_bundled(uri) {
            return rooted_path_from_absolute(uri);
        }
        if uri.starts_with("file://") {
            let parsed = match gostd::url::parse(uri) {
                Ok(parsed) => parsed,
                Err(_) => crate::core::go_panic(format!("invalid file URI: {uri}")),
            };
            if !parsed.host.is_empty() {
                return rooted_path_from_absolute(&format!("//{}{}", parsed.host, parsed.path));
            }
            return rooted_path_from_absolute(&fix_windows_uri_path(&parsed.path));
        }

        // Leave all other URIs escaped so we can round-trip them.

        let Some((scheme, path)) = uri.split_once(':') else {
            crate::core::go_panic(format!("invalid URI: {uri}"));
        };

        // ts#64544: a query or fragment is encoded with the last segment.
        let (mut path, suffix) = match path.find(['?', '#']) {
            Some(suffix_start) => (&path[..suffix_start], &path[suffix_start..]),
            None => (path, ""),
        };

        let mut authority = "ts-nul-authority";
        let mut has_authority = false;
        let mut has_path = true;
        if let Some(rest) = path.strip_prefix("//") {
            has_authority = true;
            match rest.split_once('/') {
                Some((a, p)) => {
                    authority = a;
                    path = p;
                }
                None => {
                    authority = rest;
                    path = "";
                    has_path = false;
                }
            }
        }

        let encoded_authority = if !has_authority {
            authority.to_string()
        } else if authority == "ts-nul-authority" {
            tspath::force_encode_dynamic_uri_path_segment(authority, false)
        } else {
            tspath::encode_dynamic_uri_path(authority)
        };
        let encoded_path = if has_path {
            tspath::encode_dynamic_uri_path_with_suffix(path, suffix)
        } else {
            tspath::encode_dynamic_uri_no_path(suffix)
        };

        format!(
            "{}{scheme}/{encoded_authority}/{encoded_path}",
            tspath::DYNAMIC_URI_FILE_NAME_PREFIX
        )
    }

    // Go: lsp.go:52 Path (at 673a5f17d713; ts#64159 renames it PathKey,
    // lsp.go:81)
    // ts#64544: an encoded dynamic file name keeps its case, and its bare
    // root ends with "/" (Go `canonicalDynamicFileName`, removed by ts#64159;
    // `tspath::to_path` gives the same key).
    pub fn path(&self, use_case_sensitive_file_names: bool) -> tspath::Path {
        let file_name = self.file_name();
        tspath::to_path(&file_name, "", use_case_sensitive_file_names)
    }
}

// Go: lsp.go:85 DynamicFileNameToDocumentUri (ts#64544)
#[must_use]
pub fn dynamic_file_name_to_document_uri(file_name: &str) -> DocumentUri {
    match dynamic_file_name_to_document_uri_worker(file_name, false) {
        Some(uri) => uri,
        None => crate::core::go_panic(format!("invalid file name: {file_name}")),
    }
}

// Go: lsp.go:93 TryDynamicFileNameToDocumentUri (ts#64544)
// `None` is Go's `ok == false`: the name is not a valid dynamic file name.
#[must_use]
pub fn try_dynamic_file_name_to_document_uri(file_name: &str) -> Option<DocumentUri> {
    dynamic_file_name_to_document_uri_worker(file_name, true)
}

// Go: lsp.go:97 dynamicFileNameToDocumentUri (ts#64544)
// It decodes the encoded names (`^/~ts-uri~/...`) and takes the literal
// names (`^/<scheme>/...`) as they are.
fn dynamic_file_name_to_document_uri_worker(file_name: &str, strict: bool) -> Option<DocumentUri> {
    let encoded = tspath::is_encoded_dynamic_file_name(file_name);
    let start = if encoded {
        tspath::DYNAMIC_URI_FILE_NAME_PREFIX.len()
    } else {
        2
    };
    let (scheme, rest) = file_name[start..].split_once('/')?;
    if strict && scheme.is_empty() {
        return None;
    }
    let (authority, uri_path) = rest.split_once('/')?;
    let has_authority = authority != "ts-nul-authority";
    let authority: Cow<'_, str> = if !encoded {
        Cow::Borrowed(authority)
    } else if strict {
        Cow::Owned(tspath::try_decode_dynamic_uri_path_segment(authority)?)
    } else {
        Cow::Owned(tspath::decode_dynamic_uri_path_segment(authority))
    };
    if encoded
        && has_authority
        && let Some(suffix) = tspath::decode_dynamic_uri_no_path(uri_path)
    {
        return Some(DocumentUri(format!("{scheme}://{authority}{suffix}")));
    }
    let uri_path: Cow<'_, str> = if !encoded {
        Cow::Borrowed(uri_path)
    } else if strict {
        Cow::Owned(tspath::try_decode_dynamic_uri_path(uri_path)?)
    } else {
        Cow::Owned(tspath::decode_dynamic_uri_path(uri_path))
    };
    if !has_authority {
        return Some(DocumentUri(format!("{scheme}:{uri_path}")));
    }
    Some(DocumentUri(format!("{scheme}://{authority}/{uri_path}")))
}

// Go: tspath/rooted_path.go:48 RootedPathFromAbsolute (ts#64159)
// The rooted, normalized form of an absolute path; a relative path, or a
// URL path with a query or fragment, is a Go panic.
// PORT: the port keeps string paths (bump D plan section 3, behavior only),
// and the Rust tspath (program lane) has no typed path helpers, so the
// server files share this copy.
pub fn rooted_path_from_absolute(path: &str) -> String {
    match try_rooted_path_from_absolute(path) {
        Some(path) => path,
        None => crate::core::go_panic("path must be absolute".to_string()),
    }
}

// Go: tspath/rooted_path.go:58 TryRootedPathFromAbsolute (ts#64159)
pub fn try_rooted_path_from_absolute(path: &str) -> Option<String> {
    if has_rooted_url_suffix(path) || !tspath::path_is_absolute(path) {
        return None;
    }
    let mut normalized = tspath::get_normalized_absolute_path(path, "");
    // Go: tspath/rooted_path.go:65 ensureRootedPathRootSeparator
    if tspath::get_root_length(&normalized) == normalized.len()
        && !tspath::has_trailing_directory_separator(&normalized)
    {
        normalized.push('/');
    }
    Some(normalized)
}

// Go: tspath/rooted_path.go:106 hasRootedURLSuffix (ts#64159)
fn has_rooted_url_suffix(path: &str) -> bool {
    // Go: tspath/rooted_path.go:114 hasURLRoot
    if !(tspath::get_encoded_root_length(path) < 0 && path.contains("://")) {
        return false;
    }
    let after_scheme = path.split_once("://").map_or("", |(_, rest)| rest);
    after_scheme.contains(['?', '#'])
}

// Go: lsp.go:144 fixWindowsURIPath
pub fn fix_windows_uri_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('/') {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && tspath::is_volume_character(bytes[0]) && bytes[1] == b':' {
            return rest.to_string();
        }
    }
    path.to_string()
}

// Go: lsp.go:66 HasTextDocumentURI
pub trait HasTextDocumentURI {
    fn text_document_uri(&self) -> DocumentUri;
}

// Go: lsp.go:70 HasTextDocumentPosition
pub trait HasTextDocumentPosition: HasTextDocumentURI {
    fn text_document_position(&self) -> Position;
}

// Go: lsp.go:75 HasLocations
pub trait HasLocations {
    fn get_locations(&self) -> Option<&Vec<Location>>;
}

// Go: lsp.go:79 HasLocation
pub trait HasLocation {
    fn get_location(&self) -> Location;
}

// Go: lsp.go:83 URI
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct URI(pub String); // !!!

// Go: lsp.go:85 Method
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Method(pub Cow<'static, str>);

// Go `%s` / `%v` of a string type prints the string.
impl std::fmt::Display for DocumentUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for URI {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Go v2 string arshaler for the three string types.
impl MarshalerTo for DocumentUri {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for DocumentUri {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_string_as(dec, &mut self.0, &go_type_name::<Self>())
    }
}

impl IsZero for DocumentUri {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

impl MarshalerTo for URI {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for URI {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_string_as(dec, &mut self.0, &go_type_name::<Self>())
    }
}

impl IsZero for URI {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

impl MarshalerTo for Method {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.as_ref().marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for Method {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut v = self.0.to_string();
        unmarshal_string_as(dec, &mut v, &go_type_name::<Self>())?;
        self.0 = Cow::Owned(v);
        Ok(())
    }
}

impl IsZero for Method {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

// PORT: the JSON impls of `IndexMap<DocumentUri, V>` (Go `map[DocumentUri]V`).
// The orphan rule keeps `MarshalerTo` and `UnmarshalerFrom` for `IndexMap`
// in goport_util, which calls these through `JsonMapKey`.
impl JsonMapKey for DocumentUri {
    // Go v2 map arshaler (arshal_default.go:794) for `map[DocumentUri]V`.
    // PORT: members write in insertion order; Go map order is random.
    fn marshal_map<V: MarshalerTo>(
        map: &IndexMap<Self, V>,
        enc: &mut String,
    ) -> Result<(), JsonError> {
        enc.push('{');
        for (i, (k, v)) in map.iter().enumerate() {
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

    // Go v2 map arshaler (arshal_default.go:955) for `map[DocumentUri]V`: null
    // sets nil; an object merges into the existing map (a value for a key that
    // is already present decodes into a copy of the old value); a name repeated
    // in this object is an error unless duplicates are allowed. A plain error of
    // the method of `V` gets the Go type `V`.
    // PORT: Go sets a nil map for null; the Rust zero value is an empty map.
    fn unmarshal_map<V: UnmarshalerFrom + Default + Clone>(
        map: &mut IndexMap<Self, V>,
        dec: &mut JsonDecoder<'_>,
    ) -> Result<(), JsonError> {
        let tok = dec.read_token()?;
        match tok {
            JsonToken::Null => {
                map.clear();
                Ok(())
            }
            JsonToken::BeginObject => {
                // String keys have a unique representation unless invalid
                // UTF-8 is allowed, so the map does its own duplicate check.
                if !dec.options.allow_invalid_utf8 {
                    dec.disable_namespace();
                }
                let allow_dup = dec.options.allow_duplicate_names;
                let mut seen: Option<FxHashMap<DocumentUri, ()>> = if !allow_dup && !map.is_empty()
                {
                    Some(FxHashMap::default())
                } else {
                    None
                };
                while dec.peek_kind() != b'}' {
                    let mut k = DocumentUri::default();
                    json_unmarshal_decode(dec, &mut k)?;
                    let mut v = V::default();
                    if let Some(existing) = map.get(&k) {
                        if !allow_dup && seen.as_ref().is_none_or(|s| s.contains_key(&k)) {
                            return Err(dec.duplicate_name_error());
                        }
                        v = existing.clone();
                    }
                    let err = json_unmarshal_decode(dec, &mut v);
                    if let Some(s) = &mut seen {
                        s.insert(k.clone(), ());
                    }
                    map.insert(k, v);
                    err?;
                }
                dec.read_token()?;
                Ok(())
            }
            // Go `newUnmarshalErrorAfterWithSkipping` skips the rest of the
            // value only with legacy semantics.
            _ => Err(unmarshal_kind_error(
                tok.kind(),
                &go_type_name::<IndexMap<Self, V>>(),
            )),
        }
    }
}

// Go: boolToInt (removed in tsgo#4471)
// PORT: Go tsgo#4471 replaced the generated union and `Registration`
// marshal checks with the reflective `marshalUnion` and `countNonNil`
// (structcodec.go:129, :155). Rust has no reflection, so the generated code
// keeps its per-field checks and this helper; the behavior is the same.
pub fn bool_to_int(b: bool) -> i32 {
    if b {
        return 1;
    }
    0
}

// Go `%v` of a `jsontext.Kind` (jsontext/token.go:643 Kind.String).
fn json_kind_string(k: u8) -> String {
    match k {
        0 => "invalid".to_string(),
        b'n' => "null".to_string(),
        b'f' => "false".to_string(),
        b't' => "true".to_string(),
        b'"' => "string".to_string(),
        b'0' => "number".to_string(),
        b'{' => "{".to_string(),
        b'}' => "}".to_string(),
        b'[' => "[".to_string(),
        b']' => "]".to_string(),
        _ => format!("<invalid jsontext.Kind: {}>", quote_rune(&[k])),
    }
}

// Go: lsp.go:87 errNotObject
// Generated methods return it before they read the value.
pub fn err_not_object(k: u8) -> JsonError {
    SemanticError::method(
        ErrorPos::Before,
        format!(
            "expected object start, but encountered {}",
            json_kind_string(k)
        ),
    )
}

// Go: lsp.go:91 errNull
// Generated methods return it after the member name.
pub fn err_null(field: &str) -> JsonError {
    SemanticError::method(
        ErrorPos::After,
        format!(
            "null value is not allowed for field {}",
            gostd::strconv::quote(field)
        ),
    )
}

// Go: lsp.go:95 errMissing
// Generated methods return it after the closing `}`.
pub fn err_missing<I, S>(props: I) -> JsonError
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let props: Vec<String> = props.into_iter().map(|p| p.as_ref().to_string()).collect();
    SemanticError::method(
        ErrorPos::AfterEnd,
        format!("missing required properties: {}", props.join(", ")),
    )
}

// Go: lsp.go:99 errInvalidKind
// Generated methods return it after a peek, before they read the value.
pub fn err_invalid_kind(type_name: &str, got: u8) -> JsonError {
    SemanticError::method(
        ErrorPos::Before,
        format!("invalid {}: got {}", type_name, json_kind_string(got)),
    )
}

// Go: lsp.go:103 errInvalidValue
// Generated methods return it after they read the value.
pub fn err_invalid_value(type_name: &str, data: impl AsRef<[u8]>) -> JsonError {
    SemanticError::method(
        ErrorPos::After,
        format!(
            "invalid {}: {}",
            type_name,
            String::from_utf8_lossy(data.as_ref())
        ),
    )
}

// Go: lsp.go:107 errLiteralMismatch
// Generated methods return it after they read the value.
pub fn err_literal_mismatch(type_name: &str, expected: &str, got: impl AsRef<[u8]>) -> JsonError {
    SemanticError::method(
        ErrorPos::After,
        format!(
            "expected {} value {}, got {}",
            type_name,
            expected,
            String::from_utf8_lossy(got.as_ref())
        ),
    )
}

// Go: lsp.go:111 assertOnlyOne
pub fn assert_only_one(message: &str, count: i32) {
    if count != 1 {
        crate::core::go_panic(message.to_string());
    }
}

// Go: lsp.go:117 assertAtMostOne
pub fn assert_at_most_one(message: &str, count: i32) {
    if count > 1 {
        crate::core::go_panic(message.to_string());
    }
}

// Go: lsp.go:124 jsonKeyCheck
// jsonKeyCheck compares a raw JSON key token (including quotes) against a Go string.
pub fn json_key_check(name: &[u8], key: &str) -> bool {
    name.len() == key.len() + 2 && name[0] == b'"' && &name[1..name.len() - 1] == key.as_bytes()
}

// Go: lsp.go:131 jsonObjectRawField
// jsonObjectRawField scans the top-level keys of a JSON object looking for the
// given field name, and returns its raw JSON value (e.g. `"full"` with quotes).
// Returns nil if the field is not found.
pub fn json_object_raw_field(data: &[u8], field: &str) -> JsonValue {
    let mut dec = json_new_decoder(data);
    if dec.peek_kind() != b'{' {
        return JsonValue::default();
    }
    if dec.read_token().is_err() {
        return JsonValue::default();
    }
    while dec.peek_kind() != b'}' {
        let Ok(name) = dec.read_value() else {
            return JsonValue::default();
        };
        if json_key_check(name, field) {
            let Ok(val) = dec.read_value() else {
                return JsonValue::default();
            };
            return JsonValue(val.to_vec());
        }
        if dec.skip_value().is_err() {
            return JsonValue::default();
        }
    }
    JsonValue::default()
}

// Go: structcodec.go:172 discriminatedStructDecoder
/// The state of [`scan_discriminated_struct`]: the discriminator value and
/// the object members that come before it.
///
/// PORT: Go keeps each earlier member as its name and raw value
/// (`deferredStructField`); here each is the raw name (with quotes) and the
/// raw value.
#[derive(Debug, Default)]
pub struct DiscriminatedStructDecoder {
    type_name: &'static str,
    discriminator: &'static str,
    pub discriminator_value: Vec<u8>,
    has_discriminator: bool,
    deferred: Vec<(Vec<u8>, Vec<u8>)>,
    closed: bool,
}

// Go: structcodec.go:184 scanDiscriminatedStruct
// scanDiscriminatedStruct advances through an object until it finds the
// discriminator. Only fields preceding the discriminator are retained.
pub fn scan_discriminated_struct(
    dec: &mut JsonDecoder<'_>,
    type_name: &'static str,
    discriminator: &'static str,
) -> Result<DiscriminatedStructDecoder, JsonError> {
    let k = dec.peek_kind();
    if k != b'{' {
        return Err(err_not_object(k));
    }
    dec.read_token()?;

    let mut state = DiscriminatedStructDecoder {
        type_name,
        discriminator,
        ..Default::default()
    };
    while dec.peek_kind() != b'}' {
        let raw_name = dec.read_value()?.to_vec();
        if json_key_check(&raw_name, discriminator) {
            state.discriminator_value = dec.read_value()?.to_vec();
            state.has_discriminator = true;
            return Ok(state);
        }

        let value = dec.read_value()?.to_vec();
        state.deferred.push((raw_name, value));
    }
    dec.read_token()?;
    state.closed = true;
    Ok(state)
}

impl DiscriminatedStructDecoder {
    // Go: structcodec.go:226 (discriminatedStructDecoder).invalidDiscriminator
    // PORT: with no discriminator the scan read the closing `}`; else it
    // stopped just after the discriminator value.
    #[must_use]
    pub fn invalid_discriminator(&self) -> JsonError {
        if !self.has_discriminator {
            return SemanticError::method(
                ErrorPos::AfterEnd,
                format!(
                    "invalid {}: missing discriminator {}",
                    self.type_name,
                    gostd::strconv::quote(self.discriminator)
                ),
            );
        }
        SemanticError::method(
            ErrorPos::After,
            format!(
                "invalid {} discriminator {}: {}",
                self.type_name,
                gostd::strconv::quote(self.discriminator),
                String::from_utf8_lossy(&self.discriminator_value)
            ),
        )
    }
}

// Go: structcodec.go:235 unmarshalDiscriminatedArm
// unmarshalDiscriminatedArm decodes the retained and remaining object fields
// into a concrete union arm, assigning it only after the full object succeeds.
// PORT: Go feeds the discriminator, the retained fields and then the rest of
// the object to one reflective struct decoder. Rust has no reflection: the
// members are joined, in that order, into one object that the arm's
// generated decoder reads. The caller assigns the arm.
pub fn unmarshal_discriminated_arm<T: UnmarshalerFrom + Default>(
    state: &DiscriminatedStructDecoder,
    dec: &mut JsonDecoder<'_>,
) -> Result<T, JsonError> {
    let mut object = vec![b'{'];
    let mut push = |name: &[u8], value: &[u8]| {
        if object.len() > 1 {
            object.push(b',');
        }
        object.extend_from_slice(name);
        object.push(b':');
        object.extend_from_slice(value);
    };
    if state.has_discriminator {
        push(
            format!("\"{}\"", state.discriminator).as_bytes(),
            &state.discriminator_value,
        );
    }
    for (name, value) in &state.deferred {
        push(name, value);
    }
    if !state.closed {
        while dec.peek_kind() != b'}' {
            let name = dec.read_value()?.to_vec();
            let value = dec.read_value()?;
            push(&name, value);
        }
        dec.read_token()?;
    }
    object.push(b'}');
    let mut target = T::default();
    json_unmarshal(&object, &mut target, &[])?;
    Ok(target)
}

// Go: lsp.go:161 jsonObjectHasKey
// jsonObjectHasKey scans the top-level keys of a JSON object looking for any of the
// given keys. Returns the index of the first key found, or -1 if none match.
// Bails early on first match without decoding any values.
pub fn json_object_has_key(data: &[u8], keys: &[&str]) -> i32 {
    let mut dec = json_new_decoder(data);
    if dec.peek_kind() != b'{' {
        return -1;
    }
    if dec.read_token().is_err() {
        return -1;
    }
    while dec.peek_kind() != b'}' {
        let Ok(name) = dec.read_value() else {
            return -1;
        };
        for (i, key) in keys.iter().enumerate() {
            if json_key_check(name, key) {
                return i as i32;
            }
        }
        if dec.skip_value().is_err() {
            return -1;
        }
    }
    -1
}

// Inspired by https://www.youtube.com/watch?v=dab3I-HcTVk

// Go: lsp.go:188 RequestInfo
// PORT: Go `_ [0]Params` / `_ [0]Resp` are `PhantomData`. Go builds the
// value with a struct literal; `new` is the const constructor the
// generated `*_INFO` consts use.
pub struct RequestInfo<P, R> {
    _params: PhantomData<fn() -> P>,
    _resp: PhantomData<fn() -> R>,
    pub method: Method,
}

impl<P, R> RequestInfo<P, R> {
    pub const fn new(method: Method) -> RequestInfo<P, R> {
        RequestInfo {
            _params: PhantomData,
            _resp: PhantomData,
            method,
        }
    }
}

impl<P, R: UnmarshalerFrom + Default> RequestInfo<P, R> {
    // Go: lsp.go:194 UnmarshalResult
    // PORT: Go type assertion on `any`. `None` is a nil `any`. `%T` in the
    // error prints the Rust debug value.
    pub fn unmarshal_result(&self, result: Option<Box<dyn AnyValue>>) -> Result<R, GoError> {
        let Some(result) = result else {
            return Err(gostd::errors::new("expected json.Value, got <nil>"));
        };
        let Some(raw) = result.downcast_ref::<JsonValue>() else {
            return Err(gostd::errors::new(format!(
                "expected json.Value, got {result:?}"
            )));
        };

        let mut r = R::default();
        if let Err(err) = unmarshal_root(&raw.0, &mut r) {
            return Err(gostd::errors::from_value(err));
        }
        Ok(r)
    }
}

impl<P: AnyValue, R> RequestInfo<P, R> {
    // Go: lsp.go:207 NewRequestMessage
    pub fn new_request_message(&self, id: Option<crate::jsonrpc::ID>, params: P) -> RequestMessage {
        RequestMessage {
            id,
            method: self.method.clone(),
            params: Some(Box::new(params)),
            ..RequestMessage::default()
        }
    }
}

impl<P, R> Clone for RequestInfo<P, R> {
    fn clone(&self) -> Self {
        RequestInfo::new(self.method.clone())
    }
}

impl<P, R> std::fmt::Debug for RequestInfo<P, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestInfo")
            .field("method", &self.method)
            .finish()
    }
}

// Go: lsp.go:215 NotificationInfo
pub struct NotificationInfo<P> {
    _params: PhantomData<fn() -> P>,
    pub method: Method,
}

impl<P> NotificationInfo<P> {
    pub const fn new(method: Method) -> NotificationInfo<P> {
        NotificationInfo {
            _params: PhantomData,
            method,
        }
    }
}

impl<P: AnyValue> NotificationInfo<P> {
    // Go: lsp.go:220 NewNotificationMessage
    pub fn new_notification_message(&self, params: P) -> RequestMessage {
        RequestMessage {
            method: self.method.clone(),
            params: Some(Box::new(params)),
            ..RequestMessage::default()
        }
    }
}

impl<P> Clone for NotificationInfo<P> {
    fn clone(&self) -> Self {
        NotificationInfo::new(self.method.clone())
    }
}

impl<P> std::fmt::Debug for NotificationInfo<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationInfo")
            .field("method", &self.method)
            .finish()
    }
}

// Go: lsp.go:266 Null
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Null;

// Go: lsp.go:271 (Null) UnmarshalJSONFrom
impl UnmarshalerFrom for Null {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let data = dec.read_value()?;
        if data != b"null" {
            return Err(SemanticError::method(
                ErrorPos::After,
                format!("expected null, got {}", String::from_utf8_lossy(data)),
            ));
        }
        Ok(())
    }
}

// Go: lsp.go:282 (Null) MarshalJSONTo
impl MarshalerTo for Null {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str("null");
        Ok(())
    }
}

// Go reflect: an empty struct is always zero.
impl IsZero for Null {
    fn is_zero(&self) -> bool {
        true
    }
}

// Go: lsp.go:235 UnmarshalParams
// UnmarshalParams decodes the params of an inbound request or notification
// message into the requested type. Inbound messages store their params as a
// raw [json.Value] (see [Message.UnmarshalJSON]); decoding is deferred to the
// point of dispatch so that param types for methods the server never handles
// are not forced into the binary.
//
// A [NoParams] method must be given no params; every other method must be given
// params as an object or array. A violation returns [ErrorCodeInvalidParams].
// PORT: Go `T` is a pointer type (or `NoParams`); a successful decode never
// gives a nil pointer, so the port returns the value. `%T` in the error prints
// the Rust debug value.
pub fn unmarshal_params<T: UnmarshalerFrom + Default + 'static>(
    req: &RequestMessage,
) -> Result<T, GoError> {
    let mut params = T::default();
    let mut raw: &[u8] = &[];
    if let Some(p) = req.params.as_deref() {
        let Some(v) = p.downcast_ref::<JsonValue>() else {
            return Err(super::jsonrpc::wrap_error_code(
                ErrorCode::INVALID_PARAMS,
                gostd::errors::new(format!("unexpected params type {p:?}")),
            ));
        };
        raw = &v.0;
    }

    // params is the zero value of T; this asserts on its type, i.e. whether the
    // method was declared with NoParams.
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<NoParams>() {
        if !raw.is_empty() {
            return Err(super::jsonrpc::wrap_error_code(
                ErrorCode::INVALID_PARAMS,
                gostd::errors::new(format!(
                    "expected no params, got {}",
                    String::from_utf8_lossy(raw)
                )),
            ));
        }
        return Ok(params);
    }

    // The base protocol defines params as `array | object`; reject anything else
    // (absent, null, or a scalar).
    // PORT: Go `raw.Kind()` is the first byte after JSON whitespace.
    let kind = raw
        .iter()
        .copied()
        .find(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r'));
    if kind != Some(b'{') && kind != Some(b'[') {
        return Err(super::jsonrpc::wrap_error_code(
            ErrorCode::INVALID_PARAMS,
            gostd::errors::new("params must be an object or array"),
        ));
    }
    if let Err(err) = unmarshal_root(raw, &mut params) {
        return Err(super::jsonrpc::wrap_error_code(
            ErrorCode::INVALID_PARAMS,
            gostd::errors::from_value(err),
        ));
    }
    Ok(params)
}

// Go: lsp.go:283 NoParams
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoParams;

// Go: lsp.go:288 (NoParams) IsZero
impl IsZero for NoParams {
    fn is_zero(&self) -> bool {
        true
    }
}

// Go v2 default struct arshaler for `struct{}`: `{}`.
impl MarshalerTo for NoParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        write_object_end(enc);
        Ok(())
    }
}

// Go v2 default struct arshaler for `struct{}`: null or an object (members
// skipped); any other kind is an error.
impl UnmarshalerFrom for NoParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_struct_fields(dec, "lsproto.NoParams", |_, _| Ok(false))?;
        Ok(())
    }
}

// Go: lsp.go:287 clientCapabilitiesKey
static CLIENT_CAPABILITIES_KEY: gostd::context::ContextKey<Arc<ResolvedClientCapabilities>> =
    gostd::context::ContextKey::new("clientCapabilitiesKey");

// Go: lsp.go:289 WithClientCapabilities
// PORT: Go stores the `*ResolvedClientCapabilities` pointer; the context
// holds an `Arc` to it.
pub fn with_client_capabilities(ctx: &Context, caps: Arc<ResolvedClientCapabilities>) -> Context {
    gostd::context::with_value(ctx, &CLIENT_CAPABILITIES_KEY, caps)
}

// Go: lsp.go:293 GetClientCapabilities
pub fn get_client_capabilities(ctx: &Context) -> Arc<ResolvedClientCapabilities> {
    if let Some(caps) = ctx.value(&CLIENT_CAPABILITIES_KEY) {
        return (*caps).clone();
    }
    Arc::new(ResolvedClientCapabilities::default())
}

// Go: lsp.go:302 PreferredMarkupKind
// PreferredMarkupKind returns the first (most preferred) markup kind from the given formats,
// or MarkupKindPlainText if the slice is empty.
pub fn preferred_markup_kind(formats: &[MarkupKind]) -> MarkupKind {
    if !formats.is_empty() {
        return formats[0].clone();
    }
    MarkupKind::PLAIN_TEXT
}

impl CodeActionKind {
    // Go: lsp.go:310 (CodeActionKind).Contains
    // Contains reports whether other is this code action kind or one of its children.
    #[must_use]
    pub fn contains(&self, other: &CodeActionKind) -> bool {
        *self == *other
            || *self == CodeActionKind::EMPTY
            || other
                .0
                .strip_prefix(self.0.as_ref())
                .is_some_and(|rest| rest.starts_with('.'))
    }
}

// Go: lsp.go:316
impl CodeActionKind {
    // Go: CodeActionKindSourceFixAll + ".ts"
    pub const SOURCE_FIX_ALL_TS: CodeActionKind = CodeActionKind(Cow::Borrowed("source.fixAll.ts"));
    // Go: CodeActionKindSourceOrganizeImports + ".ts"
    pub const SOURCE_ORGANIZE_IMPORTS_TS: CodeActionKind =
        CodeActionKind(Cow::Borrowed("source.organizeImports.ts"));
    // Go: CodeActionKindSource + ".removeUnusedImports.ts"
    pub const SOURCE_REMOVE_UNUSED_IMPORTS_TS: CodeActionKind =
        CodeActionKind(Cow::Borrowed("source.removeUnusedImports.ts"));
    // Go: CodeActionKindSource + ".sortImports.ts"
    pub const SOURCE_SORT_IMPORTS_TS: CodeActionKind =
        CodeActionKind(Cow::Borrowed("source.sortImports.ts"));
}

#[cfg(test)]
mod unmarshal_error_tests {
    use super::*;

    /// The error of `unmarshal_params::<T>` for a request whose raw params
    /// are `data`.
    fn params_err_text<T: UnmarshalerFrom + Default + std::fmt::Debug + 'static>(
        data: &str,
    ) -> String {
        let req = RequestMessage {
            params: Some(Box::new(JsonValue(data.as_bytes().to_vec()))),
            ..RequestMessage::default()
        };
        unmarshal_params::<T>(&req)
            .expect_err("want an error")
            .error()
    }

    // Expected texts come from Go at the pin (tsgo-oracle-52168999f3dc; the
    // probe is in continuation-r97-goport/upstream/bumpA2/evidence/
    // lsp-invalid-params-N.txt). Since tsgo#4471 Go decodes params at
    // dispatch; the text is the old inner JSON error after "InvalidParams: ".
    // Go writes "cannot" or "unable to".
    #[test]
    fn params_errors_match_go() {
        assert_eq!(
            params_err_text::<HoverParams>(r#"{"textDocument":{"uri":"file:///a.ts"}}"#),
            "InvalidParams: json: cannot unmarshal into Go lsproto.HoverParams after offset 38: missing required properties: position"
        );
        assert_eq!(
            params_err_text::<SignatureHelpParams>(
                r#"{"textDocument":{"uri":"file:///a.ts"},"position":{"line":0,"character":0},"context":5}"#
            ),
            r#"InvalidParams: json: cannot unmarshal into Go lsproto.SignatureHelpContext within "/context": expected object start, but encountered number"#
        );
        assert_eq!(
            params_err_text::<HoverParams>(
                r#"{"textDocument":{"uri":5},"position":{"line":0,"character":0}}"#
            ),
            r#"InvalidParams: json: cannot unmarshal JSON number into Go lsproto.DocumentUri within "/textDocument/uri""#
        );
        assert_eq!(
            params_err_text::<HoverParams>(
                r#"{"textDocument":{"uri":"file:///a.ts"},"position":{"line":"x","character":0}}"#
            ),
            r#"InvalidParams: json: cannot unmarshal JSON string into Go uint32 within "/position/line""#
        );
        assert_eq!(
            params_err_text::<HoverParams>("null"),
            "InvalidParams: params must be an object or array"
        );
        assert_eq!(
            params_err_text::<HoverParams>("5"),
            "InvalidParams: params must be an object or array"
        );
        assert_eq!(
            params_err_text::<NoParams>("{}"),
            "InvalidParams: expected no params, got {}"
        );
    }

    // S6-002: a signatureHelp retrigger whose `activeSignatureHelp` is null.
    // Since tsgo#4471 the message reads without error and the handler's
    // `UnmarshalParams` fails (Go at the pin, same evidence file).
    #[test]
    fn null_active_signature_help_matches_go() {
        let data = br#"{"jsonrpc":"2.0","id":4,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///a.ts"},"position":{"line":0,"character":0},"context":{"triggerKind":3,"isRetrigger":true,"activeSignatureHelp":null}}}"#;
        let mut msg = Message::default();
        msg.unmarshal_json(data).expect("the message reads");
        let err =
            unmarshal_params::<SignatureHelpParams>(msg.as_request()).expect_err("want an error");
        assert_eq!(
            err.error(),
            r#"InvalidParams: json: cannot unmarshal into Go lsproto.SignatureHelpContext within "/context/activeSignatureHelp": null value is not allowed for field "activeSignatureHelp""#
        );
    }
}
