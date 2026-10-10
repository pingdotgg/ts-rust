//! Port of Go `sourcemap/source_mapper.go`.

use crate::prelude::*;

use crate::frontend::json::{JsonDecoder, JsonError, JsonToken, UnmarshalerFrom, json_unmarshal};
use crate::frontend::scanner::scanner_p1::utf8_decode_rune_in_string;
use crate::frontend::tspath;
use crate::gostd;
use crate::sourcemap::decoder::{MISSING_SOURCE, decode_mappings};
use crate::sourcemap::generator::{NameIndex, RawSourceMap, SourceIndex};
use crate::sourcemap::lineinfo::ECMALineInfo;

// Go: sourcemap/source_mapper.go:16 Host
// PORT: Go `*ECMALineInfo` results are `Option<Rc<ECMALineInfo>>` (nil is
// `None`), as in `ls::Host`.
pub trait Host {
    fn use_case_sensitive_file_names(&self) -> bool;
    fn get_ecma_line_info(&self, file_name: &str) -> Option<Rc<ECMALineInfo>>;
    fn read_file(&self, file_name: &str) -> (String, bool);
}

// Go: sourcemap/source_mapper.go:23 MappedPosition
// Similar to `Mapping`, but position-based.
// PORT: Go `int` positions are `i32`. Go keeps `[]*MappedPosition`; the
// values are never shared after they are made, so they are stored by value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MappedPosition {
    generated_position: i32,
    source_position: i32,
    source_index: SourceIndex,
    name_index: NameIndex,
}

// Go: sourcemap/source_mapper.go:30 missingPosition
const MISSING_POSITION: i32 = -1;

impl MappedPosition {
    // Go: sourcemap/source_mapper.go:34 isSourceMappedPosition
    fn is_source_mapped_position(&self) -> bool {
        self.source_index != MISSING_SOURCE && self.source_position != MISSING_POSITION
    }
}

// Go: sourcemap/source_mapper.go:39 SourceMappedPosition
pub type SourceMappedPosition = MappedPosition;

// Go: sourcemap/source_mapper.go:41 compareSourcePositions
fn compare_source_positions(left: &SourceMappedPosition, right: &SourceMappedPosition) -> i32 {
    left.source_position.cmp(&right.source_position) as i32
}

// Go: sourcemap/source_mapper.go:46 DocumentPositionMapper
// Maps source positions to generated positions and vice versa.
// PORT: ts#64544 state. #64159 types the paths (`RootedFilePath`,
// `PathKey`, `CaseSensitivity`); that part waits for bump D wave 2b.
#[derive(Clone, Debug, Default)]
pub struct DocumentPositionMapper {
    use_case_sensitive_file_names: bool,

    source_file_absolute_paths: Vec<String>,
    source_mappings_by_path: FxHashMap<String, Vec<SourceMappedPosition>>,
    generated_absolute_file_path: String,

    generated_mappings: Vec<MappedPosition>,
    source_mappings: FxHashMap<SourceIndex, Vec<SourceMappedPosition>>,
}

