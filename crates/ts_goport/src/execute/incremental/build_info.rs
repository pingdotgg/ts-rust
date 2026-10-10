//! Port of execute/incremental/buildInfo.go: the `.tsbuildinfo` JSON shapes
//! and their custom JSON (un)marshalers.
//!
//! PORT: Go uses JSON v2 reflection plus `MarshalJSON`/`UnmarshalJSON`
//! methods. Here every type implements `MarshalerTo` and `UnmarshalerFrom`
//! from `frontend/json.rs` by hand, in Go field order, so the output bytes
//! match Go `json.Marshal`:
//! - `omitzero` struct fields are skipped when zero. A Go `[]T` field with
//!   `omitzero` is `Option<Vec<T>>`, because Go writes an empty non-nil
//!   slice as `[]` and omits only a nil slice.
//! - Go `[]*T` elements that are JSON `null` become nil pointers in Go. The
//!   Rust elements are values, so a `null` element is an unmarshal error
//!   (the reader then returns no buildinfo). Go writes such an element only
//!   in `emitDiagnosticsPerFile` (`BuildInfoDiagnosticsOfFilePtr`).
//! - Go `int` is `i32` (PORTING.md). Integers are read like the v2 int
//!   arshaler: digits only, no fraction or exponent.

use super::hash::*;
use crate::contentmapper;
use crate::frontend::prelude::*;
use crate::gostd::GoError;
// `vfs::FileInfo` is also in the frontend prelude.
use super::hash::FileInfo;

// ---------------------------------------------------------------------------
// JSON helpers (Go JSON v2 default arshalers for the kinds used here).
// ---------------------------------------------------------------------------

fn json_error(message: impl Into<String>) -> JsonError {
    JsonError {
        message: message.into(),
    }
}

// Go v2 int arshaler: null gives 0, a number must be an integer in range,
// any other kind is an error after the value is read.
fn unmarshal_int(dec: &mut JsonDecoder<'_>) -> Result<i64, JsonError> {
    match dec.peek_kind() {
        b'n' => {
            dec.read_token()?;
            Ok(0)
        }
        b'0' => {
            // PERF: the number text is borrowed (`read_token_ref`).
            let JsonTokenRef::Number(raw) = dec.read_token_ref()? else {
                unreachable!("peeked a number")
            };
            parse_go_int(raw)
        }
        _ => {
            dec.skip_value()?;
            Err(json_error("cannot unmarshal JSON value into Go int"))
        }
    }
}

// Go v2 `jsonwire.ParseUint` with a sign: digits only, range of int64.
fn parse_go_int(raw: &str) -> Result<i64, JsonError> {
    let (neg, digits) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(json_error(format!(
            "cannot unmarshal JSON number {raw} into Go int: invalid syntax"
        )));
    }
    let magnitude: u64 = digits.parse().map_err(|_| {
        json_error(format!(
            "cannot unmarshal JSON number {raw} into Go int: value out of range"
        ))
    })?;
    let max_int = 1u64 << 63;
    if (neg && magnitude > max_int) || (!neg && magnitude > max_int - 1) {
        return Err(json_error(format!(
            "cannot unmarshal JSON number {raw} into Go int: value out of range"
        )));
    }
    Ok(if neg {
        (magnitude as i64).wrapping_neg()
    } else {
        magnitude as i64
    })
}

fn unmarshal_i32(dec: &mut JsonDecoder<'_>) -> Result<i32, JsonError> {
    // PORT: Go `int` is 64 bits; the Rust port uses `i32`.
    Ok(unmarshal_int(dec)? as i32)
}

fn unmarshal_string(dec: &mut JsonDecoder<'_>) -> Result<String, JsonError> {
    let mut s = String::new();
    json_unmarshal_decode(dec, &mut s)?;
    Ok(s)
}

fn unmarshal_bool(dec: &mut JsonDecoder<'_>) -> Result<bool, JsonError> {
    let mut b = false;
    json_unmarshal_decode(dec, &mut b)?;
    Ok(b)
}

