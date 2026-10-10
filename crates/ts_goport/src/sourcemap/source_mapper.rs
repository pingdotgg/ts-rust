//! Port of Go `sourcemap/source_mapper.go`.

use crate::prelude::*;

use crate::frontend::json::json_unmarshal;
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

// Go: sourcemap/source_mapper.go:38 SourceMappedPosition
pub type SourceMappedPosition = MappedPosition;

// Go: sourcemap/source_mapper.go:41 DocumentPositionMapper
// Maps source positions to generated positions and vice versa.
#[derive(Clone, Debug, Default)]
pub struct DocumentPositionMapper {
    use_case_sensitive_file_names: bool,

    source_file_absolute_paths: Vec<String>,
    source_to_source_index_map: FxHashMap<String, SourceIndex>,
    generated_absolute_file_path: String,

    generated_mappings: Vec<MappedPosition>,
    source_mappings: FxHashMap<SourceIndex, Vec<SourceMappedPosition>>,
}

// Go: sourcemap/source_mapper.go:52 createDocumentPositionMapper
fn create_document_position_mapper(
    host: &dyn Host,
    source_map: &RawSourceMap,
    map_path: &str,
) -> Rc<DocumentPositionMapper> {
    let map_directory = tspath::get_directory_path(map_path);
    let source_root = if !source_map.source_root.is_empty() {
        tspath::get_normalized_absolute_path(&source_map.source_root, &map_directory)
    } else {
        map_directory.clone()
    };
    let generated_absolute_file_path =
        tspath::get_normalized_absolute_path(&source_map.file, &map_directory);
    let source_file_absolute_paths: Vec<String> = source_map
        .sources
        .iter()
        .map(|source| tspath::get_normalized_absolute_path(source, &source_root))
        .collect();
    let use_case_sensitive_file_names = host.use_case_sensitive_file_names();
    let mut source_to_source_index_map: FxHashMap<String, SourceIndex> =
        FxHashMap::with_capacity_and_hasher(source_file_absolute_paths.len(), Default::default());
    for (i, source) in source_file_absolute_paths.iter().enumerate() {
        source_to_source_index_map.insert(
            tspath::get_canonical_file_name(source, use_case_sensitive_file_names),
            i as SourceIndex,
        );
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
            let line_info =
                host.get_ecma_line_info(&source_file_absolute_paths[mapping.source_index as usize]);
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
                a.source_position - b.source_position
            },
        );
        *list = deduplicate_sorted(std::mem::take(list), |a, b| {
            a.generated_position == b.generated_position
                && a.source_index == b.source_index
                && a.source_position == b.source_position
        });
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
        source_to_source_index_map,
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

    // Go: sourcemap/source_mapper.go:197 GetGeneratedPosition
    // PORT: nil receiver as in `get_source_position`.
    #[must_use]
    pub fn get_generated_position(
        d: Option<&DocumentPositionMapper>,
        loc: &DocumentPosition,
    ) -> Option<DocumentPosition> {
        let d = d?;
        let Some(&source_index) =
            d.source_to_source_index_map
                .get(&tspath::get_canonical_file_name(
                    &loc.file_name,
                    d.use_case_sensitive_file_names,
                ))
        else {
            return None;
        };
        // Go compares with `len(d.sourceMappings)`, the number of map keys.
        if source_index < 0 || source_index as usize >= d.source_mappings.len() {
            return None;
        }
        // Go: a missing key reads a nil slice.
        let source_mappings: &[SourceMappedPosition] = match d.source_mappings.get(&source_index) {
            Some(list) => list.as_slice(),
            None => &[],
        };
        let (target_index, _) = gostd::slices::binary_search_func(
            source_mappings,
            loc.pos,
            |m: &SourceMappedPosition, pos: &i32| m.source_position - *pos,
        );

        if target_index >= source_mappings.len() {
            return None;
        }

        let mapping = &source_mappings[target_index];
        if mapping.source_index != source_index {
            return None;
        }

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

// Go: sourcemap/source_mapper.go:257 convertDocumentToSourceMapper
fn convert_document_to_source_mapper(
    host: &dyn Host,
    contents: &str,
    map_file_name: &str,
) -> Option<Rc<DocumentPositionMapper>> {
    let source_map = try_parse_raw_source_map(contents);
    let Some(source_map) = source_map else {
        // invalid map
        return None;
    };
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
        &source_map,
        map_file_name,
    ))
}

// Go: sourcemap/source_mapper.go:272 tryParseRawSourceMap
fn try_parse_raw_source_map(contents: &str) -> Option<RawSourceMap> {
    let mut source_map = RawSourceMap::default();
    let err = json_unmarshal(contents.as_bytes(), &mut source_map, &[]);
    if err.is_err() {
        return None;
    }
    if source_map.version != 3 {
        return None;
    }
    Some(source_map)
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