// Go: sourcemap/source_mapper.go:56 createDocumentPositionMapper
// PORT: ts#64544 state (see `DocumentPositionMapper`). Go `sourceRootField
// *string` is `Option<&str>` (nil is `None`).
fn create_document_position_mapper(
    host: &dyn Host,
    source_map: &RawSourceMap,
    source_root_field: Option<&str>,
    null_sources: &[bool],
    map_path: &str,
) -> Rc<DocumentPositionMapper> {
    let map_directory = tspath::get_directory_path(map_path);
    let mut source_url_prefix = String::new();
    // ECMA-426 prefixes an explicit empty sourceRoot with "/", but TypeScript and
    // established consumers treat it as absent. Preserve that compatibility.
    if let Some(source_root) = source_root_field.filter(|root| !root.is_empty()) {
        source_url_prefix.push_str(source_root);
        if !source_url_prefix.ends_with('/') {
            source_url_prefix.push('/');
        }
    }
    let generated_absolute_file_path =
        tspath::get_normalized_absolute_path(&source_map.file, &map_directory);
    // Go `copy(unmappedSources, nullSources)`: at most `len(sources)` entries.
    let mut unmapped_sources = vec![false; source_map.sources.len()];
    let copied = unmapped_sources.len().min(null_sources.len());
    unmapped_sources[..copied].copy_from_slice(&null_sources[..copied]);
    let mut source_file_absolute_paths = vec![String::new(); source_map.sources.len()];
    for (i, source) in source_map.sources.iter().enumerate() {
        if unmapped_sources[i] {
            continue;
        }
        let source_with_prefix = format!("{source_url_prefix}{source}");
        let resolved = if source_with_prefix.is_empty() {
            map_path.to_string()
        } else {
            tspath::get_normalized_absolute_path(&source_with_prefix, &map_directory)
        };
        source_file_absolute_paths[i] = resolved;
    }
    let use_case_sensitive_file_names = host.use_case_sensitive_file_names();
    // PORT: Go map of index lists; each list is in index order, so map
    // iteration order does not reach the result.
    let mut source_to_source_index_map: FxHashMap<String, Vec<SourceIndex>> =
        FxHashMap::with_capacity_and_hasher(source_file_absolute_paths.len(), Default::default());
    for (i, source) in source_file_absolute_paths.iter().enumerate() {
        if unmapped_sources[i] {
            continue;
        }
        let key = tspath::get_canonical_file_name(source, use_case_sensitive_file_names);
        source_to_source_index_map
            .entry(key)
            .or_default()
            .push(i as SourceIndex);
    }

    let mut decoded_mappings: Vec<MappedPosition> = Vec::new();
    let mut generated_mappings: Vec<MappedPosition>;
    // PORT: Go map; each list is sorted on its own, so iteration order does
    // not reach the result.
    let mut source_mappings: FxHashMap<SourceIndex, Vec<SourceMappedPosition>> =
        FxHashMap::default();

    // getDecodedMappings()
    let mut decoder = decode_mappings(&source_map.mappings);
    for mapping in decoder.values() {
        // processMapping()
        let mut generated_position = -1;
        let line_info = host.get_ecma_line_info(&generated_absolute_file_path);
        if let Some(line_info) = &line_info {
            generated_position = compute_position_of_line_and_utf16_character(
                &line_info.line_starts,
                mapping.generated_line,
                mapping.generated_character,
                &line_info.text,
                true, /*allowEdits*/
            );
        }

        let mut source_position = -1;
        if mapping.is_source_mapping() {
            let source_index = mapping.source_index as isize;
            if source_index >= 0
                && (source_index as usize) < source_file_absolute_paths.len()
                && !unmapped_sources[source_index as usize]
            {
                let line_info =
                    host.get_ecma_line_info(&source_file_absolute_paths[source_index as usize]);
                if let Some(line_info) = &line_info {
                    let pos = compute_position_of_line_and_utf16_character(
                        &line_info.line_starts,
                        mapping.source_line,
                        mapping.source_character,
                        &line_info.text,
                        true, /*allowEdits*/
                    );
                    source_position = pos;
                }
            }
        }

        decoded_mappings.push(MappedPosition {
            generated_position,
            source_index: mapping.source_index,
            source_position,
            name_index: mapping.name_index,
        });
    }
    if decoder.error().is_some() {
        decoded_mappings = Vec::new();
    }

    // getSourceMappings()
    for mapping in &decoded_mappings {
        if !mapping.is_source_mapped_position() {
            continue;
        }
        let source_index = mapping.source_index;
        let list = source_mappings.entry(source_index).or_default();
        list.push(SourceMappedPosition {
            generated_position: mapping.generated_position,
            source_index,
            source_position: mapping.source_position,
            name_index: mapping.name_index,
        });
    }
    for list in source_mappings.values_mut() {
        gostd::slices::sort_func(
            list.as_mut_slice(),
            |a: &SourceMappedPosition, b: &SourceMappedPosition| {
                go_assert!(
                    a.source_index == b.source_index,
                    "All source mappings should have the same source index"
                );
                compare_source_positions(a, b)
            },
        );
        *list = deduplicate_sorted(std::mem::take(list), |a, b| {
            a.generated_position == b.generated_position
                && a.source_index == b.source_index
                && a.source_position == b.source_position
        });
    }
    let mut source_mappings_by_path: FxHashMap<String, Vec<SourceMappedPosition>> =
        FxHashMap::with_capacity_and_hasher(source_to_source_index_map.len(), Default::default());
    for (path, source_indices) in source_to_source_index_map {
        let mut mappings: Vec<SourceMappedPosition> = Vec::new();
        for source_index in source_indices {
            // Go: a missing key reads a nil slice.
            if let Some(list) = source_mappings.get(&source_index) {
                mappings.extend_from_slice(list);
            }
        }
        gostd::slices::sort_func(mappings.as_mut_slice(), compare_source_positions);
        source_mappings_by_path.insert(path, mappings);
    }

    // getGeneratedMappings()
    generated_mappings = decoded_mappings;
    gostd::slices::sort_func(
        generated_mappings.as_mut_slice(),
        |a: &MappedPosition, b: &MappedPosition| a.generated_position - b.generated_position,
    );
    generated_mappings = deduplicate_sorted(generated_mappings, |a, b| {
        a.generated_position == b.generated_position
            && a.source_index == b.source_index
            && a.source_position == b.source_position
    });

    Rc::new(DocumentPositionMapper {
        use_case_sensitive_file_names,
        source_file_absolute_paths,
        source_mappings_by_path,
        generated_absolute_file_path,
        generated_mappings,
        source_mappings,
    })
}