// Go v2 slice arshaler: null gives a nil slice (`None`), an array gives a
// non-nil slice, any other kind is an error.
fn unmarshal_slice<T>(
    dec: &mut JsonDecoder<'_>,
    mut elem: impl FnMut(&mut JsonDecoder<'_>) -> Result<T, JsonError>,
) -> Result<Option<Vec<T>>, JsonError> {
    match dec.peek_kind() {
        b'n' => {
            dec.read_token()?;
            Ok(None)
        }
        b'[' => {
            dec.read_token()?;
            let mut out = Vec::new();
            while dec.peek_kind() != b']' {
                out.push(elem(dec)?);
            }
            dec.read_token()?;
            Ok(Some(out))
        }
        _ => {
            dec.skip_value()?;
            Err(json_error("cannot unmarshal JSON value into Go slice"))
        }
    }
}

// Unmarshal one element of a Go `[]*T` slice.
fn unmarshal_elem<T: UnmarshalerFrom + Default>(dec: &mut JsonDecoder<'_>) -> Result<T, JsonError> {
    if dec.peek_kind() == b'n' {
        dec.read_token()?;
        return Err(json_error("PORT: null element for a Go pointer"));
    }
    let mut v = T::default();
    json_unmarshal_decode(dec, &mut v)?;
    Ok(v)
}

// Go v2 struct arshaler over a JSON object. `field` returns false for an
// unknown member name, which is skipped (v2 does not reject unknown
// members by default). Null leaves the zero value.
fn unmarshal_object(
    dec: &mut JsonDecoder<'_>,
    mut field: impl FnMut(&str, &mut JsonDecoder<'_>) -> Result<bool, JsonError>,
) -> Result<(), JsonError> {
    match dec.peek_kind() {
        b'n' => {
            dec.read_token()?;
            Ok(())
        }
        b'{' => {
            dec.read_token()?;
            while dec.peek_kind() != b'}' {
                // PERF: the name is borrowed when it can be (`read_token_ref`).
                let JsonTokenRef::String(name) = dec.read_token_ref()? else {
                    return Err(json_error("invalid object member name"));
                };
                if !field(&name, dec)? {
                    dec.skip_value()?;
                }
            }
            dec.read_token()?;
            Ok(())
        }
        _ => {
            dec.skip_value()?;
            Err(json_error("cannot unmarshal JSON value into Go struct"))
        }
    }
}

// Go `json.Unmarshal(data, &v)` for the nested calls inside the Go
// `UnmarshalJSON` methods.
fn unmarshal_bytes<T: UnmarshalerFrom + Default>(data: &[u8]) -> Result<T, JsonError> {
    let mut v = T::default();
    json_unmarshal(data, &mut v, &[])?;
    Ok(v)
}

// Go `json.Unmarshal` into a value read by a closure.
fn unmarshal_bytes_with<T>(
    data: &[u8],
    read: impl FnOnce(&mut JsonDecoder<'_>) -> Result<T, JsonError>,
) -> Result<T, JsonError> {
    let mut dec = JsonDecoder::new(data, JsonOptions::default());
    let v = read(&mut dec)?;
    dec.check_eof()?;
    Ok(v)
}

// Go v2 `[N]int` arshaler: exactly N elements.
fn unmarshal_int_array<const N: usize>(dec: &mut JsonDecoder<'_>) -> Result<[i32; N], JsonError> {
    let mut out = [0i32; N];
    match dec.peek_kind() {
        b'n' => {
            dec.read_token()?;
            Ok(out)
        }
        b'[' => {
            dec.read_token()?;
            let mut i = 0;
            while dec.peek_kind() != b']' {
                if i >= N {
                    return Err(json_error("too many array elements"));
                }
                out[i] = unmarshal_i32(dec)?;
                i += 1;
            }
            dec.read_token()?;
            if i < N {
                return Err(json_error("too few array elements"));
            }
            Ok(out)
        }
        _ => {
            dec.skip_value()?;
            Err(json_error("cannot unmarshal JSON value into Go array"))
        }
    }
}

// Go v2 `any` arshaler: null, bool, float64, string, []any and
// map[string]any.
// PORT: Go `map[string]any` does not keep member order; the Rust map keeps
// the input order.
fn unmarshal_any(dec: &mut JsonDecoder<'_>) -> Result<CompilerOptionsValue, JsonError> {
    match dec.peek_kind() {
        b'n' => {
            dec.read_token()?;
            Ok(CompilerOptionsValue::Nil)
        }
        b't' | b'f' => Ok(CompilerOptionsValue::Bool(unmarshal_bool(dec)?)),
        b'"' => Ok(CompilerOptionsValue::String(unmarshal_string(dec)?)),
        b'0' => {
            let mut f = 0.0f64;
            json_unmarshal_decode(dec, &mut f)?;
            Ok(CompilerOptionsValue::Number(f))
        }
        b'[' => Ok(CompilerOptionsValue::List(
            unmarshal_slice(dec, unmarshal_any)?.unwrap_or_default(),
        )),
        b'{' => {
            dec.read_token()?;
            let mut map = IndexMap::new();
            while dec.peek_kind() != b'}' {
                let key = unmarshal_string(dec)?;
                let value = unmarshal_any(dec)?;
                map.insert(key, value);
            }
            dec.read_token()?;
            Ok(CompilerOptionsValue::Map(map))
        }
        _ => {
            dec.skip_value()?;
            Err(json_error("invalid JSON value"))
        }
    }
}

// Go `collections.OrderedMap[string, any].UnmarshalJSONFrom`. A pointer
// field that is JSON null stays nil.
fn unmarshal_options(
    dec: &mut JsonDecoder<'_>,
) -> Result<Option<IndexMap<String, CompilerOptionsValue>>, JsonError> {
    let token = dec.read_token()?;
    if token.kind() == b'n' {
        return Ok(None);
    }
    if token.kind() != b'{' {
        return Err(json_error(
            "cannot unmarshal non-object JSON value into Map",
        ));
    }
    let mut map = IndexMap::new();
    while dec.peek_kind() != b'}' {
        let key = unmarshal_string(dec)?;
        let value = unmarshal_any(dec)?;
        map.insert(key, value);
    }
    dec.read_token()?;
    Ok(Some(map))
}

fn marshal_int(enc: &mut String, v: i64) {
    enc.push_str(&v.to_string());
}

// Go v2 float64 marshaler (`jsonwire.AppendFloat`): ECMA-262 number
// formatting, except that -0 is "-0".
fn marshal_float(enc: &mut String, v: f64) -> Result<(), JsonError> {
    if !v.is_finite() {
        return Err(json_error(format!("unsupported value: {v}")));
    }
    if v == 0.0 && v.is_sign_negative() {
        enc.push_str("-0");
    } else {
        enc.push_str(&crate::jsnum::Number::from(v).to_string());
    }
    Ok(())
}

// Go v2 marshaler for the `any` values stored in `BuildInfo.Options`: the
// values read back from JSON, and the typed `core.CompilerOptions` field
// values that `snapshotToBuildInfo` stores.
pub fn marshal_any(enc: &mut String, v: &CompilerOptionsValue) -> Result<(), JsonError> {
    match v {
        CompilerOptionsValue::Nil | CompilerOptionsValue::IntPtr(None) => enc.push_str("null"),
        CompilerOptionsValue::Bool(b) => b.marshal_json_to(enc)?,
        CompilerOptionsValue::Int(i) | CompilerOptionsValue::IntPtr(Some(i)) => {
            marshal_int(enc, i64::from(*i));
        }
        CompilerOptionsValue::Number(f) => marshal_float(enc, *f)?,
        CompilerOptionsValue::String(s) => s.marshal_json_to(enc)?,
        // Go v2 struct arshaler: `diagnostics.Message` has only unexported
        // fields and no `json` tags, so marshaling it fails with
        // `errNoExportedFields` (go-json-experiment fields.go:77).
        // PORT: v2 also names the JSON pointer and picks "cannot" or
        // "unable to" at random; this text has neither (see json.rs).
        CompilerOptionsValue::Message(_) => {
            return Err(json_error(
                "json: cannot marshal from Go diagnostics.Message: Go struct has no exported fields",
            ));
        }
        // Go: core/tristate.go:54 MarshalJSON
        CompilerOptionsValue::Tristate(t) => {
            enc.push_str(std::str::from_utf8(t.marshal_json()).expect("ascii"));
        }
        CompilerOptionsValue::ScriptTarget(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::ModuleKind(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::ModuleResolutionKind(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::ModuleDetectionKind(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::JsxEmit(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::NewLineKind(k) => marshal_int(enc, i64::from(k.0)),
        CompilerOptionsValue::List(list) => {
            enc.push('[');
            for (i, item) in list.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                marshal_any(enc, item)?;
            }
            enc.push(']');
        }
        CompilerOptionsValue::Map(map) => {
            enc.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                k.marshal_json_to(enc)?;
                enc.push(':');
                marshal_any(enc, item)?;
            }
            enc.push('}');
        }
        CompilerOptionsValue::StringList(list) => list.marshal_json_to(enc)?,
        // Go v2 writes a nil slice as `[]`.
        CompilerOptionsValue::NilList => enc.push_str("[]"),
        // Go `*collections.OrderedMap[string, []string]`: nil is null, and a
        // nil `[]string` value is `[]`.
        CompilerOptionsValue::Paths(None) => enc.push_str("null"),
        CompilerOptionsValue::Paths(Some(map)) => {
            enc.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                k.marshal_json_to(enc)?;
                enc.push(':');
                item.as_deref().unwrap_or_default().marshal_json_to(enc)?;
            }
            enc.push('}');
        }
        CompilerOptionsValue::EmptyStruct => enc.push_str("{}"),
    }
    Ok(())
}

// Writes the members of a Go struct in field order.
struct ObjectWriter<'a> {
    enc: &'a mut String,
    first: bool,
}

impl<'a> ObjectWriter<'a> {
    fn new(enc: &'a mut String) -> ObjectWriter<'a> {
        enc.push('{');
        ObjectWriter { enc, first: true }
    }

    fn name(&mut self, name: &str) -> &mut String {
        if !self.first {
            self.enc.push(',');
        }
        self.first = false;
        self.enc.push('"');
        self.enc.push_str(name);
        self.enc.push_str("\":");
        self.enc
    }

    fn bool_omitzero(&mut self, name: &str, v: bool) {
        if v {
            self.name(name).push_str("true");
        }
    }

    fn int_omitzero(&mut self, name: &str, v: i64) {
        if v != 0 {
            marshal_int(self.name(name), v);
        }
    }

    fn string_omitzero(&mut self, name: &str, v: &str) -> Result<(), JsonError> {
        if !v.is_empty() {
            v.marshal_json_to(self.name(name))?;
        }
        Ok(())
    }

    fn slice_omitzero<T: MarshalerTo>(
        &mut self,
        name: &str,
        v: Option<&Vec<T>>,
    ) -> Result<(), JsonError> {
        if let Some(v) = v {
            v.marshal_json_to(self.name(name))?;
        }
        Ok(())
    }

    fn end(self) {
        self.enc.push('}');
    }
}

// ---------------------------------------------------------------------------
// buildInfo.go
// ---------------------------------------------------------------------------

// Go: incremental/buildInfo.go:16 BuildInfoFileId
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BuildInfoFileId(pub i32);

// Go: incremental/buildInfo.go:17 BuildInfoFileIdListId
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BuildInfoFileIdListId(pub i32);

impl UnmarshalerFrom for BuildInfoFileId {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        self.0 = unmarshal_i32(dec)?;
        Ok(())
    }
}

impl MarshalerTo for BuildInfoFileId {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        marshal_int(enc, i64::from(self.0));
        Ok(())
    }
}

// Go: incremental/buildInfo.go:21 BuildInfoRoot
// buildInfoRoot is
// - for incremental program buildinfo
//   - start and end of FileId for consecutive fileIds to be included as root
//   - start - single fileId that is root
//
// - for non incremental program buildinfo
//   - string that is the root file name
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoRoot {
    pub start: BuildInfoFileId,
    pub end: BuildInfoFileId,
    pub non_incremental: String, // Root of a non incremental program
}

impl MarshalerTo for BuildInfoRoot {
    // Go: incremental/buildInfo.go:35 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if self.start.0 != 0 {
            if self.end.0 != 0 {
                vec![self.start, self.end].marshal_json_to(enc)
            } else {
                self.start.marshal_json_to(enc)
            }
        } else {
            self.non_incremental.marshal_json_to(enc)
        }
    }
}

impl UnmarshalerFrom for BuildInfoRoot {
    // Go: incremental/buildInfo.go:47 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        match unmarshal_bytes_with(data, unmarshal_int_array::<2>) {
            Err(_) => match unmarshal_bytes_with(data, unmarshal_i32) {
                Err(_) => match unmarshal_bytes::<String>(data) {
                    Err(_) => Err(json_error(format!(
                        "invalid BuildInfoRoot: {}",
                        String::from_utf8_lossy(data)
                    ))),
                    Ok(name) => {
                        *self = BuildInfoRoot {
                            non_incremental: name,
                            ..Default::default()
                        };
                        Ok(())
                    }
                },
                Ok(start) => {
                    *self = BuildInfoRoot {
                        start: BuildInfoFileId(start),
                        ..Default::default()
                    };
                    Ok(())
                }
            },
            Ok(start_and_end) => {
                *self = BuildInfoRoot {
                    start: BuildInfoFileId(start_and_end[0]),
                    end: BuildInfoFileId(start_and_end[1]),
                    non_incremental: String::new(),
                };
                Ok(())
            }
        }
    }
}

