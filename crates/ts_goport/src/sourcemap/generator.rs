//! Port of Go `sourcemap/generator.go`: the source map generator that the
//! printer feeds while it writes a file.

use crate::prelude::*;

use crate::frontend::json::append_json_quote;
use crate::frontend::tspath::{ComparePathsOptions, get_relative_path_to_directory_or_url};

/// Go `sourcemap.SourceIndex`.
pub type SourceIndex = i32;
/// Go `sourcemap.NameIndex`.
pub type NameIndex = i32;

const SOURCE_INDEX_NOT_SET: SourceIndex = -1;
const NAME_INDEX_NOT_SET: NameIndex = -1;
const NOT_SET: i32 = -1;
const NOT_SET_UTF16: i32 = -1;

// Go: sourcemap/generator.go:26 Generator
// PORT: Go `int` lines and `core.UTF16Offset` characters are `i32`, like the
// printer writer. Go `[]*string` sources content is `Vec<Option<String>>`.
#[derive(Debug, Default)]
pub struct Generator {
    path_options: ComparePathsOptions,
    file: String,
    source_root: String,
    sources_directory_path: String,
    raw_sources: Vec<String>,
    sources: Vec<String>,
    source_to_source_index_map: FxHashMap<String, SourceIndex>,
    sources_content: Vec<Option<String>>,
    names: Vec<String>,
    name_to_name_index_map: FxHashMap<String, NameIndex>,
    mappings: String,
    last_generated_line: i32,
    last_generated_character: i32,
    last_source_index: SourceIndex,
    last_source_line: i32,
    last_source_character: i32,
    last_name_index: NameIndex,
    has_last: bool,
    pending_generated_line: i32,
    pending_generated_character: i32,
    pending_source_index: SourceIndex,
    pending_source_line: i32,
    pending_source_character: i32,
    pending_name_index: NameIndex,
    has_pending: bool,
    has_pending_source: bool,
    has_pending_name: bool,
}

// Go: sourcemap/generator.go:56 RawSourceMap
// PORT: Go `SourcesContent []*string` with `omitzero` is `None` when Go has a
// nil slice. `SourceRoot` has `omitzero` too (ts#64544): `to_json` omits an
// empty root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawSourceMap {
    pub version: i32,
    pub file: String,
    pub source_root: String,
    pub sources: Vec<String>,
    pub names: Vec<String>,
    pub mappings: String,
    pub sources_content: Option<Vec<Option<String>>>,
}

impl RawSourceMap {
    /// Go `json.Marshal(rawSourceMap)` with the `internal/json` defaults
    /// (go-json-experiment v2: fields in struct order, no HTML escaping).
    // PORT: not a Go function. The struct tags give the key order.
    #[must_use]
    pub fn to_json(&self) -> String {
        // A decoded map can hold any mappings text, so check it before the
        // fast quote. The generator skips this check.
        let quote_mappings: fn(&mut String, &str) = if self.mappings.bytes().all(is_vlq_byte) {
            append_vlq_quote
        } else {
            append_json_quote
        };
        RawSourceMapJson {
            version: self.version,
            file: &self.file,
            source_root: &self.source_root,
            sources: &self.sources,
            names: &self.names,
            mappings: &self.mappings,
            sources_content: self.sources_content.as_deref(),
        }
        .write(quote_mappings)
    }
}

/// Borrowed `RawSourceMap` fields. `RawSourceMap::to_json` and
/// `Generator::bytes` both write JSON through it, so the key order and the
/// omit rules are in one place and the generator clones nothing.
struct RawSourceMapJson<'a> {
    version: i32,
    file: &'a str,
    source_root: &'a str,
    sources: &'a [String],
    names: &'a [String],
    mappings: &'a str,
    /// `None` omits the key (Go `omitzero` on a nil slice).
    sources_content: Option<&'a [Option<String>]>,
}