// Go: sourcemap/source_mapper.go:164 DocumentPosition
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentPosition {
    pub file_name: String,
    pub pos: i32,
}

impl DocumentPositionMapper {
    // Go: sourcemap/source_mapper.go:169 GetSourcePosition
    // PORT: Go allows a nil receiver; `d` is `None` for it (the lsproto
    // `X::resolve(v: Option<&X>)` form). Go returns `*DocumentPosition`.
    #[must_use]
    pub fn get_source_position(
        d: Option<&DocumentPositionMapper>,
        loc: &DocumentPosition,
    ) -> Option<DocumentPosition> {
        let d = d?;
        if d.generated_mappings.is_empty() {
            return None;
        }

        let (target_index, _) = gostd::slices::binary_search_func(
            &d.generated_mappings,
            loc.pos,
            |m: &MappedPosition, pos: &i32| m.generated_position - *pos,
        );

        if target_index >= d.generated_mappings.len() {
            return None;
        }

        let mapping = &d.generated_mappings[target_index];
        if !mapping.is_source_mapped_position() {
            return None;
        }

        // Closest position
        Some(DocumentPosition {
            file_name: d.source_file_absolute_paths[mapping.source_index as usize].clone(),
            pos: mapping.source_position,
        })
    }

    // Go: sourcemap/source_mapper.go:252 GetGeneratedPosition
    // PORT: nil receiver as in `get_source_position`.
    #[must_use]
    pub fn get_generated_position(
        d: Option<&DocumentPositionMapper>,
        loc: &DocumentPosition,
    ) -> Option<DocumentPosition> {
        let d = d?;
        let source_mappings = d
            .source_mappings_by_path
            .get(&tspath::get_canonical_file_name(
                &loc.file_name,
                d.use_case_sensitive_file_names,
            ))?;
        if source_mappings.is_empty() {
            return None;
        }
        let (target_index, _) = gostd::slices::binary_search_func(
            source_mappings,
            loc.pos,
            |m: &SourceMappedPosition, pos: &i32| m.source_position - *pos,
        );

        if target_index >= source_mappings.len() {
            return None;
        }

        let mapping = &source_mappings[target_index];

        // Closest position
        Some(DocumentPosition {
            file_name: d.generated_absolute_file_path.clone(),
            pos: mapping.generated_position,
        })
    }
}