// Go: incremental/buildInfo.go:73 buildInfoFileInfoNoSignature
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfoNoSignature {
    pub version: String,
    pub no_signature: bool,
    pub affects_global_scope: bool,
    pub implied_node_format: ResolutionMode,
}

impl MarshalerTo for BuildInfoFileInfoNoSignature {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("version", &self.version)?;
        w.bool_omitzero("noSignature", self.no_signature);
        w.bool_omitzero("affectsGlobalScope", self.affects_global_scope);
        w.int_omitzero("impliedNodeFormat", i64::from(self.implied_node_format.0));
        w.end();
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoFileInfoNoSignature {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "version" => self.version = unmarshal_string(dec)?,
                "noSignature" => self.no_signature = unmarshal_bool(dec)?,
                "affectsGlobalScope" => self.affects_global_scope = unmarshal_bool(dec)?,
                "impliedNodeFormat" => self.implied_node_format = ModuleKind(unmarshal_i32(dec)?),
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

// Go: incremental/buildInfo.go:83 buildInfoFileInfoWithSignature
//
//	 Signature is
//		 - undefined if FileInfo.version === FileInfo.signature
//		 - string actual signature
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfoWithSignature {
    pub version: String,
    pub signature: String,
    pub affects_global_scope: bool,
    pub implied_node_format: ResolutionMode,
}

impl MarshalerTo for BuildInfoFileInfoWithSignature {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("version", &self.version)?;
        w.string_omitzero("signature", &self.signature)?;
        w.bool_omitzero("affectsGlobalScope", self.affects_global_scope);
        w.int_omitzero("impliedNodeFormat", i64::from(self.implied_node_format.0));
        w.end();
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoFileInfoWithSignature {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "version" => self.version = unmarshal_string(dec)?,
                "signature" => self.signature = unmarshal_string(dec)?,
                "affectsGlobalScope" => self.affects_global_scope = unmarshal_bool(dec)?,
                "impliedNodeFormat" => self.implied_node_format = ModuleKind(unmarshal_i32(dec)?),
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

/// PORT: not in Go (perf). The members of both object forms of a
/// `BuildInfoFileInfo`, decoded in one pass. When this decode succeeds,
/// the Go decodes of `buildInfoFileInfoNoSignature` and
/// `buildInfoFileInfoWithSignature` succeed too: each reads a subset of
/// these members with the same decoders and skips the rest, and the value
/// is valid JSON with no duplicate names. So the result is the one of
/// Go's `UnmarshalJSON` for an object.
#[derive(Default)]
struct BuildInfoFileInfoMembers {
    version: String,
    signature: String,
    no_signature: bool,
    affects_global_scope: bool,
    implied_node_format: ResolutionMode,
}

impl BuildInfoFileInfoMembers {
    /// The `BuildInfoFileInfo` of the object `data`, or `None` when the
    /// one-pass decode fails.
    fn decode(data: &[u8]) -> Option<BuildInfoFileInfo> {
        let members = unmarshal_bytes::<BuildInfoFileInfoMembers>(data).ok()?;
        Some(if members.no_signature {
            BuildInfoFileInfo {
                no_signature: Some(BuildInfoFileInfoNoSignature {
                    version: members.version,
                    no_signature: true,
                    affects_global_scope: members.affects_global_scope,
                    implied_node_format: members.implied_node_format,
                }),
                ..Default::default()
            }
        } else {
            BuildInfoFileInfo {
                file_info: Some(BuildInfoFileInfoWithSignature {
                    version: members.version,
                    signature: members.signature,
                    affects_global_scope: members.affects_global_scope,
                    implied_node_format: members.implied_node_format,
                }),
                ..Default::default()
            }
        })
    }
}

impl UnmarshalerFrom for BuildInfoFileInfoMembers {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "version" => self.version = unmarshal_string(dec)?,
                "signature" => self.signature = unmarshal_string(dec)?,
                "noSignature" => self.no_signature = unmarshal_bool(dec)?,
                "affectsGlobalScope" => self.affects_global_scope = unmarshal_bool(dec)?,
                "impliedNodeFormat" => self.implied_node_format = ModuleKind(unmarshal_i32(dec)?),
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

// Go: incremental/buildInfo.go:90 BuildInfoFileInfo
// PORT: Go unexported fields are plain `pub` fields. Go nil pointers are
// `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFileInfo {
    pub signature: String,
    pub no_signature: Option<BuildInfoFileInfoNoSignature>,
    pub file_info: Option<BuildInfoFileInfoWithSignature>,
}

// Go: incremental/buildInfo.go:96 newBuildInfoFileInfo
#[must_use]
pub fn new_build_info_file_info(file_info: &FileInfo) -> BuildInfoFileInfo {
    if file_info.version == file_info.signature {
        if !file_info.affects_global_scope
            && file_info.implied_node_format == RESOLUTION_MODE_COMMON_JS
        {
            return BuildInfoFileInfo {
                signature: file_info.signature.clone(),
                ..Default::default()
            };
        }
    } else if file_info.signature.is_empty() {
        return BuildInfoFileInfo {
            no_signature: Some(BuildInfoFileInfoNoSignature {
                version: file_info.version.clone(),
                no_signature: true,
                affects_global_scope: file_info.affects_global_scope,
                implied_node_format: file_info.implied_node_format,
            }),
            ..Default::default()
        };
    }
    BuildInfoFileInfo {
        file_info: Some(BuildInfoFileInfoWithSignature {
            version: file_info.version.clone(),
            signature: if file_info.signature == file_info.version {
                String::new()
            } else {
                file_info.signature.clone()
            },
            affects_global_scope: file_info.affects_global_scope,
            implied_node_format: file_info.implied_node_format,
        }),
        ..Default::default()
    }
}

impl BuildInfoFileInfo {
    // Go: incremental/buildInfo.go:116 GetFileInfo
    // PORT: the Go nil receiver check is the caller's `Option`.
    #[must_use]
    pub fn get_file_info(&self) -> FileInfo {
        if !self.signature.is_empty() {
            return FileInfo {
                version: self.signature.clone(),
                signature: self.signature.clone(),
                implied_node_format: RESOLUTION_MODE_COMMON_JS,
                ..Default::default()
            };
        }
        if let Some(no_signature) = &self.no_signature {
            return FileInfo {
                version: no_signature.version.clone(),
                affects_global_scope: no_signature.affects_global_scope,
                implied_node_format: no_signature.implied_node_format,
                ..Default::default()
            };
        }
        let file_info = self
            .file_info
            .as_ref()
            .expect("BuildInfoFileInfo has a fileInfo");
        FileInfo {
            version: file_info.version.clone(),
            signature: if file_info.signature.is_empty() {
                file_info.version.clone()
            } else {
                file_info.signature.clone()
            },
            affects_global_scope: file_info.affects_global_scope,
            implied_node_format: file_info.implied_node_format,
        }
    }

    // Go: incremental/buildInfo.go:142 HasSignature
    #[must_use]
    pub fn has_signature(&self) -> bool {
        !self.signature.is_empty()
    }
}

impl MarshalerTo for BuildInfoFileInfo {
    // Go: incremental/buildInfo.go:146 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if !self.signature.is_empty() {
            return self.signature.marshal_json_to(enc);
        }
        if let Some(no_signature) = &self.no_signature {
            return no_signature.marshal_json_to(enc);
        }
        match &self.file_info {
            Some(file_info) => file_info.marshal_json_to(enc),
            // Go `json.Marshal` of a nil pointer.
            None => {
                enc.push_str("null");
                Ok(())
            }
        }
    }
}

impl UnmarshalerFrom for BuildInfoFileInfo {
    // Go: incremental/buildInfo.go:156 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        // PORT: perf. An object decodes once into the members of both
        // forms; Go decodes it up to three times. A value that fails
        // here takes the Go path below.
        if data.first() == Some(&b'{')
            && let Some(file_info) = BuildInfoFileInfoMembers::decode(data)
        {
            *self = file_info;
            return Ok(());
        }
        match unmarshal_bytes::<String>(data) {
            Err(_) => {
                let no_signature = unmarshal_bytes::<BuildInfoFileInfoNoSignature>(data);
                match no_signature {
                    Ok(no_signature) if no_signature.no_signature => {
                        *self = BuildInfoFileInfo {
                            no_signature: Some(no_signature),
                            ..Default::default()
                        };
                        Ok(())
                    }
                    _ => match unmarshal_bytes::<BuildInfoFileInfoWithSignature>(data) {
                        Err(_) => Err(json_error(format!(
                            "invalid BuildInfoFileInfo: {}",
                            String::from_utf8_lossy(data)
                        ))),
                        Ok(file_info) => {
                            *self = BuildInfoFileInfo {
                                file_info: Some(file_info),
                                ..Default::default()
                            };
                            Ok(())
                        }
                    },
                }
            }
            Ok(v_signature) => {
                *self = BuildInfoFileInfo {
                    signature: v_signature,
                    ..Default::default()
                };
                Ok(())
            }
        }
    }
}

// Go: incremental/buildInfo.go:176 BuildInfoReferenceMapEntry
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoReferenceMapEntry {
    pub file_id: BuildInfoFileId,
    pub file_id_list_id: BuildInfoFileIdListId,
}

impl MarshalerTo for BuildInfoReferenceMapEntry {
    // Go: incremental/buildInfo.go:181 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        marshal_int(enc, i64::from(self.file_id.0));
        enc.push(',');
        marshal_int(enc, i64::from(self.file_id_list_id.0));
        enc.push(']');
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoReferenceMapEntry {
    // Go: incremental/buildInfo.go:185 UnmarshalJSON
    // PORT: Go reads a `*[2]int`; null leaves it nil and then panics on the
    // index. The port returns an error instead.
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        let v = unmarshal_bytes_with(data, unmarshal_int_array::<2>)?;
        *self = BuildInfoReferenceMapEntry {
            file_id: BuildInfoFileId(v[0]),
            file_id_list_id: BuildInfoFileIdListId(v[1]),
        };
        Ok(())
    }
}

// Go: incremental/buildInfo.go:197 BuildInfoDiagnostic
// PORT: Go `diagnostics.Category` (int32) is stored as `i32`, so the zero
// value (`CategoryWarning`) is omitted like in Go. Go `diagnostics.Key` is
// `String`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnostic {
    // BuildInfoFileId if it is for a File thats other than its stored for
    pub file: BuildInfoFileId,
    pub no_file: bool,
    pub pos: i32,
    pub end: i32,
    pub code: i32,
    pub category: i32,
    pub source: String,
    pub message_text: String,
    pub message_key: String,
    pub message_args: Option<Vec<String>>,
    pub message_chain: Option<Vec<BuildInfoDiagnostic>>,
    pub related_information: Option<Vec<BuildInfoDiagnostic>>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub skipped_on_no_emit: bool,
    pub repopulate_info: Option<BuildInfoRepopulateInfo>,
}

