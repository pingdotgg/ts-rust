//! Shared compiler primitives (`TextRange`, `JsString`, `SourceText`). This
//! was the `ts_core` crate.

use std::{fmt, ops::Range, sync::Arc};

/// Lossless source bytes plus a byte-offset-preserving scanner view.
///
/// Invalid UTF-8 bytes are retained in `as_bytes` and represented by one ASCII
/// sentinel byte each in `as_scannable_str`, so scanner byte offsets remain
/// identical to offsets in the original input.
#[derive(Clone, Eq, PartialEq)]
pub struct SourceText {
    bytes: Arc<[u8]>,
    scannable: Arc<str>,
    invalid_byte_ranges: Arc<[Range<usize>]>,
}

impl SourceText {
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();
        let (scannable, invalid_byte_ranges) = make_scannable(&bytes);
        Self {
            bytes: bytes.into(),
            scannable: scannable.into(),
            invalid_byte_ranges: invalid_byte_ranges.into(),
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        if self.invalid_byte_ranges.is_empty() {
            Some(&self.scannable)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_scannable_str(&self) -> &str {
        &self.scannable
    }

    #[must_use]
    pub fn invalid_byte_ranges(&self) -> &[Range<usize>] {
        &self.invalid_byte_ranges
    }

    #[must_use]
    pub fn is_valid_utf8(&self) -> bool {
        self.invalid_byte_ranges.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

impl From<String> for SourceText {
    fn from(value: String) -> Self {
        Self::from_bytes(value.into_bytes())
    }
}

impl From<&str> for SourceText {
    fn from(value: &str) -> Self {
        Self::from_bytes(value.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for SourceText {
    fn from(value: Vec<u8>) -> Self {
        Self::from_bytes(value)
    }
}

impl fmt::Debug for SourceText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceText")
            .field("bytes", &self.bytes)
            .field("scannable", &self.scannable)
            .field(
                "invalid_byte_ranges",
                &RangesDebug(&self.invalid_byte_ranges),
            )
            .finish()
    }
}

// lust: `RangesDebug` and `RangeDebug` print a range list as std's Debug of
// `[Range<usize>]` does, for `{:?}` and `{:#?}` (lust has no Debug of a range
// inside a container). Used only by `SourceText`'s Debug.
struct RangesDebug<'a>(&'a [Range<usize>]);

impl fmt::Debug for RangesDebug<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|range| RangeDebug(range)))
            .finish()
    }
}

struct RangeDebug<'a>(&'a Range<usize>);

impl fmt::Debug for RangeDebug<'_> {
    // The same steps as std's `impl Debug for Range`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.start.fmt(formatter)?;
        formatter.write_str("..")?;
        self.0.end.fmt(formatter)
    }
}

impl PartialEq<str> for SourceText {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for SourceText {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

fn make_scannable(bytes: &[u8]) -> (String, Vec<Range<usize>>) {
    let mut scannable = String::with_capacity(bytes.len());
    let mut invalid = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        match std::str::from_utf8(&bytes[offset..]) {
            Ok(valid) => {
                scannable.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_end = offset + error.valid_up_to();
                // SAFETY is unnecessary: `valid_up_to` guarantees this prefix.
                scannable.push_str(std::str::from_utf8(&bytes[offset..valid_end]).unwrap());
                let invalid_len = error.error_len().unwrap_or(bytes.len() - valid_end);
                let invalid_end = valid_end + invalid_len;
                invalid.push(valid_end..invalid_end);
                scannable.extend(std::iter::repeat_n('\u{7f}', invalid_len));
                offset = invalid_end;
            }
        }
    }
    debug_assert_eq!(scannable.len(), bytes.len());
    (scannable, invalid)
}

/// A JavaScript string represented as UTF-16 code units. Unlike Rust `String`,
/// this preserves lone surrogates produced by escape sequences.
#[derive(Clone, Default, Eq, Hash, PartialEq)]
pub struct JsString(Vec<u16>);

impl JsString {
    #[must_use]
    pub fn from_utf8(value: &str) -> Self {
        Self(value.encode_utf16().collect())
    }

    #[must_use]
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }

    #[must_use]
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub fn push_char(&mut self, ch: char) {
        let mut units = [0; 2];
        self.0.extend_from_slice(ch.encode_utf16(&mut units));
    }

    pub fn push_unit(&mut self, unit: u16) {
        self.0.push(unit);
    }

    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        char::decode_utf16(self.0.iter().copied())
            .map(|result| result.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
}

impl fmt::Debug for JsString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("JsString").field(&self.0).finish()
    }
}

/// A source position measured in UTF-8 bytes, matching the native compiler.
#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct TextPos(u32);

impl TextPos {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for TextPos {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A half-open source range measured in UTF-8 bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextRange {
    pub start: TextPos,
    pub end: TextPos,
}

impl TextRange {
    #[must_use]
    pub const fn new(start: TextPos, end: TextPos) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.get() - self.start.get()
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start.get() == self.end.get()
    }
}

/// A compiler diagnostic independent of rendering and localization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub code: Option<u32>,
    pub category: DiagnosticCategory,
    pub message: String,
}

/// TypeScript's diagnostic severity categories.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum DiagnosticCategory {
    Warning = 0,
    Error = 1,
    Suggestion = 2,
    Message = 3,
}

/// Bidirectional mapping between native UTF-8 byte offsets and JavaScript
/// UTF-16 code-unit offsets.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PositionMap {
    entries: Vec<PositionMapEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PositionMapEntry {
    utf8_pos: usize,
    delta: usize,
}