// Go: sourcemap/source_mapper.go:229 GetDocumentPositionMapper
// PORT: Go returns `*DocumentPositionMapper`; nil is `None`, and the mapper is
// shared (`LanguageService.documentPositionMappers`) as `Rc`.
pub fn get_document_position_mapper(
    host: &dyn Host,
    generated_file_name: &str,
) -> Option<Rc<DocumentPositionMapper>> {
    let mut map_file_name = try_get_source_mapping_url(host, generated_file_name);
    if !map_file_name.is_empty() {
        let (base64_object, matched) = try_parse_base64_url(&map_file_name);
        if matched {
            if !base64_object.is_empty() {
                if let Ok(decoded) = base64_std_encoding_decode_string(base64_object) {
                    // PORT: Go `string(decoded)` keeps any bytes. Invalid UTF-8
                    // always fails the JSON decode in Go (no AllowInvalidUTF8),
                    // so the mapper is nil; a Rust `String` cannot hold it.
                    return match String::from_utf8(decoded) {
                        Ok(contents) => {
                            convert_document_to_source_mapper(host, &contents, generated_file_name)
                        }
                        Err(_) => None,
                    };
                }
            }
            // Not a data URL we can parse, skip it
            map_file_name = String::new();
        }
    }

    let mut possible_map_locations: Vec<String> = Vec::new();
    if !map_file_name.is_empty() {
        possible_map_locations.push(map_file_name);
    }
    possible_map_locations.push(format!("{generated_file_name}.map"));
    for location in &possible_map_locations {
        let map_file_name = tspath::get_normalized_absolute_path(
            location,
            &tspath::get_directory_path(generated_file_name),
        );
        let (map_file_contents, ok) = host.read_file(&map_file_name);
        if ok {
            return convert_document_to_source_mapper(host, &map_file_contents, &map_file_name);
        }
    }
    None
}

// Go: sourcemap/source_mapper.go:310 convertDocumentToSourceMapper
fn convert_document_to_source_mapper(
    host: &dyn Host,
    contents: &str,
    map_file_name: &str,
) -> Option<Rc<DocumentPositionMapper>> {
    let parsed = try_parse_raw_source_map(contents);
    let Some(parsed) = parsed else {
        // invalid map
        return None;
    };
    let source_map = &parsed.source_map;
    if source_map.sources.is_empty() || source_map.file.is_empty() || source_map.mappings.is_empty()
    {
        // invalid map
        return None;
    }

    // Don't support source maps that contain inlined sources
    if source_map
        .sources_content
        .as_ref()
        .is_some_and(|sources_content| sources_content.iter().any(|s| s.is_some()))
    {
        return None;
    }

    Some(create_document_position_mapper(
        host,
        source_map,
        parsed.source_root.as_deref(),
        &parsed.null_sources,
        map_file_name,
    ))
}

// Go: sourcemap/source_mapper.go:325 parsedRawSourceMap
struct ParsedRawSourceMap {
    source_map: RawSourceMap,
    /// Go `*string`: `None` when the key is absent or `null`.
    source_root: Option<String>,
    null_sources: Vec<bool>,
}

// Go: sourcemap/source_mapper.go:332 rawSourceMapJSON (local type of tryParseRawSourceMap)
// PORT: decoded with the Go JSON v2 default struct rules, as `RawSourceMap`
// is (decoder.rs): exact names, unknown names skipped, any duplicate name an
// error. `sourceRoot` is `*string` and `sources` is `[]*string`, so `null`
// stays apart from a string.
#[derive(Default)]
struct RawSourceMapJson {
    version: i32,
    file: String,
    source_root: Option<String>,
    sources: Vec<Option<String>>,
    names: Vec<String>,
    mappings: String,
    sources_content: Option<Vec<Option<String>>>,
}

impl UnmarshalerFrom for RawSourceMapJson {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let tok = dec.read_token()?;
        match tok {
            JsonToken::Null => {
                *self = RawSourceMapJson::default();
                Ok(())
            }
            JsonToken::BeginObject => {
                while dec.peek_kind() != b'}' {
                    let JsonToken::String(name) = dec.read_token()? else {
                        return Err(JsonError {
                            message: "object member name must be a string".to_string(),
                        });
                    };
                    match name.as_str() {
                        "version" => self.version.unmarshal_json_from(dec)?,
                        "file" => self.file.unmarshal_json_from(dec)?,
                        "sourceRoot" => self.source_root.unmarshal_json_from(dec)?,
                        "sources" => self.sources.unmarshal_json_from(dec)?,
                        "names" => self.names.unmarshal_json_from(dec)?,
                        "mappings" => self.mappings.unmarshal_json_from(dec)?,
                        "sourcesContent" => self.sources_content.unmarshal_json_from(dec)?,
                        // Skip unknown value since we have no place to store it.
                        _ => dec.skip_value()?,
                    }
                }
                dec.read_token()?;
                Ok(())
            }
            _ => {
                // Go `newUnmarshalErrorAfterWithSkipping`.
                if tok == JsonToken::BeginArray {
                    while dec.peek_kind() != b']' {
                        dec.skip_value()?;
                    }
                    dec.read_token()?;
                }
                Err(JsonError {
                    message: "cannot unmarshal JSON value into Go sourcemap.rawSourceMapJSON"
                        .to_string(),
                })
            }
        }
    }
}