impl MarshalerTo for BuildInfoDiagnostic {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.int_omitzero("file", i64::from(self.file.0));
        w.bool_omitzero("noFile", self.no_file);
        w.int_omitzero("pos", i64::from(self.pos));
        w.int_omitzero("end", i64::from(self.end));
        w.int_omitzero("code", i64::from(self.code));
        w.int_omitzero("category", i64::from(self.category));
        w.string_omitzero("source", &self.source)?;
        w.string_omitzero("messageText", &self.message_text)?;
        w.string_omitzero("messageKey", &self.message_key)?;
        w.slice_omitzero("messageArgs", self.message_args.as_ref())?;
        w.slice_omitzero("messageChain", self.message_chain.as_ref())?;
        w.slice_omitzero("relatedInformation", self.related_information.as_ref())?;
        w.bool_omitzero("reportsUnnecessary", self.reports_unnecessary);
        w.bool_omitzero("reportsDeprecated", self.reports_deprecated);
        w.bool_omitzero("skippedOnNoEmit", self.skipped_on_no_emit);
        if let Some(info) = &self.repopulate_info {
            info.marshal_json_to(w.name("repopulateInfo"))?;
        }
        w.end();
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoDiagnostic {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "file" => self.file = BuildInfoFileId(unmarshal_i32(dec)?),
                "noFile" => self.no_file = unmarshal_bool(dec)?,
                "pos" => self.pos = unmarshal_i32(dec)?,
                "end" => self.end = unmarshal_i32(dec)?,
                "code" => self.code = unmarshal_i32(dec)?,
                "category" => self.category = unmarshal_i32(dec)?,
                "source" => self.source = unmarshal_string(dec)?,
                "messageText" => self.message_text = unmarshal_string(dec)?,
                "messageKey" => self.message_key = unmarshal_string(dec)?,
                "messageArgs" => self.message_args = unmarshal_slice(dec, unmarshal_string)?,
                "messageChain" => self.message_chain = unmarshal_slice(dec, unmarshal_elem)?,
                "relatedInformation" => {
                    self.related_information = unmarshal_slice(dec, unmarshal_elem)?;
                }
                "reportsUnnecessary" => self.reports_unnecessary = unmarshal_bool(dec)?,
                "reportsDeprecated" => self.reports_deprecated = unmarshal_bool(dec)?,
                "skippedOnNoEmit" => self.skipped_on_no_emit = unmarshal_bool(dec)?,
                "repopulateInfo" => {
                    self.repopulate_info = if dec.peek_kind() == b'n' {
                        dec.read_token()?;
                        None
                    } else {
                        let mut info = BuildInfoRepopulateInfo::default();
                        json_unmarshal_decode(dec, &mut info)?;
                        Some(info)
                    };
                }
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

// Go: incremental/buildInfo.go:214 BuildInfoRepopulateInfo
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoRepopulateInfo {
    pub kind: RepopulateDiagnosticKind,
    pub module_reference: String,
    pub mode: ResolutionMode,
    pub package_name: String,
}

impl MarshalerTo for BuildInfoRepopulateInfo {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        marshal_int(w.name("kind"), i64::from(self.kind.0));
        w.string_omitzero("moduleReference", &self.module_reference)?;
        w.int_omitzero("mode", i64::from(self.mode.0));
        w.string_omitzero("packageName", &self.package_name)?;
        w.end();
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoRepopulateInfo {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "kind" => self.kind = RepopulateDiagnosticKind(unmarshal_i32(dec)?),
                "moduleReference" => self.module_reference = unmarshal_string(dec)?,
                "mode" => self.mode = ModuleKind(unmarshal_i32(dec)?),
                "packageName" => self.package_name = unmarshal_string(dec)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

// Go: incremental/buildInfo.go:221 BuildInfoDiagnosticsOfFile
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnosticsOfFile {
    pub file_id: BuildInfoFileId,
    pub diagnostics: Vec<BuildInfoDiagnostic>,
}

impl MarshalerTo for BuildInfoDiagnosticsOfFile {
    // Go: incremental/buildInfo.go:226 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        self.file_id.marshal_json_to(enc)?;
        enc.push(',');
        self.diagnostics.marshal_json_to(enc)?;
        enc.push(']');
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoDiagnosticsOfFile {
    // Go: incremental/buildInfo.go:233 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        *self = build_info_diagnostics_of_file_from_bytes(data)?;
        Ok(())
    }
}

/// Go `*BuildInfoDiagnosticsOfFile`, an element of
/// `BuildInfo.EmitDiagnosticsPerFile`. `None` is Go nil.
// PORT: `setEmitDiagnostics` keeps each `toBuildInfoDiagnosticsOfFile`
// result, and that is nil for a file whose cached lists are both empty. Go
// marshals the nil element as `null`. Field reads go through `Deref`, which
// panics on nil like a Go nil pointer dereference.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoDiagnosticsOfFilePtr(pub Option<BuildInfoDiagnosticsOfFile>);

impl std::ops::Deref for BuildInfoDiagnosticsOfFilePtr {
    type Target = BuildInfoDiagnosticsOfFile;

    fn deref(&self) -> &BuildInfoDiagnosticsOfFile {
        self.0
            .as_ref()
            .expect("invalid memory address or nil pointer dereference")
    }
}

impl MarshalerTo for BuildInfoDiagnosticsOfFilePtr {
    // Go v2 pointer marshaler: nil is `null`, else the pointee's `MarshalJSON`.
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        match &self.0 {
            Some(diagnostics) => diagnostics.marshal_json_to(enc),
            None => {
                enc.push_str("null");
                Ok(())
            }
        }
    }
}

impl UnmarshalerFrom for BuildInfoDiagnosticsOfFilePtr {
    // PORT: `unmarshal_elem` rejects a `null` element before this runs (see
    // the module doc), so the result is never nil.
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut diagnostics = BuildInfoDiagnosticsOfFile::default();
        diagnostics.unmarshal_json_from(dec)?;
        self.0 = Some(diagnostics);
        Ok(())
    }
}

// Go: incremental/buildInfo.go:233 UnmarshalJSON (body)
fn build_info_diagnostics_of_file_from_bytes(
    data: &[u8],
) -> Result<BuildInfoDiagnosticsOfFile, JsonError> {
    let file_id_and_diagnostics = unmarshal_bytes_with(data, |dec| {
        unmarshal_slice(dec, |dec| Ok(dec.read_value()?.to_vec()))
    })
    .map_err(|_| {
        json_error(format!(
            "invalid BuildInfoDiagnosticsOfFile: {}",
            String::from_utf8_lossy(data)
        ))
    })?
    .unwrap_or_default();
    if file_id_and_diagnostics.len() != 2 {
        return Err(json_error(format!(
            "invalid BuildInfoDiagnosticsOfFile: expected 2 elements, got {}",
            file_id_and_diagnostics.len()
        )));
    }
    let file_id = unmarshal_bytes::<BuildInfoFileId>(&file_id_and_diagnostics[0])
        .map_err(|e| json_error(format!("invalid fileId in BuildInfoDiagnosticsOfFile: {e}")))?;
    let diagnostics = unmarshal_bytes_with(&file_id_and_diagnostics[1], |dec| {
        unmarshal_slice(dec, unmarshal_elem::<BuildInfoDiagnostic>)
    })
    .map_err(|e| {
        json_error(format!(
            "invalid diagnostics in BuildInfoDiagnosticsOfFile: {e}"
        ))
    })?
    .unwrap_or_default();
    Ok(BuildInfoDiagnosticsOfFile {
        file_id,
        diagnostics,
    })
}

// Go: incremental/buildInfo.go:258 BuildInfoSemanticDiagnostic
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoSemanticDiagnostic {
    pub file_id: BuildInfoFileId, // File is not in changedSet and still doesnt have cached diagnostics
    pub diagnostics: Option<BuildInfoDiagnosticsOfFile>, // Diagnostics for file
}

impl MarshalerTo for BuildInfoSemanticDiagnostic {
    // Go: incremental/buildInfo.go:263 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if self.file_id.0 != 0 {
            return self.file_id.marshal_json_to(enc);
        }
        match &self.diagnostics {
            Some(diagnostics) => diagnostics.marshal_json_to(enc),
            None => {
                enc.push_str("null");
                Ok(())
            }
        }
    }
}