impl RawSourceMapJson<'_> {
    /// Writes the JSON. `quote_mappings` is `append_vlq_quote` when the
    /// mappings are known VLQ text, else `append_json_quote`.
    fn write(&self, quote_mappings: fn(&mut String, &str)) -> String {
        let mut out = String::with_capacity(self.len_hint());
        out.push_str("{\"version\":");
        out.push_str(&self.version.to_string());
        out.push_str(",\"file\":");
        append_json_quote(&mut out, self.file);
        // Go `json:"sourceRoot,omitzero"` (ts#64544): an empty root is omitted.
        if !self.source_root.is_empty() {
            out.push_str(",\"sourceRoot\":");
            append_json_quote(&mut out, self.source_root);
        }
        out.push_str(",\"sources\":");
        append_json_string_array(&mut out, self.sources);
        out.push_str(",\"names\":");
        append_json_string_array(&mut out, self.names);
        out.push_str(",\"mappings\":");
        quote_mappings(&mut out, self.mappings);
        if let Some(sources_content) = self.sources_content {
            out.push_str(",\"sourcesContent\":[");
            for (i, content) in sources_content.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match content {
                    Some(content) => append_json_quote(&mut out, content),
                    None => out.push_str("null"),
                }
            }
            out.push(']');
        }
        out.push('}');
        out
    }

    /// The output size without escapes, so the common case never grows the
    /// buffer. Each list item adds two quotes and a comma.
    fn len_hint(&self) -> usize {
        let list = |values: &[String]| values.iter().map(|v| v.len() + 3).sum::<usize>();
        let content = self.sources_content.map_or(0, |contents| {
            contents
                .iter()
                .map(|c| c.as_ref().map_or(5, |c| c.len() + 3))
                .sum::<usize>()
        });
        128 + self.file.len()
            + self.source_root.len()
            + self.mappings.len()
            + list(self.sources)
            + list(self.names)
            + content
    }
}

/// A byte that VLQ mappings can hold: a base64 digit, ',' or ';'.
fn is_vlq_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b',' | b';')
}

/// Quotes VLQ mappings with one `push_str`. JSON escapes none of the VLQ
/// bytes, so the output is the same as `append_json_quote`.
fn append_vlq_quote(out: &mut String, mappings: &str) {
    debug_assert!(
        mappings.bytes().all(is_vlq_byte),
        "mappings must hold only base64 digits, ',' and ';'"
    );
    out.push('"');
    out.push_str(mappings);
    out.push('"');
}

fn append_json_string_array(out: &mut String, values: &[String]) {
    out.push('[');
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        append_json_quote(out, value);
    }
    out.push(']');
}

// Go: sourcemap/generator.go:66 NewGenerator
#[must_use]
pub fn new_generator(
    file: &str,
    source_root: &str,
    sources_directory_path: &str,
    options: ComparePathsOptions,
) -> Generator {
    Generator {
        file: file.to_string(),
        source_root: source_root.to_string(),
        sources_directory_path: sources_directory_path.to_string(),
        path_options: options,
        // PORT: skips the first small regrowths of the mappings buffer.
        mappings: String::with_capacity(MAPPINGS_INITIAL_CAPACITY),
        ..Generator::default()
    }
}

/// Start size of the mappings buffer, so small files never regrow it.
const MAPPINGS_INITIAL_CAPACITY: usize = 1024;

impl Generator {
    // Go: sourcemap/generator.go:75 Sources
    #[must_use]
    pub fn sources(&self) -> Vec<String> {
        self.raw_sources.clone()
    }

    // Go: sourcemap/generator.go:78 AddSource
    // Adds a source to the source map
    pub fn add_source(&mut self, file_name: &str) -> SourceIndex {
        let source = get_relative_path_to_directory_or_url(
            &self.sources_directory_path,
            file_name,
            true, /*isAbsolutePathAnUrl*/
            &self.path_options,
        );

        if let Some(&source_index) = self.source_to_source_index_map.get(&source) {
            return source_index;
        }
        let source_index = self.sources.len() as SourceIndex;
        self.sources.push(source.clone());
        self.raw_sources.push(file_name.to_string());
        self.source_to_source_index_map.insert(source, source_index);
        source_index
    }

    // Go: sourcemap/generator.go:101 SetSourceContent
    // Sets the content for a source
    pub fn set_source_content(
        &mut self,
        source_index: SourceIndex,
        content: &str,
    ) -> Result<(), String> {
        if source_index < 0 || source_index as usize >= self.sources.len() {
            return Err("sourceIndex is out of range".to_string());
        }
        while self.sources_content.len() <= source_index as usize {
            self.sources_content.push(None);
        }
        self.sources_content[source_index as usize] = Some(content.to_string());
        Ok(())
    }

    // Go: sourcemap/generator.go:113 AddName
    // Declares a name in the source map, returning the index of the name
    pub fn add_name(&mut self, name: &str) -> NameIndex {
        if let Some(&name_index) = self.name_to_name_index_map.get(name) {
            return name_index;
        }
        let name_index = self.names.len() as NameIndex;
        self.names.push(name.to_string());
        self.name_to_name_index_map
            .insert(name.to_string(), name_index);
        name_index
    }

