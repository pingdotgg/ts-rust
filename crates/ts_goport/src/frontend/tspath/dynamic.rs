//! Port of tspath/dynamic.go (ts#64544).
//!
//! A dynamic file name is the file name of an editor document that is not a
//! `file:` URI. `lsproto` writes it as
//! `^/~ts-uri~/<scheme>/<authority>/<path>`. Path segments that a file name
//! cannot hold as they are (empty, `.`, `..`, a backslash, or text that looks
//! like an escape) are written as `~ts-uri-escape~<hex>~<extension>`, so
//! normalization never changes them. Module specifiers use
//! `~ts-uri-spec~<hex>~` for the same segments.

use std::borrow::Cow;

use super::extension::get_declaration_file_extension;
use super::path::{
    get_any_extension_from_path, get_root_length, has_trailing_directory_separator,
    path_is_absolute, remove_trailing_directory_separator,
};
use crate::frontend::prelude::*;

// Go: tspath/dynamic.go:10 DynamicURIFileNamePrefix
pub const DYNAMIC_URI_FILE_NAME_PREFIX: &str = "^/~ts-uri~/";
// Go: tspath/dynamic.go:11 dynamicURIPathSegmentEscapePrefix
const DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX: &str = "~ts-uri-escape~";
// Go: tspath/dynamic.go:12 dynamicURIModuleSpecifierEscapePrefix
const DYNAMIC_URI_MODULE_SPECIFIER_ESCAPE_PREFIX: &str = "~ts-uri-spec~";
// Go: tspath/dynamic.go:13 dynamicURINoPathEscapePrefix
const DYNAMIC_URI_NO_PATH_ESCAPE_PREFIX: &str = "~ts-uri-no-path~";

// Go: tspath/dynamic.go:16 IsEncodedDynamicFileName
pub fn is_encoded_dynamic_file_name(path: &str) -> bool {
    path.starts_with(DYNAMIC_URI_FILE_NAME_PREFIX)
}

// Go: tspath/dynamic.go:20 canonicalDynamicURIPath
// A dynamic root ("^/~ts-uri~/<scheme>/<authority>") always ends with "/".
pub fn canonical_dynamic_uri_path(path: &str) -> Cow<'_, str> {
    if is_encoded_dynamic_file_name(path)
        && get_root_length(path) == path.len()
        && !has_trailing_directory_separator(path)
    {
        return Cow::Owned(format!("{path}/"));
    }
    Cow::Borrowed(path)
}

// Go: tspath/dynamic.go:29 EncodeDynamicURIPath
pub fn encode_dynamic_uri_path(path: &str) -> String {
    encode_dynamic_uri_path_root_aware(path, true)
}

// Go: tspath/dynamic.go:33 EncodeDynamicURIPathWithSuffix
pub fn encode_dynamic_uri_path_with_suffix(path: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        return encode_dynamic_uri_path(path);
    }
    let slash = path.rfind('/');
    let mut before = String::new();
    if let Some(slash) = slash {
        before = encode_dynamic_uri_directory_path(&path[..slash]) + "/";
    }
    let last = match slash {
        Some(slash) => &path[slash + 1..],
        None => path,
    };
    before + &force_encode_dynamic_uri_path_segment_with_suffix(last, suffix)
}

// Go: tspath/dynamic.go:45 EncodeDynamicURIDirectoryPath
pub fn encode_dynamic_uri_directory_path(path: &str) -> String {
    encode_dynamic_uri_path_root_aware(path, false)
}

// Go: tspath/dynamic.go:49 encodeDynamicURIPathRootAware
fn encode_dynamic_uri_path_root_aware(path: &str, preserve_final_extension: bool) -> String {
    let encoded = encode_dynamic_uri_path_segments(path, preserve_final_extension);
    if !path_is_absolute(&encoded) {
        return encoded;
    }
    match encoded.split_once('/') {
        None => force_encode_dynamic_uri_path_segment(&encoded, preserve_final_extension),
        Some((first, rest)) => force_encode_dynamic_uri_path_segment(first, false) + "/" + rest,
    }
}