impl UnmarshalerFrom for BuildInfoSemanticDiagnostic {
    // Go: incremental/buildInfo.go:270 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        match unmarshal_bytes::<BuildInfoFileId>(data) {
            Err(_) => {
                let Ok(diagnostics) = build_info_diagnostics_of_file_from_bytes(data) else {
                    return Err(json_error(format!(
                        "invalid BuildInfoSemanticDiagnostic: {}",
                        String::from_utf8_lossy(data)
                    )));
                };
                *self = BuildInfoSemanticDiagnostic {
                    diagnostics: Some(diagnostics),
                    ..Default::default()
                };
                Ok(())
            }
            Ok(file_id) => {
                *self = BuildInfoSemanticDiagnostic {
                    file_id,
                    ..Default::default()
                };
                Ok(())
            }
        }
    }
}

// Go: incremental/buildInfo.go:289 BuildInfoFilePendingEmit
// fileId if pending emit is same as what compilerOptions suggest
// [fileId] if pending emit is only dts file emit
// [fileId, emitKind] if any other type emit is pending
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoFilePendingEmit {
    pub file_id: BuildInfoFileId,
    pub emit_kind: FileEmitKind,
}

impl MarshalerTo for BuildInfoFilePendingEmit {
    // Go: incremental/buildInfo.go:294 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if self.emit_kind.is_empty() {
            return self.file_id.marshal_json_to(enc);
        }
        if self.emit_kind == FileEmitKind::DTS {
            let file_list_ids = vec![self.file_id];
            return file_list_ids.marshal_json_to(enc);
        }
        enc.push('[');
        marshal_int(enc, i64::from(self.file_id.0));
        enc.push(',');
        marshal_int(enc, i64::from(self.emit_kind.0));
        enc.push(']');
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoFilePendingEmit {
    // Go: incremental/buildInfo.go:306 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        match unmarshal_bytes::<BuildInfoFileId>(data) {
            Err(_) => {
                let int_tuple =
                    unmarshal_bytes_with(data, |dec| unmarshal_slice(dec, unmarshal_int));
                let int_tuple = match int_tuple {
                    Ok(Some(t)) if !t.is_empty() => t,
                    _ => {
                        return Err(json_error(format!(
                            "invalid BuildInfoFilePendingEmit: {}",
                            String::from_utf8_lossy(data)
                        )));
                    }
                };
                match int_tuple.len() {
                    1 => {
                        *self = BuildInfoFilePendingEmit {
                            file_id: BuildInfoFileId(int_tuple[0] as i32),
                            emit_kind: FileEmitKind::DTS,
                        };
                        Ok(())
                    }
                    2 => {
                        *self = BuildInfoFilePendingEmit {
                            file_id: BuildInfoFileId(int_tuple[0] as i32),
                            emit_kind: FileEmitKind(int_tuple[1] as u32),
                        };
                        Ok(())
                    }
                    n => Err(json_error(format!(
                        "invalid BuildInfoFilePendingEmit: expected 1 or 2 integers, got {n}"
                    ))),
                }
            }
            Ok(file_id) => {
                *self = BuildInfoFilePendingEmit {
                    file_id,
                    ..Default::default()
                };
                Ok(())
            }
        }
    }
}

// Go: incremental/buildInfo.go:338 BuildInfoEmitSignature
// [fileId, signature] if different from file's signature
// fileId if file wasnt emitted
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoEmitSignature {
    pub file_id: BuildInfoFileId,
    pub signature: String, // Signature if it is different from file's Signature
    pub differs_only_in_dts_map: bool, // true if signature is different only in dtsMap value
    pub differs_in_options: bool, // true if signature is different in options used to emit file
}

impl BuildInfoEmitSignature {
    // Go: incremental/buildInfo.go:345 noEmitSignature
    #[must_use]
    pub fn no_emit_signature(&self) -> bool {
        self.signature.is_empty() && !self.differs_only_in_dts_map && !self.differs_in_options
    }

    // Go: incremental/buildInfo.go:350 toEmitSignature
    // PORT: Go `collections.SyncMap[tspath.Path, *emitSignature]` is a plain
    // map. Go dereferences a missing entry (nil pointer panic); so does the
    // `expect`.
    #[must_use]
    pub fn to_emit_signature(
        &self,
        path: &Path,
        emit_signatures: &FxHashMap<Path, EmitSignature>,
    ) -> EmitSignature {
        let mut signature = String::new();
        let mut signature_with_different_options: Option<Vec<String>> = None;
        if self.differs_only_in_dts_map {
            let mut list = Vec::with_capacity(1);
            let info = emit_signatures
                .get(path)
                .expect("emit signature for differsOnlyInDtsMap");
            list.push(info.signature.clone());
            signature_with_different_options = Some(list);
        } else if self.differs_in_options {
            let mut list = Vec::with_capacity(1);
            list.push(self.signature.clone());
            signature_with_different_options = Some(list);
        } else {
            signature = self.signature.clone();
        }
        EmitSignature {
            signature,
            signature_with_different_options,
        }
    }
}