    // Go: sourcemap/generator.go:126 isNewGeneratedPosition
    fn is_new_generated_position(&self, generated_line: i32, generated_character: i32) -> bool {
        !self.has_pending
            || self.pending_generated_line != generated_line
            || self.pending_generated_character != generated_character
    }

    // Go: sourcemap/generator.go:132 isBacktrackingSourcePosition
    fn is_backtracking_source_position(
        &self,
        source_index: SourceIndex,
        source_line: i32,
        source_character: i32,
    ) -> bool {
        source_index != SOURCE_INDEX_NOT_SET
            && source_line != NOT_SET
            && source_character != NOT_SET_UTF16
            && self.pending_source_index == source_index
            && (self.pending_source_line > source_line
                || self.pending_source_line == source_line
                    && self.pending_source_character > source_character)
    }

    // Go: sourcemap/generator.go:141 shouldCommitMapping
    fn should_commit_mapping(&self) -> bool {
        self.has_pending
            && (!self.has_last
                || self.last_generated_line != self.pending_generated_line
                || self.last_generated_character != self.pending_generated_character
                || self.last_source_index != self.pending_source_index
                || self.last_source_line != self.pending_source_line
                || self.last_source_character != self.pending_source_character
                || self.last_name_index != self.pending_name_index)
    }

    // Go: sourcemap/generator.go:151 appendMappingCharCode
    // PORT: takes an ASCII byte. The `& 0x7F` changes no caller's value; it
    // tells the compiler the char is ASCII, so `push` writes one byte.
    fn append_mapping_char_code(&mut self, char_code: u8) {
        debug_assert!(char_code.is_ascii());
        self.mappings.push(char::from(char_code & 0x7F));
    }

    // Go: sourcemap/generator.go:155 appendBase64VLQ
    fn append_base64_vlq(&mut self, in_value: i32) {
        // Add a new least significant bit that has the sign of the value.
        // if negative number the least significant bit that gets added to the number has value 1
        // else least significant bit value that gets added is 0
        // eg. -1 changes to binary : 01 [1] => 3
        //     +1 changes to binary : 01 [0] => 2
        // PORT: Go `int` is 64 bits; widen so the shift cannot overflow.
        let mut in_value = i64::from(in_value);
        if in_value < 0 {
            in_value = ((-in_value) << 1) + 1;
        } else {
            in_value <<= 1;
        }

        // Encode 5 bits at a time starting from least significant bits
        loop {
            let mut current_digit = in_value & 31; // 11111
            in_value >>= 5;
            if in_value > 0 {
                // There are still more digits to decode, set the msb (6th bit)
                current_digit |= 32;
            }
            self.append_mapping_char_code(base64_format_encode(current_digit as usize));
            if in_value <= 0 {
                break;
            }
        }
    }

    // Go: sourcemap/generator.go:182 commitPendingMapping
    fn commit_pending_mapping(&mut self) {
        if !self.should_commit_mapping() {
            return;
        }

        // Line/Comma delimiters
        if self.last_generated_line < self.pending_generated_line {
            // Emit line delimiters
            loop {
                self.append_mapping_char_code(b';');
                self.last_generated_line += 1;
                if self.last_generated_line >= self.pending_generated_line {
                    break;
                }
            }
            // Only need to set this once
            self.last_generated_character = 0;
        } else {
            if self.last_generated_line != self.pending_generated_line {
                // panic rather than error as an invariant has been violated
                panic!("generatedLine cannot backtrack");
            }
            // Emit comma to separate the entry
            if self.has_last {
                self.append_mapping_char_code(b',');
            }
        }

        // 1. Relative generated character
        self.append_base64_vlq(self.pending_generated_character - self.last_generated_character);
        self.last_generated_character = self.pending_generated_character;

        if self.has_pending_source {
            // 2. Relative sourceIndex
            self.append_base64_vlq(self.pending_source_index - self.last_source_index);
            self.last_source_index = self.pending_source_index;

            // 3. Relative source line
            self.append_base64_vlq(self.pending_source_line - self.last_source_line);
            self.last_source_line = self.pending_source_line;

            // 4. Relative source character
            self.append_base64_vlq(self.pending_source_character - self.last_source_character);
            self.last_source_character = self.pending_source_character;

            if self.has_pending_name {
                // 5. Relative nameIndex
                self.append_base64_vlq(self.pending_name_index - self.last_name_index);
                self.last_name_index = self.pending_name_index;
            }
        }

        self.has_last = true;
    }