// Go: tspath/dynamic.go:61 EncodeDynamicRelativeURIPath
pub fn encode_dynamic_relative_uri_path(path: &str) -> String {
    encode_dynamic_relative_uri_path_worker(path, true)
}

// Go: tspath/dynamic.go:65 EncodeDynamicRelativeURIDirectoryPath
pub fn encode_dynamic_relative_uri_directory_path(path: &str) -> String {
    encode_dynamic_relative_uri_path_worker(path, false)
}

// Go: tspath/dynamic.go:69 encodeDynamicRelativeURIPath
fn encode_dynamic_relative_uri_path_worker(path: &str, preserve_final_extension: bool) -> String {
    encode_dynamic_uri_path_root_aware(path, preserve_final_extension)
}

// Go: tspath/dynamic.go:73 encodeDynamicURIPath
// PORT: renamed; Go has both `EncodeDynamicURIPath` and this unexported one.
fn encode_dynamic_uri_path_segments(path: &str, preserve_final_extension: bool) -> String {
    if !dynamic_uri_path_needs_encoding(path) {
        return path.to_string();
    }

    let mut result =
        String::with_capacity(path.len() + DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX.len());
    let mut path = path;
    loop {
        let (segment, rest) = match path.split_once('/') {
            Some((segment, rest)) => (segment, Some(rest)),
            None => (path, None),
        };
        result.push_str(&encode_dynamic_uri_path_segment(
            segment,
            preserve_final_extension && rest.is_none(),
        ));
        let Some(rest) = rest else {
            return result;
        };
        result.push('/');
        path = rest;
    }
}

// Go: tspath/dynamic.go:95 EncodeDynamicModuleSpecifier
pub fn encode_dynamic_module_specifier(specifier: &str) -> String {
    encode_dynamic_module_specifier_worker(specifier, true)
}

// Go: tspath/dynamic.go:99 EncodeDynamicDirectorySpecifier
pub fn encode_dynamic_directory_specifier(specifier: &str) -> String {
    encode_dynamic_module_specifier_worker(specifier, false)
}

// Go: tspath/dynamic.go:103 encodeDynamicModuleSpecifier
fn encode_dynamic_module_specifier_worker(
    specifier: &str,
    preserve_final_extension: bool,
) -> String {
    if !specifier.contains(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX)
        && !specifier.contains(DYNAMIC_URI_MODULE_SPECIFIER_ESCAPE_PREFIX)
    {
        return specifier.to_string();
    }

    let mut result =
        String::with_capacity(specifier.len() + DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX.len());
    let mut specifier = specifier;
    loop {
        let Some(separator) = specifier.find(['/', '\\']) else {
            result.push_str(&encode_dynamic_module_specifier_segment(
                specifier,
                preserve_final_extension,
            ));
            return result;
        };
        result.push_str(&encode_dynamic_module_specifier_segment(
            &specifier[..separator],
            false,
        ));
        result.push_str(&specifier[separator..separator + 1]);
        specifier = &specifier[separator + 1..];
    }
}

// Go: tspath/dynamic.go:123 encodeDynamicModuleSpecifierSegment
fn encode_dynamic_module_specifier_segment(segment: &str, preserve_extension: bool) -> String {
    if let Some(encoded) = segment.strip_prefix(DYNAMIC_URI_MODULE_SPECIFIER_ESCAPE_PREFIX) {
        let physical = format!("{DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX}{encoded}");
        if decode_dynamic_uri_path_segment(&physical) != physical {
            return physical;
        }
    }
    if segment.starts_with(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX) {
        return force_encode_dynamic_uri_path_segment(segment, preserve_extension);
    }
    segment.to_string()
}