impl MarshalerTo for BuildInfoEmitSignature {
    // Go: incremental/buildInfo.go:370 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if self.no_emit_signature() {
            return self.file_id.marshal_json_to(enc);
        }
        enc.push('[');
        self.file_id.marshal_json_to(enc)?;
        enc.push(',');
        if self.differs_only_in_dts_map {
            enc.push_str("[]");
        } else if self.differs_in_options {
            vec![self.signature.clone()].marshal_json_to(enc)?;
        } else {
            self.signature.marshal_json_to(enc)?;
        }
        enc.push(']');
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfoEmitSignature {
    // Go: incremental/buildInfo.go:389 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        if let Ok(file_id) = unmarshal_bytes::<BuildInfoFileId>(data) {
            *self = BuildInfoEmitSignature {
                file_id,
                ..Default::default()
            };
            return Ok(());
        }
        let Ok(file_id_and_signature) =
            unmarshal_bytes_with(data, |dec| unmarshal_slice(dec, unmarshal_any))
        else {
            return Err(json_error(format!(
                "invalid BuildInfoEmitSignature: {}",
                String::from_utf8_lossy(data)
            )));
        };
        let file_id_and_signature = file_id_and_signature.unwrap_or_default();
        if file_id_and_signature.len() != 2 {
            return Err(json_error(format!(
                "invalid BuildInfoEmitSignature: expected 2 elements, got {}",
                file_id_and_signature.len()
            )));
        }
        let file_id = match &file_id_and_signature[0] {
            // Go `BuildInfoFileId(id)` converts float64 to int by truncation.
            CompilerOptionsValue::Number(id) => BuildInfoFileId(*id as i32),
            other => {
                return Err(json_error(format!(
                    "invalid fileId in BuildInfoEmitSignature: expected float64, got {other:?}"
                )));
            }
        };
        let mut signature = String::new();
        let mut differs_only_in_dts_map = false;
        let mut differs_in_options = false;
        match &file_id_and_signature[1] {
            CompilerOptionsValue::String(signature_v) => signature = signature_v.clone(),
            CompilerOptionsValue::List(signature_list) => match signature_list.len() {
                0 => differs_only_in_dts_map = true,
                1 => match &signature_list[0] {
                    CompilerOptionsValue::String(sig) => {
                        signature = sig.clone();
                        differs_in_options = true;
                    }
                    other => {
                        return Err(json_error(format!(
                            "invalid signature in BuildInfoEmitSignature: expected string, got {other:?}"
                        )));
                    }
                },
                n => {
                    return Err(json_error(format!(
                        "invalid signature in BuildInfoEmitSignature: expected string or []string with 0 or 1 element, got {n} elements"
                    )));
                }
            },
            other => {
                return Err(json_error(format!(
                    "invalid signature in BuildInfoEmitSignature: expected string or []string, got {other:?}"
                )));
            }
        }
        *self = BuildInfoEmitSignature {
            file_id,
            signature,
            differs_only_in_dts_map,
            differs_in_options,
        };
        Ok(())
    }
}

// Go: incremental/buildInfo.go:444 BuildInfoResolvedRoot
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfoResolvedRoot {
    pub resolved: BuildInfoFileId,
    pub root: BuildInfoFileId,
}

impl MarshalerTo for BuildInfoResolvedRoot {
    // Go: incremental/buildInfo.go:449 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        vec![self.resolved, self.root].marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for BuildInfoResolvedRoot {
    // Go: incremental/buildInfo.go:453 UnmarshalJSON
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        if dec.peek_kind() == b'n' {
            dec.read_token()?;
            return Err(json_error("PORT: null element for a Go pointer"));
        }
        let data = dec.read_value()?;
        let Ok(resolved_and_root) = unmarshal_bytes_with(data, unmarshal_int_array::<2>) else {
            return Err(json_error(format!(
                "invalid BuildInfoResolvedRoot: {}",
                String::from_utf8_lossy(data)
            )));
        };
        *self = BuildInfoResolvedRoot {
            resolved: BuildInfoFileId(resolved_and_root[0]),
            root: BuildInfoFileId(resolved_and_root[1]),
        };
        Ok(())
    }
}

// Go: incremental/buildInfo.go:465 BuildInfo
// PORT: Go `*collections.OrderedMap[string, any]` is
// `Option<IndexMap<String, CompilerOptionsValue>>`. Go `any` values read
// from JSON are `Nil`, `Bool`, `Number`, `String`, `List` or `Map`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildInfo {
    pub version: String,

    // Common between incremental and tsc -b buildinfo for non incremental programs
    pub errors: bool,
    pub check_pending: bool,
    pub root: Option<Vec<BuildInfoRoot>>,
    pub package_jsons: Option<Vec<String>>,
    pub missing_package_jsons: Option<Vec<String>>,
    pub content_mapper_identities: Option<Vec<String>>,

    // IncrementalProgram info
    pub file_names: Option<Vec<String>>,
    pub file_infos: Option<Vec<BuildInfoFileInfo>>,
    pub file_ids_list: Option<Vec<Vec<BuildInfoFileId>>>,
    pub options: Option<IndexMap<String, CompilerOptionsValue>>,
    /// Effect-TS/tsgo patch 028: the Effect plugin options of the program
    /// (`EffectPluginOptions::to_value`), so a change to them invalidates
    /// the cached semantic diagnostics.
    pub effect: Option<CompilerOptionsValue>,
    pub referenced_map: Option<Vec<BuildInfoReferenceMapEntry>>,
    pub semantic_diagnostics_per_file: Option<Vec<BuildInfoSemanticDiagnostic>>,
    pub emit_diagnostics_per_file: Option<Vec<BuildInfoDiagnosticsOfFilePtr>>,
    pub change_file_set: Option<Vec<BuildInfoFileId>>,
    pub affected_files_pending_emit: Option<Vec<BuildInfoFilePendingEmit>>,
    pub latest_changed_dts_file: String, // Because this is only output file in the program, we dont need fileId to deduplicate name
    pub emit_signatures: Option<Vec<BuildInfoEmitSignature>>,
    pub resolved_root: Option<Vec<BuildInfoResolvedRoot>>,

    // NonIncrementalProgram info
    pub semantic_errors: bool,
}

impl MarshalerTo for BuildInfo {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("version", &self.version)?;
        w.bool_omitzero("errors", self.errors);
        w.bool_omitzero("checkPending", self.check_pending);
        w.slice_omitzero("root", self.root.as_ref())?;
        w.slice_omitzero("packageJsons", self.package_jsons.as_ref())?;
        w.slice_omitzero("missingPackageJsons", self.missing_package_jsons.as_ref())?;
        w.slice_omitzero(
            "contentMapperIdentities",
            self.content_mapper_identities.as_ref(),
        )?;
        w.slice_omitzero("fileNames", self.file_names.as_ref())?;
        w.slice_omitzero("fileInfos", self.file_infos.as_ref())?;
        w.slice_omitzero("fileIdsList", self.file_ids_list.as_ref())?;
        if let Some(options) = &self.options {
            // Go: collections/ordered_map.go:217 MarshalJSONTo
            let enc = w.name("options");
            enc.push('{');
            for (i, (k, v)) in options.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                k.marshal_json_to(enc)?;
                enc.push(':');
                marshal_any(enc, v)?;
            }
            enc.push('}');
        }
        if let Some(effect) = &self.effect {
            marshal_any(w.name("effect"), effect)?;
        }
        w.slice_omitzero("referencedMap", self.referenced_map.as_ref())?;
        w.slice_omitzero(
            "semanticDiagnosticsPerFile",
            self.semantic_diagnostics_per_file.as_ref(),
        )?;
        w.slice_omitzero(
            "emitDiagnosticsPerFile",
            self.emit_diagnostics_per_file.as_ref(),
        )?;
        w.slice_omitzero("changeFileSet", self.change_file_set.as_ref())?;
        w.slice_omitzero(
            "affectedFilesPendingEmit",
            self.affected_files_pending_emit.as_ref(),
        )?;
        w.string_omitzero("latestChangedDtsFile", &self.latest_changed_dts_file)?;
        w.slice_omitzero("emitSignatures", self.emit_signatures.as_ref())?;
        w.slice_omitzero("resolvedRoot", self.resolved_root.as_ref())?;
        w.bool_omitzero("semanticErrors", self.semantic_errors);
        w.end();
        Ok(())
    }
}

impl UnmarshalerFrom for BuildInfo {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        unmarshal_object(dec, |name, dec| {
            match name {
                "version" => self.version = unmarshal_string(dec)?,
                "errors" => self.errors = unmarshal_bool(dec)?,
                "checkPending" => self.check_pending = unmarshal_bool(dec)?,
                "root" => self.root = unmarshal_slice(dec, unmarshal_elem)?,
                "packageJsons" => self.package_jsons = unmarshal_slice(dec, unmarshal_string)?,
                "missingPackageJsons" => {
                    self.missing_package_jsons = unmarshal_slice(dec, unmarshal_string)?;
                }
                "contentMapperIdentities" => {
                    self.content_mapper_identities = unmarshal_slice(dec, unmarshal_string)?;
                }
                "fileNames" => self.file_names = unmarshal_slice(dec, unmarshal_string)?,
                "fileInfos" => self.file_infos = unmarshal_slice(dec, unmarshal_elem)?,
                "fileIdsList" => {
                    self.file_ids_list = unmarshal_slice(dec, |dec| {
                        Ok(
                            unmarshal_slice(dec, |dec| Ok(BuildInfoFileId(unmarshal_i32(dec)?)))?
                                .unwrap_or_default(),
                        )
                    })?;
                }
                "options" => self.options = unmarshal_options(dec)?,
                "effect" => self.effect = Some(unmarshal_any(dec)?),
                "referencedMap" => self.referenced_map = unmarshal_slice(dec, unmarshal_elem)?,
                "semanticDiagnosticsPerFile" => {
                    self.semantic_diagnostics_per_file = unmarshal_slice(dec, unmarshal_elem)?;
                }
                "emitDiagnosticsPerFile" => {
                    self.emit_diagnostics_per_file = unmarshal_slice(dec, unmarshal_elem)?;
                }
                "changeFileSet" => {
                    self.change_file_set =
                        unmarshal_slice(dec, |dec| Ok(BuildInfoFileId(unmarshal_i32(dec)?)))?;
                }
                "affectedFilesPendingEmit" => {
                    self.affected_files_pending_emit = unmarshal_slice(dec, unmarshal_elem)?;
                }
                "latestChangedDtsFile" => self.latest_changed_dts_file = unmarshal_string(dec)?,
                "emitSignatures" => self.emit_signatures = unmarshal_slice(dec, unmarshal_elem)?,
                "resolvedRoot" => self.resolved_root = unmarshal_slice(dec, unmarshal_elem)?,
                "semanticErrors" => self.semantic_errors = unmarshal_bool(dec)?,
                _ => return Ok(false),
            }
            Ok(true)
        })
    }
}