    // Go: sourcemap/generator.go:237 addMapping
    fn add_mapping(
        &mut self,
        generated_line: i32,
        generated_character: i32,
        source_index: SourceIndex,
        source_line: i32,
        source_character: i32,
        name_index: NameIndex,
    ) {
        if self.is_new_generated_position(generated_line, generated_character)
            || self.is_backtracking_source_position(source_index, source_line, source_character)
        {
            self.commit_pending_mapping();
            self.pending_generated_line = generated_line;
            self.pending_generated_character = generated_character;
            self.has_pending_source = false;
            self.has_pending_name = false;
            self.has_pending = true;
        }

        if source_index != SOURCE_INDEX_NOT_SET
            && source_line != NOT_SET
            && source_character != NOT_SET_UTF16
        {
            self.pending_source_index = source_index;
            self.pending_source_line = source_line;
            self.pending_source_character = source_character;
            self.has_pending_source = true;
            if name_index != NAME_INDEX_NOT_SET {
                self.pending_name_index = name_index;
                self.has_pending_name = true;
            }
        }
    }

    // Go: sourcemap/generator.go:261 AddGeneratedMapping
    // Adds a mapping without source information
    pub fn add_generated_mapping(
        &mut self,
        generated_line: i32,
        generated_character: i32,
    ) -> Result<(), String> {
        if generated_line < self.pending_generated_line {
            return Err("generatedLine cannot backtrack".to_string());
        }
        if generated_character < 0 {
            return Err("generatedCharacter cannot be negative".to_string());
        }
        self.add_mapping(
            generated_line,
            generated_character,
            SOURCE_INDEX_NOT_SET,
            NOT_SET,       /*sourceLine*/
            NOT_SET_UTF16, /*sourceCharacter*/
            NAME_INDEX_NOT_SET,
        );
        self.has_pending_source = false;
        self.has_pending_name = false;
        Ok(())
    }

    // Go: sourcemap/generator.go:275 AddSourceMapping
    // Adds a mapping with source information
    pub fn add_source_mapping(
        &mut self,
        generated_line: i32,
        generated_character: i32,
        source_index: SourceIndex,
        source_line: i32,
        source_character: i32,
    ) -> Result<(), String> {
        if generated_line < self.pending_generated_line {
            return Err("generatedLine cannot backtrack".to_string());
        }
        if generated_character < 0 {
            return Err("generatedCharacter cannot be negative".to_string());
        }
        if source_index < 0 || source_index as usize >= self.sources.len() {
            return Err("sourceIndex is out of range".to_string());
        }
        if source_line < 0 {
            return Err("sourceLine cannot be negative".to_string());
        }
        if source_character < 0 {
            return Err("sourceCharacter cannot be negative".to_string());
        }
        if self.has_pending
            && !self.is_new_generated_position(generated_line, generated_character)
            && !self.has_pending_source
        {
            return Ok(());
        }
        self.add_mapping(
            generated_line,
            generated_character,
            source_index,
            source_line,
            source_character,
            NAME_INDEX_NOT_SET,
        );
        Ok(())
    }

    // Go: sourcemap/generator.go:299 AddNamedSourceMapping
    // Adds a mapping with source and name information
    pub fn add_named_source_mapping(
        &mut self,
        generated_line: i32,
        generated_character: i32,
        source_index: SourceIndex,
        source_line: i32,
        source_character: i32,
        name_index: NameIndex,
    ) -> Result<(), String> {
        if generated_line < self.pending_generated_line {
            return Err("generatedLine cannot backtrack".to_string());
        }
        if generated_character < 0 {
            return Err("generatedCharacter cannot be negative".to_string());
        }
        if source_index < 0 || source_index as usize >= self.sources.len() {
            return Err("sourceIndex is out of range".to_string());
        }
        if source_line < 0 {
            return Err("sourceLine cannot be negative".to_string());
        }
        if source_character < 0 {
            return Err("sourceCharacter cannot be negative".to_string());
        }
        if name_index < 0 || name_index as usize >= self.names.len() {
            return Err("nameIndex is out of range".to_string());
        }
        if self.has_pending
            && !self.is_new_generated_position(generated_line, generated_character)
            && !self.has_pending_source
        {
            return Ok(());
        }
        self.add_mapping(
            generated_line,
            generated_character,
            source_index,
            source_line,
            source_character,
            name_index,
        );
        Ok(())
    }

    // Go: sourcemap/generator.go:316 RawSourceMap
    // Gets the source map as a `RawSourceMap` object
    pub fn raw_source_map(&mut self) -> RawSourceMap {
        self.commit_pending_mapping();
        RawSourceMap {
            version: 3,
            file: self.file.clone(),
            source_root: self.source_root.clone(),
            sources: self.sources.clone(),
            names: self.names.clone(),
            mappings: self.mappings.clone(),
            // Go `slices.Clone` of a nil slice is nil, and `omitzero` drops it.
            sources_content: if self.sources_content.is_empty() {
                None
            } else {
                Some(self.sources_content.clone())
            },
        }
    }