// Go: sourcemap/source_mapper.go:331 tryParseRawSourceMap
fn try_parse_raw_source_map(contents: &str) -> Option<ParsedRawSourceMap> {
    let mut encoded = RawSourceMapJson::default();
    let err = json_unmarshal(contents.as_bytes(), &mut encoded, &[]);
    if err.is_err() {
        return None;
    }
    if encoded.version != 3 {
        return None;
    }
    let mut sources = vec![String::new(); encoded.sources.len()];
    let mut null_sources = vec![false; encoded.sources.len()];
    for (i, source) in encoded.sources.into_iter().enumerate() {
        match source {
            None => null_sources[i] = true,
            Some(source) => sources[i] = source,
        }
    }
    Some(ParsedRawSourceMap {
        source_map: RawSourceMap {
            version: encoded.version,
            file: encoded.file,
            source_root: String::new(),
            sources,
            names: encoded.names,
            mappings: encoded.mappings,
            sources_content: encoded.sources_content,
        },
        source_root: encoded.source_root,
        null_sources,
    })
}

// Go: sourcemap/source_mapper.go:284 tryGetSourceMappingURL
// PORT: util.rs has the exported Go `TryGetSourceMappingURL` with the same
// snake name; it is called by path.
fn try_get_source_mapping_url(host: &dyn Host, file_name: &str) -> String {
    let line_info = host.get_ecma_line_info(file_name);
    crate::sourcemap::util::try_get_source_mapping_url(line_info.as_deref())
}

// Go: sourcemap/source_mapper.go:290 tryParseBase64Url
// Equivalent to /^data:(?:application\/json;(?:charset=[uU][tT][fF]-8;)?base64,([A-Za-z0-9+/=]+)$)?/
fn try_parse_base64_url(url: &str) -> (&str, bool) {
    let Some(mut url) = url.strip_prefix("data:") else {
        return ("", false);
    };
    let Some(rest) = url.strip_prefix("application/json;") else {
        return ("", true);
    };
    url = rest;
    if let Some(rest) = url.strip_prefix("charset=") {
        url = rest;
        // PORT: Go slices `url[:len("utf-8;")]` by byte (it panics on a shorter
        // url, as this does) and compares with `strings.EqualFold`. No
        // non-ASCII rune folds to these ASCII bytes, so an ASCII
        // case-insensitive byte compare gives the same result.
        if !url.as_bytes()[.."utf-8;".len()].eq_ignore_ascii_case(b"utf-8;") {
            return ("", true);
        }
        url = &url["utf-8;".len()..];
    }
    let Some(rest) = url.strip_prefix("base64,") else {
        return ("", true);
    };
    url = rest;
    for r in url.chars() {
        if !(is_ascii_letter(r) || is_digit(r) || r == '+' || r == '/' || r == '=') {
            return ("", true);
        }
    }
    (url, true)
}