// Go: incremental/buildInfo.go:501 ContentMapperIdentities (tsgo#4712)
// ContentMapperIdentities returns the project's sorted mapper transform identities. A nil project means
// the compiler host has no configured content mappers.
// PORT: Go nil (the slice and the project) is `None`.
pub fn content_mapper_identities(
    project: Option<&dyn contentmapper::Project>,
) -> Result<Option<Vec<String>>, GoError> {
    match project {
        None => Ok(None),
        Some(project) => project.identities().map(Some),
    }
}

// Go: core/version.go Version, with Effect-TS/tsgo patch 021
/// The version that build info records, and must record to be reused.
/// `effect` is whether the program runs the Effect rules
/// (`rulerunner::enabled_options`). Effect-TS/tsgo adds
/// "+effect-tsgo.<version>" to every version its binary reports. tsc-rs adds
/// it only when the rules run, so plain build info stays tsgo's. Plain tsgo
/// and tsc-rs without the rules then see Effect build info as from another
/// version and check again: they do not read Effect diagnostics that plain
/// tsgo cannot print ("Unknown diagnostic message"). In the same way, tsc-rs
/// with the rules checks again after a build without them, for example a
/// standalone API build.
#[must_use]
pub fn build_info_version(effect: bool) -> std::borrow::Cow<'static, str> {
    if effect {
        format!(
            "{}+effect-tsgo.{}",
            version(),
            crate::effect::etscore::EFFECT_VERSION
        )
        .into()
    } else {
        version().into()
    }
}

impl BuildInfo {
    // Go: incremental/buildInfo.go:495 IsValidVersion
    // PORT: `effect` is whether the reading program runs the Effect rules
    // (`build_info_version`). Build info with Effect options and the plain
    // version is from tsc-rs before effectfix2 (Theo PR #4), which did not
    // add the suffix. effect-tsgo sees another version there and builds
    // again. A program without the rules does the same, so it does not reuse
    // those Effect diagnostics (a standalone API process).
    #[must_use]
    pub fn is_valid_version(&self, effect: bool) -> bool {
        self.version == build_info_version(effect) && (effect || self.effect.is_none())
    }

    // Go: incremental/buildInfo.go:510 ContentMapperIdentitiesMatch (tsgo#4712)
    // ContentMapperIdentitiesMatch reports whether the content mapper identities recorded in this build info
    // match the given current identities (as produced by ContentMapperIdentities).
    // PORT: Go `slices.Equal` treats nil and empty alike, as `unwrap_or_default` does.
    #[must_use]
    pub fn content_mapper_identities_match(&self, current: Option<&[String]>) -> bool {
        self.content_mapper_identities
            .as_deref()
            .unwrap_or_default()
            == current.unwrap_or_default()
    }

    // Go: incremental/buildInfo.go:496 IsIncremental
    // PORT: the Go nil receiver check is the caller's `Option`.
    #[must_use]
    pub fn is_incremental(&self) -> bool {
        self.file_names
            .as_ref()
            .is_some_and(|names| !names.is_empty())
    }

    // Go: incremental/buildInfo.go:502 fileName
    // PORT: an id out of range gives "", as in Go.
    #[must_use]
    pub fn file_name(&self, file_id: BuildInfoFileId) -> &str {
        let file_names = self.file_names.as_deref().unwrap_or_default();
        if file_id.0 < 1 || file_id.0 as usize > file_names.len() {
            return "";
        }
        &file_names[(file_id.0 - 1) as usize]
    }

    // Go: incremental/buildInfo.go:509 fileInfo
    // PORT: an id out of range gives `None` (Go nil).
    #[must_use]
    pub fn file_info(&self, file_id: BuildInfoFileId) -> Option<&BuildInfoFileInfo> {
        let file_infos = self.file_infos.as_deref().unwrap_or_default();
        if file_id.0 < 1 || file_id.0 as usize > file_infos.len() {
            return None;
        }
        Some(&file_infos[(file_id.0 - 1) as usize])
    }

    // Go: incremental/buildInfo.go:508 GetCompilerOptions
    #[must_use]
    pub fn get_compiler_options(&self, build_info_directory: &str) -> CompilerOptions {
        let mut options = CompilerOptions::default();
        for (option, value) in self.options.iter().flatten() {
            if !build_info_directory.is_empty() {
                let (result, ok) = convert_option_to_absolute_path(
                    option,
                    value,
                    &COMMAND_LINE_COMPILER_OPTIONS_MAP,
                    build_info_directory,
                );
                if ok {
                    parse_compiler_options(option, result, &mut options);
                    continue;
                }
            }
            parse_compiler_options(option, value.clone(), &mut options);
        }
        // Effect-TS/tsgo patch 028.
        options.effect = self
            .effect
            .as_ref()
            .and_then(crate::effect::etscore::EffectPluginOptions::from_value)
            .map(std::sync::Arc::new);
        options
    }

    // Go: incremental/buildInfo.go:524 IsEmitPending
    #[must_use]
    pub fn is_emit_pending(
        &self,
        resolved: &ParsedCommandLine,
        build_info_directory: &str,
    ) -> bool {
        // Some of the emit files like source map or dts etc are not yet done
        if !resolved.compiler_options().no_emit.is_true()
            || resolved.compiler_options().get_emit_declarations()
        {
            let mut pending_emit = get_pending_emit_kind_with_options(
                resolved.compiler_options(),
                &self.get_compiler_options(build_info_directory),
            );
            if resolved.compiler_options().no_emit.is_true() {
                pending_emit &= FileEmitKind::DTS_ERRORS;
            }
            return !pending_emit.is_empty();
        }
        false
    }

    // Go: incremental/buildInfo.go:544 GetPackageJsons
    pub fn get_package_jsons(&self, build_info_directory: &str) -> impl Iterator<Item = String> {
        get_normalized_paths(self.package_jsons.as_deref(), build_info_directory)
    }

    // Go: incremental/buildInfo.go:548 GetMissingPackageJsons
    pub fn get_missing_package_jsons(
        &self,
        build_info_directory: &str,
    ) -> impl Iterator<Item = String> {
        get_normalized_paths(self.missing_package_jsons.as_deref(), build_info_directory)
    }