    // Go: sourcemap/generator.go:347 bytes
    // PORT: Go marshals the cloned `RawSourceMap()`. This writes the same
    // JSON from the fields in place, after the same commit.
    fn bytes(&mut self) -> String {
        self.commit_pending_mapping();
        RawSourceMapJson {
            version: 3,
            file: &self.file,
            source_root: &self.source_root,
            sources: &self.sources,
            names: &self.names,
            mappings: &self.mappings,
            // Same omit rule as `raw_source_map`.
            sources_content: if self.sources_content.is_empty() {
                None
            } else {
                Some(self.sources_content.as_slice())
            },
        }
        .write(append_vlq_quote)
    }

    // Go: sourcemap/generator.go:356 String
    // Gets the string representation of the source map
    pub fn string(&mut self) -> String {
        self.bytes()
    }

    // Go: sourcemap/generator.go:360 Base64DataURL
    pub fn base64_data_url(&mut self) -> String {
        const PREFIX: &str = "data:application/json;base64,";
        let data = self.bytes();
        let mut sb = String::with_capacity(PREFIX.len() + data.len().div_ceil(3) * 4);
        sb.push_str(PREFIX);
        // PORT: `data` is in the port form (see
        // `scanner_util::GO_STRING_MARKER`); Go encodes its bytes.
        base64_std_encode(&mut sb, &go_string_bytes(&data));
        sb
    }
}

/// The standard base64 digits. Both `base64FormatEncode` and
/// `base64.StdEncoding` use this order.
const BASE64_DIGITS: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Go `base64.StdEncoding` (with `=` padding).
// PORT: Go uses `encoding/base64`; the crate adds no dependency for it.
fn base64_std_encode(out: &mut String, data: &[u8]) {
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |&b| u32::from(b));
        let b2 = chunk.get(2).map_or(0, |&b| u32::from(b));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64_DIGITS[((triple >> 18) & 63) as usize] as char);
        out.push(BASE64_DIGITS[((triple >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_DIGITS[((triple >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_DIGITS[(triple & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
}

// Go: sourcemap/generator.go:372 base64FormatEncode
// PORT: a table lookup that returns the ASCII byte. A value past 63 panics
// on the index, as Go panics.
fn base64_format_encode(value: usize) -> u8 {
    BASE64_DIGITS[value]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq_and_json_match_go() {
        let mut generator = new_generator("a.js", "", "/p", ComparePathsOptions::default());
        let source = generator.add_source("/p/a.ts");
        generator.add_source_mapping(0, 0, source, 0, 0).unwrap();
        generator.add_source_mapping(0, 4, source, 0, 6).unwrap();
        generator.add_source_mapping(1, 0, source, 1, 0).unwrap();
        assert_eq!(
            generator.string(),
            r#"{"version":3,"file":"a.js","sources":["a.ts"],"names":[],"mappings":"AAAA,IAAM;AACN"}"#
        );
    }

    #[test]
    fn generator_json_matches_raw_source_map_json() {
        let mut generator = new_generator("a.js", "r/", "/p", ComparePathsOptions::default());
        let source = generator.add_source("/p/a.ts");
        generator
            .set_source_content(source, "let x = \"\\\n")
            .unwrap();
        let name = generator.add_name("x");
        generator
            .add_named_source_mapping(0, 4, source, 0, 4, name)
            .unwrap();
        generator.add_source_mapping(2, 0, source, 1, 0).unwrap();
        let raw = generator.raw_source_map().to_json();
        assert_eq!(generator.string(), raw);
        assert_eq!(
            raw,
            r#"{"version":3,"file":"a.js","sourceRoot":"r/","sources":["a.ts"],"names":["x"],"mappings":"IAAIA;;AACJ","sourcesContent":["let x = \"\\\n"]}"#
        );

        // A decoded map can hold mappings that need escapes.
        let decoded = RawSourceMap {
            version: 3,
            mappings: "A\"".to_string(),
            ..RawSourceMap::default()
        };
        assert_eq!(
            decoded.to_json(),
            r#"{"version":3,"file":"","sources":[],"names":[],"mappings":"A\""}"#
        );
    }

    #[test]
    fn base64_padding() {
        let mut s = String::new();
        base64_std_encode(&mut s, b"ab");
        assert_eq!(s, "YWI=");
    }
}