// Go: scanner/scanner.go:2741 ComputePositionOfLineAndUTF16Character
// ComputePositionOfLineAndUTF16Character converts a line and UTF-16 character offset
// back to a byte position. The character parameter is measured in UTF-16 code units.
// It scans from the line start to correctly handle multi-byte characters.
// When allowEdits is true, out-of-range values are clamped instead of panicking.
// PORT: Go package `scanner`; scanner_util.rs has no port of it, and only
// this file uses it. Go `int` values are `i32` here, like `Mapping`.
fn compute_position_of_line_and_utf16_character(
    line_starts: &[i32],
    line: i32,
    character: i32,
    text: &str,
    allow_edits: bool,
) -> i32 {
    let mut line = line;
    if line < 0 || line as usize >= line_starts.len() {
        if allow_edits {
            // Clamp line to nearest allowable value
            if line < 0 {
                line = 0;
            } else if line as usize >= line_starts.len() {
                line = line_starts.len() as i32 - 1;
            }
        } else {
            panic!(
                "Bad line number. Line: {}, lineStarts.length: {}.",
                line,
                line_starts.len()
            );
        }
    }

    let line_start = line_starts[line as usize];

    if character > 0 {
        // UTF-16 character offset: scan from line start counting UTF-16 code units.
        let mut line_end = text.len() as i32;
        if ((line + 1) as usize) < line_starts.len() {
            line_end = line_starts[(line + 1) as usize];
        }
        let mut utf16_count: i32 = 0;
        let mut pos = line_start;
        while pos < line_end {
            if utf16_count >= character {
                break;
            }
            // Go `text[pos:]` panics past the end of the text.
            assert!(pos as usize <= text.len(), "slice bounds out of range");
            let (r, size) = utf8_decode_rune_in_string(text, pos as usize);
            utf16_count += utf16_rune_len(r);
            pos += size;
        }
        if !allow_edits {
            if pos == line_end && utf16_count < character {
                panic!("Bad UTF-16 character offset. Line: {line}, character: {character}.");
            }
            go_assert!(pos as usize <= text.len());
            return pos;
        }
        if pos as usize > text.len() {
            return text.len() as i32;
        }
        return pos;
    }

    // Character is 0: line start position.
    let res = line_start;

    if allow_edits {
        if res as usize > text.len() {
            return text.len() as i32;
        }
        return res;
    }
    go_assert!(res as usize <= text.len()); // Allow single character overflow for trailing newline
    res
}

/// Go `utf16.RuneLen(r)`.
// Go: unicode/utf16/utf16.go RuneLen
fn utf16_rune_len(r: i32) -> i32 {
    if (0..0xD800).contains(&r) || (0xE000..0x10000).contains(&r) {
        1
    } else if (0x10000..=0x10FFFF).contains(&r) {
        2
    } else {
        -1
    }
}

// Go: core/core.go:847 DeduplicateSorted
// PORT: Go package `core`; not ported elsewhere in the crate.
fn deduplicate_sorted<T: Copy>(slice: Vec<T>, is_equal: impl Fn(&T, &T) -> bool) -> Vec<T> {
    if slice.is_empty() {
        return slice;
    }
    let mut last = slice[0];
    let mut deduplicated: Vec<T> = Vec::with_capacity(slice.len());
    deduplicated.push(slice[0]);
    for &next in &slice[1..] {
        if is_equal(&last, &next) {
            continue;
        }

        deduplicated.push(next);
        last = next;
    }

    deduplicated
}

// Go: encoding/base64/base64.go:312 decodeQuantum (StdEncoding.DecodeString)
// PORT: no base64 dependency. This is Go `base64.StdEncoding.DecodeString`:
// the standard alphabet, `=` padding required, not strict, and `\r` and `\n`
// skipped. Only the error result matters to the caller.
fn base64_std_encoding_decode_string(s: &str) -> Result<Vec<u8>, ()> {
    fn decode_map(c: u8) -> u8 {
        match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 0xFF,
        }
    }

    let src = s.as_bytes();
    let mut dst: Vec<u8> = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        // Decode quantum using the base64 alphabet
        let mut dbuf = [0u8; 4];
        let mut dlen = 4;
        let mut trailing_garbage = false;

        let mut j = 0;
        while j < dbuf.len() {
            if src.len() == si {
                if j == 0 {
                    return Ok(dst);
                }
                // j == 1, or padding is required (StdEncoding)
                return Err(());
            }
            let input = src[si];
            si += 1;

            let out = decode_map(input);
            if out != 0xFF {
                dbuf[j] = out;
                j += 1;
                continue;
            }

            if input == b'\n' || input == b'\r' {
                continue;
            }

            if input != b'=' {
                return Err(());
            }

            // We've reached the end and there's padding
            match j {
                0 | 1 => {
                    // incorrect padding
                    return Err(());
                }
                2 => {
                    // "==" is expected, the first "=" is already consumed.
                    // skip over newlines
                    while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                        si += 1;
                    }
                    if si == src.len() {
                        // not enough padding
                        return Err(());
                    }
                    if src[si] != b'=' {
                        // incorrect padding
                        return Err(());
                    }

                    si += 1;
                }
                _ => {}
            }

            // skip over newlines
            while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                si += 1;
            }
            if si < src.len() {
                // trailing garbage
                trailing_garbage = true;
            }
            dlen = j;
            break;
        }

        // Convert 4x 6bit source bytes into 3 bytes
        let val = u32::from(dbuf[0]) << 18
            | u32::from(dbuf[1]) << 12
            | u32::from(dbuf[2]) << 6
            | u32::from(dbuf[3]);
        let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
        match dlen {
            4 => dst.extend_from_slice(&bytes[..3]),
            3 => dst.extend_from_slice(&bytes[..2]),
            2 => dst.extend_from_slice(&bytes[..1]),
            _ => {}
        }
        if trailing_garbage {
            return Err(());
        }
    }
    Ok(dst)
}