// Go: tspath/dynamic.go:136 DynamicURIPathToModuleSpecifier
pub fn dynamic_uri_path_to_module_specifier(path: &str) -> String {
    if !path.contains(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX) {
        return path.to_string();
    }
    path.split('/')
        .map(|segment| {
            if segment.starts_with(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX)
                && decode_dynamic_uri_path_segment(segment) != segment
            {
                format!(
                    "{DYNAMIC_URI_MODULE_SPECIFIER_ESCAPE_PREFIX}{}",
                    &segment[DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX.len()..]
                )
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

// Go: tspath/dynamic.go:151 EncodeDynamicLogicalModuleSpecifier
pub fn encode_dynamic_logical_module_specifier(specifier: &str) -> String {
    if specifier.is_empty() {
        return String::new();
    }
    let trailing_separator = has_trailing_directory_separator(specifier);
    let specifier = if trailing_separator {
        remove_trailing_directory_separator(specifier)
    } else {
        specifier
    };
    let mut encoded =
        dynamic_uri_path_to_module_specifier(&encode_dynamic_relative_uri_path(specifier));
    if trailing_separator {
        encoded.push('/');
    }
    encoded
}

// Go: tspath/dynamic.go:166 dynamicURIPathNeedsEncoding
fn dynamic_uri_path_needs_encoding(path: &str) -> bool {
    path.split('/').any(dynamic_uri_path_segment_needs_encoding)
}

// Go: tspath/dynamic.go:179 encodeDynamicURIPathSegment
fn encode_dynamic_uri_path_segment(segment: &str, preserve_extension: bool) -> String {
    if dynamic_uri_path_segment_needs_encoding(segment) {
        return force_encode_dynamic_uri_path_segment(segment, preserve_extension);
    }
    segment.to_string()
}

// Go: tspath/dynamic.go:186 dynamicURIPathSegmentNeedsEncoding
fn dynamic_uri_path_segment_needs_encoding(segment: &str) -> bool {
    segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.starts_with(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX)
        || segment.starts_with(DYNAMIC_URI_MODULE_SPECIFIER_ESCAPE_PREFIX)
        || segment.starts_with(DYNAMIC_URI_NO_PATH_ESCAPE_PREFIX)
        || segment.contains('\\')
}

// Go: tspath/dynamic.go:196 EncodeDynamicURINoPath
pub fn encode_dynamic_uri_no_path(suffix: &str) -> String {
    format!(
        "{DYNAMIC_URI_NO_PATH_ESCAPE_PREFIX}{}~",
        hex_encode(&go_string_bytes(suffix))
    )
}

// Go: tspath/dynamic.go:200 DecodeDynamicURINoPath
pub fn decode_dynamic_uri_no_path(path: &str) -> Option<String> {
    let encoded = path.strip_prefix(DYNAMIC_URI_NO_PATH_ESCAPE_PREFIX)?;
    let (encoded, rest) = encoded.split_once('~')?;
    if !rest.is_empty() {
        return None;
    }
    hex_decode_utf8(encoded)
}

// Go: tspath/dynamic.go:91 ForceEncodeDynamicURIPathSegment
// Go: tspath/dynamic.go:216 forceEncodeDynamicURIPathSegment
// PORT: Go's exported wrapper and its unexported body are one function.
pub fn force_encode_dynamic_uri_path_segment(segment: &str, preserve_extension: bool) -> String {
    let (segment, extension) = if preserve_extension && segment != "." && segment != ".." {
        split_dynamic_uri_file_extension(segment)
    } else {
        (segment, "")
    };
    format!(
        "{DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX}{}~{extension}",
        hex_encode(&go_string_bytes(segment))
    )
}

// Go: tspath/dynamic.go:224 forceEncodeDynamicURIPathSegmentWithSuffix
fn force_encode_dynamic_uri_path_segment_with_suffix(segment: &str, suffix: &str) -> String {
    let (segment, extension) = split_dynamic_uri_file_extension(segment);
    let mut payload = go_string_bytes(segment).into_owned();
    payload.push(0);
    payload.extend_from_slice(&go_string_bytes(suffix));
    format!(
        "{DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX}{}~{extension}",
        hex_encode(&payload)
    )
}

// Go: tspath/dynamic.go:230 splitDynamicURIFileExtension
fn split_dynamic_uri_file_extension(segment: &str) -> (&str, &str) {
    let base_name = match segment.rfind('\\') {
        Some(i) => &segment[i + 1..],
        None => segment,
    };
    let mut extension = get_declaration_file_extension(base_name);
    if extension.is_empty() {
        extension = get_any_extension_from_path(base_name, &[], true);
    }
    if extension.is_empty() {
        return (segment, "");
    }
    // The extension is a suffix of `base_name`, which ends `segment`.
    let split = segment.len() - extension.len();
    (&segment[..split], &segment[split..])
}

// Go: tspath/dynamic.go:242 DecodeDynamicURIPath
pub fn decode_dynamic_uri_path(path: &str) -> String {
    if !path.contains(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX) {
        return path.to_string();
    }
    path.split('/')
        .map(decode_dynamic_uri_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

// Go: tspath/dynamic.go:253 TryDecodeDynamicURIPath
pub fn try_decode_dynamic_uri_path(path: &str) -> Option<String> {
    let segments = path
        .split('/')
        .map(try_decode_dynamic_uri_path_segment)
        .collect::<Option<Vec<_>>>()?;
    Some(segments.join("/"))
}

// Go: tspath/dynamic.go:265 DecodeDynamicURIPathForDisk
// The decoded relative path, or `None` when a segment cannot be a disk path
// segment (empty, `.`, `..` or with a backslash) or the path is absolute.
pub fn decode_dynamic_uri_path_for_disk(path: &str) -> Option<String> {
    let decoded = decode_dynamic_uri_path(path);
    if path_is_absolute(&decoded) {
        return None;
    }
    let mut remaining = decoded.as_str();
    loop {
        let (segment, rest) = match remaining.split_once('/') {
            Some((segment, rest)) => (segment, Some(rest)),
            None => (remaining, None),
        };
        if segment.is_empty() {
            if rest.is_none() {
                return Some(decoded);
            }
            return None;
        }
        if segment == "." || segment == ".." || segment.contains('\\') {
            return None;
        }
        let Some(rest) = rest else {
            return Some(decoded);
        };
        remaining = rest;
    }
}

// Go: tspath/dynamic.go:289 DecodeDynamicURIPathSegment
pub fn decode_dynamic_uri_path_segment(segment: &str) -> String {
    try_decode_dynamic_uri_path_segment(segment).unwrap_or_else(|| segment.to_string())
}

// Go: tspath/dynamic.go:297 TryDecodeDynamicURIPathSegment
pub fn try_decode_dynamic_uri_path_segment(segment: &str) -> Option<String> {
    let Some(encoded) = segment.strip_prefix(DYNAMIC_URI_PATH_SEGMENT_ESCAPE_PREFIX) else {
        if dynamic_uri_path_segment_needs_encoding(segment) {
            return None;
        }
        return Some(segment.to_string());
    };
    let (encoded, extension) = encoded.split_once('~')?;
    let decoded = hex_decode_utf8(encoded)?;
    if let Some((base, suffix)) = decoded.split_once('\0') {
        return Some(format!("{base}{extension}{suffix}"));
    }
    Some(decoded + extension)
}

// PORT: Go `hex.EncodeToString` (lower-case digits).
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

// PORT: Go `hex.DecodeString` (either digit case; an odd length or another
// byte is an error) followed by `utf8.Valid`.
fn hex_decode_utf8(encoded: &str) -> Option<String> {
    fn digit(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = encoded.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        decoded.push((digit(pair[0])? << 4) | digit(pair[1])?);
    }
    String::from_utf8(decoded).ok()
}