    // Go: incremental/buildInfo.go:562 GetBuildInfoRootInfoReader
    #[must_use]
    pub fn get_build_info_root_info_reader(
        &self,
        build_info_directory: &str,
        compare_path_options: &ComparePathsOptions,
    ) -> BuildInfoRootInfoReader {
        let file_count = self.file_names.as_ref().map_or(0, Vec::len);
        let mut resolved_root_file_infos: FxHashMap<Path, BuildInfoFileInfo> =
            FxHashMap::with_capacity_and_hasher(file_count, Default::default());
        // Roots of the File
        let mut root_to_resolved: FxIndexMap<Path, Path> =
            FxIndexMap::with_capacity_and_hasher(file_count, Default::default());
        let mut resolved_to_root: FxHashMap<Path, Path> = FxHashMap::with_capacity_and_hasher(
            self.resolved_root.as_ref().map_or(0, Vec::len),
            Default::default(),
        );
        let to_path_fn = |file_name: &str| -> Path {
            to_path(
                file_name,
                build_info_directory,
                compare_path_options.use_case_sensitive_file_names,
            )
        };

        // Create map from resolvedRoot to Root
        for resolved in self.resolved_root.iter().flatten() {
            let resolved_root = self.file_name(resolved.resolved);
            let root = self.file_name(resolved.root);
            if !resolved_root.is_empty() && !root.is_empty() {
                resolved_to_root.insert(to_path_fn(resolved_root), to_path_fn(root));
            }
        }

        let mut add_root = |resolved_root: &str, file_info: Option<&BuildInfoFileInfo>| {
            if resolved_root.is_empty() {
                return;
            }
            let resolved_root_path = to_path_fn(resolved_root);
            if let Some(root_path) = resolved_to_root.get(&resolved_root_path) {
                root_to_resolved.insert(root_path.clone(), resolved_root_path.clone());
            } else {
                root_to_resolved.insert(resolved_root_path.clone(), resolved_root_path.clone());
            }
            if let Some(file_info) = file_info {
                resolved_root_file_infos.insert(resolved_root_path, file_info.clone());
            }
        };

        for root in self.root.iter().flatten() {
            if !root.non_incremental.is_empty() {
                add_root(&root.non_incremental, None);
            } else if root.end.0 == 0 {
                add_root(self.file_name(root.start), self.file_info(root.start));
            } else {
                for i in root.start.0..=root.end.0 {
                    let i = BuildInfoFileId(i);
                    add_root(self.file_name(i), self.file_info(i));
                }
            }
        }

        BuildInfoRootInfoReader {
            resolved_root_file_infos,
            root_to_resolved,
        }
    }
}

// Go: incremental/buildInfo.go:498 IsBuildInfoFileNameDefaultLibrary
#[must_use]
pub fn is_build_info_file_name_default_library(file_name: &str) -> bool {
    !path_is_relative(file_name) && !path_is_absolute(file_name)
}

// Go: incremental/buildInfo.go:552 getNormalizedPaths
// PORT: both lifetimes stay separate (edition 2024 captures both) so the
// public getters can return this opaque type.
fn get_normalized_paths(
    paths: Option<&[String]>,
    build_info_directory: &str,
) -> impl Iterator<Item = String> {
    paths
        .unwrap_or_default()
        .iter()
        .map(move |path| get_normalized_absolute_path(path, build_info_directory))
}

// Go: incremental/buildInfo.go:613 BuildInfoRootInfoReader
#[derive(Clone, Debug, Default)]
pub struct BuildInfoRootInfoReader {
    pub resolved_root_file_infos: FxHashMap<Path, BuildInfoFileInfo>,
    pub root_to_resolved: FxIndexMap<Path, Path>,
}

impl BuildInfoRootInfoReader {
    // Go: incremental/buildInfo.go:584 GetBuildInfoFileInfo
    // PORT: Go nil info is `None`; a missing root gives `(None, "")`.
    #[must_use]
    pub fn get_build_info_file_info(
        &self,
        input_file_path: &Path,
    ) -> (Option<&BuildInfoFileInfo>, Path) {
        if let Some(info) = self.resolved_root_file_infos.get(input_file_path) {
            return (Some(info), input_file_path.clone());
        }
        if let Some(resolved) = self.root_to_resolved.get(input_file_path) {
            return (
                self.resolved_root_file_infos.get(resolved),
                resolved.clone(),
            );
        }
        (None, Path::default())
    }

    // Go: incremental/buildInfo.go:594 Roots
    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        self.root_to_resolved.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every buildinfo shape in one file: the three root forms, the three
    // file info forms, options with a map and a list, nested diagnostics,
    // the three pending emit forms and the four emit signature forms.
    const SAMPLE: &str = r#"{"version":"FakeTSVersion","errors":true,"checkPending":true,"root":[[2,3],4,"../x.ts"],"fileNames":["lib.d.ts","./a.ts","./b.ts","./c.ts","./d.ts"],"fileInfos":["abc",{"version":"v","signature":"s","affectsGlobalScope":true,"impliedNodeFormat":1},{"version":"v2","noSignature":true,"impliedNodeFormat":99},{"version":"v3"},"e"],"fileIdsList":[[2],[]],"options":{"composite":true,"module":199,"outDir":"./dist","paths":{"a":["b"]},"lib":["lib.es2022.d.ts"]},"referencedMap":[[3,1]],"semanticDiagnosticsPerFile":[1,[2,[{"pos":1,"end":2,"code":2322,"category":1,"messageKey":"k","messageArgs":["a\"\n"],"messageChain":[{"code":1,"messageKey":"m","repopulateInfo":{"kind":1,"moduleReference":"x","mode":99}}],"relatedInformation":[{"file":3,"pos":3,"end":4,"code":2728,"category":3,"messageKey":"r"}]}]]],"emitDiagnosticsPerFile":[[2,[{"end":1,"code":9010,"category":1,"messageKey":"i","skippedOnNoEmit":true}]]],"changeFileSet":[3],"affectedFilesPendingEmit":[2,[3],[4,17]],"latestChangedDtsFile":"./dist/a.d.ts","emitSignatures":[2,[3,"sig"],[4,[]],[5,["s2"]]],"resolvedRoot":[[2,5]],"semanticErrors":true}"#;

    #[test]
    fn round_trips_every_shape() {
        let mut info = BuildInfo::default();
        json_unmarshal(SAMPLE.as_bytes(), &mut info, &[]).unwrap();
        assert_eq!(json_marshal(&info, &[]).unwrap(), SAMPLE);

        let root = info.root.as_ref().unwrap();
        assert_eq!(
            (root[0].start, root[0].end),
            (BuildInfoFileId(2), BuildInfoFileId(3))
        );
        assert_eq!(root[2].non_incremental, "../x.ts");
        assert!(info.file_info(BuildInfoFileId(1)).unwrap().has_signature());
        assert!(
            info.file_info(BuildInfoFileId(3))
                .unwrap()
                .no_signature
                .is_some()
        );
        assert_eq!(
            info.file_info(BuildInfoFileId(4))
                .unwrap()
                .get_file_info()
                .signature,
            "v3"
        );
        let pending = info.affected_files_pending_emit.as_ref().unwrap();
        assert_eq!(pending[1].emit_kind, FileEmitKind::DTS);
        let signatures = info.emit_signatures.as_ref().unwrap();
        assert!(signatures[2].differs_only_in_dts_map);
        assert!(signatures[3].differs_in_options);
    }

    // The one-pass object decode (`BuildInfoFileInfoMembers`) gives what Go's
    // three decodes give, also where it fails and the Go path runs.
    #[test]
    fn file_info_object_decode_matches_go_order() {
        fn go_order(data: &[u8]) -> Option<BuildInfoFileInfo> {
            if let Ok(signature) = unmarshal_bytes::<String>(data) {
                return Some(BuildInfoFileInfo {
                    signature,
                    ..Default::default()
                });
            }
            if let Ok(no_signature) = unmarshal_bytes::<BuildInfoFileInfoNoSignature>(data)
                && no_signature.no_signature
            {
                return Some(BuildInfoFileInfo {
                    no_signature: Some(no_signature),
                    ..Default::default()
                });
            }
            let file_info = unmarshal_bytes::<BuildInfoFileInfoWithSignature>(data).ok()?;
            Some(BuildInfoFileInfo {
                file_info: Some(file_info),
                ..Default::default()
            })
        }
        for data in [
            r#"{"version":"v","signature":"s","affectsGlobalScope":true,"impliedNodeFormat":1}"#,
            r#"{"version":"v","noSignature":true,"impliedNodeFormat":99}"#,
            r#"{"version":"v","noSignature":false}"#,
            r#"{"version":"v","noSignature":true,"signature":"s"}"#,
            r#"{"version":"v","signature":1,"noSignature":true}"#,
            r#"{"version":"v","noSignature":"x","signature":"s"}"#,
            r#"{"version":null,"extra":[1,{"a":2}],"impliedNodeFormat":null}"#,
            r#"{"version":"v","version":"w"}"#,
            r#"{"version":1}"#,
            r#"{}"#,
        ] {
            let mut decoded = BuildInfoFileInfo::default();
            let got = json_unmarshal(data.as_bytes(), &mut decoded, &[])
                .ok()
                .map(|()| decoded);
            assert_eq!(got, go_order(data.as_bytes()), "{data}");
        }
    }

    #[test]
    fn rejects_invalid_shapes() {
        let mut info = BuildInfo::default();
        assert!(json_unmarshal(br#"{"root":[[1,2,3]]}"#, &mut info, &[]).is_err());
        assert!(json_unmarshal(br#"{"root":[1.5]}"#, &mut info, &[]).is_err());
        assert!(json_unmarshal(br#"{"emitSignatures":[[1,[1]]]}"#, &mut info, &[]).is_err());
    }
}