// Go: sourcemap/source_mapper_test.go (the ts#64544 tests). The ts#64159
// tests (`TestSourceMapperIgnoresExternalMapURLSuffix`,
// `TestSourceMapperIgnoresSourceURLSuffix`) wait for bump D wave 2b.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner_util::compute_ecma_line_starts;
    use crate::sourcemap::lineinfo::create_ecma_line_info;

    // Go: sourcemap/source_mapper_test.go:11 sourceMapperTestHost
    struct SourceMapperTestHost {
        files: FxHashMap<String, String>,
    }

    impl SourceMapperTestHost {
        fn new(files: &[(&str, &str)]) -> Self {
            SourceMapperTestHost {
                files: files
                    .iter()
                    .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
                    .collect(),
            }
        }
    }

    impl Host for SourceMapperTestHost {
        fn use_case_sensitive_file_names(&self) -> bool {
            true
        }

        fn get_ecma_line_info(&self, file_name: &str) -> Option<Rc<ECMALineInfo>> {
            let text = self.files.get(file_name)?;
            Some(Rc::new(create_ecma_line_info(
                text,
                compute_ecma_line_starts(text),
            )))
        }

        fn read_file(&self, file_name: &str) -> (String, bool) {
            match self.files.get(file_name) {
                Some(text) => (text.clone(), true),
                None => (String::new(), false),
            }
        }
    }

    fn pos(file_name: &str, pos: i32) -> DocumentPosition {
        DocumentPosition {
            file_name: file_name.to_string(),
            pos,
        }
    }

    fn source_position(
        mapper: &DocumentPositionMapper,
        loc: DocumentPosition,
    ) -> Option<DocumentPosition> {
        DocumentPositionMapper::get_source_position(Some(mapper), &loc)
    }

    fn generated_position(
        mapper: &DocumentPositionMapper,
        loc: DocumentPosition,
    ) -> Option<DocumentPosition> {
        DocumentPositionMapper::get_generated_position(Some(mapper), &loc)
    }

    // Go: sourcemap/source_mapper_test.go:32 TestSourceMapperPreservesEmptySourceEntries
    #[test]
    fn test_source_mapper_preserves_empty_source_entries() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/src/real.ts", "source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sourceRoot":"../src","sources":["","real.ts"],"names":[],"mappings":"ACAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            Some(pos("/project/src/real.ts", 0))
        );
        assert_eq!(
            generated_position(&mapper, pos("/project/src/real.ts", 0)),
            Some(pos("/project/out/out.d.ts", 0))
        );
    }

    // Go: sourcemap/source_mapper_test.go:61 TestSourceMapperResolvesEmptySourceToSourceRoot
    #[test]
    fn test_source_mapper_resolves_empty_source_to_source_root() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/src", "source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sourceRoot":"../src","sources":[""],"names":[],"mappings":"AAAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            Some(pos("/project/src", 0))
        );
    }

    // Go: sourcemap/source_mapper_test.go:83 TestSourceMapperResolvesEmptySourceToMapURLWithoutSourceRoot
    #[test]
    fn test_source_mapper_resolves_empty_source_to_map_url_without_source_root() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/out/out.d.ts.map", "source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sources":[""],"names":[],"mappings":"AAAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            Some(pos("/project/out/out.d.ts.map", 0))
        );
    }

    // Go: sourcemap/source_mapper_test.go:105 TestSourceMapperTreatsEmptySourceRootAsAbsent
    // PORT: Go runs each case and root as a parallel subtest; here they run
    // in a loop, and a failure names them.
    #[test]
    fn test_source_mapper_treats_empty_source_root_as_absent() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/out/out.d.ts.map", "map-relative empty source"),
            ("/project/out/a.ts", "map-relative source"),
            ("/project/src/a.ts", "parent-relative source"),
            ("/", "unrelated root"),
            ("/a.ts", "unrelated root source"),
            ("/missing.ts", "unrelated root source"),
        ]);
        let cases = [
            ("empty source", "", "/project/out/out.d.ts.map"),
            ("relative source", "a.ts", "/project/out/a.ts"),
            ("parent-relative source", "../src/a.ts", "/project/src/a.ts"),
            ("missing source", "missing.ts", ""),
        ];
        let roots = [("absent", ""), ("empty", r#""sourceRoot":"","#)];
        for (name, source, file_name) in cases {
            for (root_name, field) in roots {
                let contents = format!(
                    r#"{{"version":3,"file":"out.d.ts",{field}"sources":["{source}"],"names":[],"mappings":"AAAA"}}"#
                );
                let mapper = convert_document_to_source_mapper(
                    &host,
                    &contents,
                    "/project/out/out.d.ts.map",
                )
                .unwrap_or_else(|| panic!("{name}/{root_name}: mapper"));
                let source_pos = source_position(&mapper, pos("/project/out/out.d.ts", 0));
                if file_name.is_empty() {
                    assert_eq!(source_pos, None, "{name}/{root_name}");
                    continue;
                }
                assert_eq!(source_pos, Some(pos(file_name, 0)), "{name}/{root_name}");
                assert_eq!(
                    generated_position(&mapper, pos(file_name, 0)),
                    Some(pos("/project/out/out.d.ts", 0)),
                    "{name}/{root_name}"
                );
            }
        }
    }

    // Go: sourcemap/source_mapper_test.go:169 TestSourceMapperPrefixesAbsoluteSourceWithNonemptySourceRoot
    #[test]
    fn test_source_mapper_prefixes_absolute_source_with_nonempty_source_root() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/src/actual/a.ts", "prefixed source"),
            ("/actual/a.ts", "unprefixed source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sourceRoot":"../src","sources":["/actual/a.ts"],"names":[],"mappings":"AAAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            Some(pos("/project/src/actual/a.ts", 0))
        );
    }

    // Go: sourcemap/source_mapper_test.go:192 TestSourceMapperRetainsDuplicateSourceIndices
    #[test]
    fn test_source_mapper_retains_duplicate_source_indices() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/src", "source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sourceRoot":"../src","sources":["",""],"names":[],"mappings":"AAAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            generated_position(&mapper, pos("/project/src", 0)),
            Some(pos("/project/out/out.d.ts", 0))
        );
    }

    // Go: sourcemap/source_mapper_test.go:214 TestSourceMapperPreservesNullSourceEntries
    #[test]
    fn test_source_mapper_preserves_null_source_entries() {
        let host = SourceMapperTestHost::new(&[
            ("/project/out/out.d.ts", "generated"),
            ("/project/out/real.ts", "source"),
        ]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sources":[null,"real.ts"],"names":[],"mappings":"ACAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            Some(pos("/project/out/real.ts", 0))
        );

        let null_mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sources":[null],"names":[],"mappings":"AAAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("null mapper");
        assert_eq!(
            source_position(&null_mapper, pos("/project/out/out.d.ts", 0)),
            None
        );
    }

    // Go: sourcemap/source_mapper_test.go:247 TestSourceMapperIgnoresOutOfRangeSourceIndex
    #[test]
    fn test_source_mapper_ignores_out_of_range_source_index() {
        let host = SourceMapperTestHost::new(&[]);
        let mapper = convert_document_to_source_mapper(
            &host,
            r#"{"version":3,"file":"out.d.ts","sources":["real.ts"],"names":[],"mappings":"ACAA"}"#,
            "/project/out/out.d.ts.map",
        )
        .expect("mapper");
        assert_eq!(
            source_position(&mapper, pos("/project/out/out.d.ts", 0)),
            None
        );
    }
}