impl PositionMap {
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut entries = Vec::new();
        let mut delta = 0;
        for (start, ch) in text.char_indices() {
            let utf8_size = ch.len_utf8();
            let utf16_size = ch.len_utf16();
            if utf8_size != utf16_size {
                delta += utf8_size - utf16_size;
                entries.push(PositionMapEntry {
                    utf8_pos: start + utf8_size,
                    delta,
                });
            }
        }
        Self { entries }
    }

    #[must_use]
    pub fn is_ascii_only(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn utf8_to_utf16(&self, utf8_offset: usize) -> usize {
        let index = self
            .entries
            .partition_point(|entry| entry.utf8_pos <= utf8_offset);
        index
            .checked_sub(1)
            .map_or(utf8_offset, |index| utf8_offset - self.entries[index].delta)
    }

    #[must_use]
    pub fn utf16_to_utf8(&self, utf16_offset: usize) -> usize {
        let index = self
            .entries
            .partition_point(|entry| entry.utf8_pos - entry.delta <= utf16_offset);
        index.checked_sub(1).map_or(utf16_offset, |index| {
            utf16_offset + self.entries[index].delta
        })
    }
}

impl Diagnostic {
    #[must_use]
    pub fn new(range: TextRange, message: impl Into<String>) -> Self {
        Self {
            range,
            code: None,
            category: DiagnosticCategory::Error,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn typescript(
        range: TextRange,
        code: u32,
        category: DiagnosticCategory,
        message: impl Into<String>,
    ) -> Self {
        Self {
            range,
            code: Some(code),
            category,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Diagnostic, DiagnosticCategory, JsString, PositionMap, RangesDebug, SourceText, TextPos,
        TextRange,
    };
    use std::ops::Range;

    #[test]
    fn diagnostics_support_legacy_and_typescript_metadata() {
        let range = TextRange::new(TextPos::new(1), TextPos::new(2));
        let legacy = Diagnostic::new(range, "legacy");
        assert_eq!(legacy.code, None);
        assert_eq!(legacy.category, DiagnosticCategory::Error);

        let structured =
            Diagnostic::typescript(range, 9999, DiagnosticCategory::Warning, "structured");
        assert_eq!(structured.code, Some(9999));
        assert_eq!(structured.category, DiagnosticCategory::Warning);
    }

    #[test]
    fn source_text_preserves_invalid_bytes_and_offsets() {
        let source = SourceText::from_bytes(vec![b'a', 0x80, b'b', 0xf0, 0x9f]);
        assert_eq!(source.as_bytes(), &[b'a', 0x80, b'b', 0xf0, 0x9f]);
        assert_eq!(source.as_scannable_str().as_bytes(), b"a\x7fb\x7f\x7f");
        assert_eq!(source.invalid_byte_ranges(), &[1..2, 3..5]);
        assert_eq!(source.as_scannable_str().len(), source.len());
        assert!(source.as_str().is_none());
    }

    #[test]
    fn source_text_debug_prints_ranges_as_std() {
        let ranges: [Range<usize>; 3] = [1..2, 3..5, 10..usize::MAX];
        let ours = RangesDebug(&ranges);
        assert_eq!(format!("{ours:?}"), format!("{:?}", &ranges[..]));
        assert_eq!(format!("{ours:#?}"), format!("{:#?}", &ranges[..]));
        assert_eq!(format!("{ours:x?}"), format!("{:x?}", &ranges[..]));
        assert_eq!(format!("{ours:#X?}"), format!("{:#X?}", &ranges[..]));
        assert_eq!(format!("{ours:5?}"), format!("{:5?}", &ranges[..]));
        let none = RangesDebug(&[]);
        assert_eq!(
            format!("{none:#?}"),
            format!("{:#?}", &[] as &[Range<usize>])
        );
        let source = SourceText::from_bytes(vec![b'a', 0x80, b'b', 0xf0, 0x9f]);
        let pretty = format!("{source:#?}");
        assert!(pretty.contains("invalid_byte_ranges: [\n        1..2,\n        3..5,\n    ],"));
        assert!(format!("{source:?}").ends_with("invalid_byte_ranges: [1..2, 3..5] }"));
    }

    #[test]
    fn ranges_are_half_open() {
        let range = TextRange::new(TextPos::new(2), TextPos::new(7));
        assert_eq!(range.len(), 5);
        assert!(!range.is_empty());
    }

    #[test]
    fn maps_ascii_without_allocated_entries() {
        let map = PositionMap::new("const answer = 42");
        assert!(map.is_ascii_only());
        assert_eq!(map.utf8_to_utf16(10), 10);
        assert_eq!(map.utf16_to_utf8(10), 10);
    }

    #[test]
    fn maps_bmp_and_astral_characters() {
        let text = "aé😀b";
        let map = PositionMap::new(text);
        assert!(!map.is_ascii_only());
        assert_eq!(map.utf8_to_utf16(text.find('b').unwrap()), 4);
        assert_eq!(map.utf16_to_utf8(4), text.find('b').unwrap());
        for utf16_offset in 0..=5 {
            assert_eq!(
                map.utf8_to_utf16(map.utf16_to_utf8(utf16_offset)),
                utf16_offset
            );
        }
    }

    #[test]
    fn javascript_strings_preserve_lone_surrogates() {
        let value = JsString::from_units(vec![0xd83e, 0xdd80, 0xd800]);
        assert_eq!(value.as_units(), &[0xd83e, 0xdd80, 0xd800]);
        assert_eq!(value.to_string_lossy(), "🦀�");
    }
}
