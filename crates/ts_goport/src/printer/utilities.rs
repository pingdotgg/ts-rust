//! Port of `printer/utilities.go`.

use crate::prelude::*;

use crate::flags_macros::go_flags;
use crate::printer::emit_context::EmitContext;
use crate::printer::text_writer::get_default_indent_size;

// Go: printer/utilities.go:18 getLiteralTextFlags
go_flags!(GetLiteralTextFlags, i32 {
    NONE = 0; // getLiteralTextFlagsNone
    NEVER_ASCII_ESCAPE = 1 << 0; // getLiteralTextFlagsNeverAsciiEscape
    JSX_ATTRIBUTE_ESCAPE = 1 << 1; // getLiteralTextFlagsJsxAttributeEscape
    TERMINATE_UNTERMINATED_LITERALS = 1 << 2; // getLiteralTextFlagsTerminateUnterminatedLiterals
    ALLOW_NUMERIC_SEPARATOR = 1 << 3; // getLiteralTextFlagsAllowNumericSeparator
});

// Go: printer/utilities.go:28 QuoteChar
/// Go `type QuoteChar rune`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QuoteChar(pub char);

impl QuoteChar {
    pub const SINGLE_QUOTE: Self = Self('\''); // QuoteCharSingleQuote
    pub const DOUBLE_QUOTE: Self = Self('"'); // QuoteCharDoubleQuote
    pub const BACKTICK: Self = Self('`'); // QuoteCharBacktick
}

// Go: printer/utilities.go:36 jsxEscapedCharsMap
// PORT: the Go map becomes a match.
fn jsx_escaped_chars_map(ch: char) -> Option<&'static str> {
    match ch {
        '"' => Some("&quot;"),
        '\'' => Some("&apos;"),
        _ => None,
    }
}

// Go: printer/utilities.go:41 escapedCharsMap
// PORT: the Go map becomes a match.
fn escaped_chars_map(ch: char) -> Option<&'static str> {
    Some(match ch {
        '\t' => "\\t",
        '\u{000B}' => "\\v",
        '\u{000C}' => "\\f",
        '\u{0008}' => "\\b",
        '\r' => "\\r",
        '\n' => "\\n",
        '\\' => "\\\\",
        '"' => "\\\"",
        '\'' => "\\'",
        '`' => "\\`",
        '$' => "\\$",            // when quoteChar == '`'
        '\u{2028}' => "\\u2028", // lineSeparator
        '\u{2029}' => "\\u2029", // paragraphSeparator
        '\u{0085}' => "\\u0085", // nextLine
        _ => return None,
    })
}

// Go: printer/utilities.go:58 encodeJsxCharacterEntity
fn encode_jsx_character_entity(b: &mut String, char_code: u32) {
    let hex_char_code = format!("{char_code:X}");
    b.push_str("&#x");
    b.push_str(&hex_char_code);
    b.push(';');
}

// Go: printer/utilities.go:65 encodeUtf16EscapeSequence
fn encode_utf16_escape_sequence(b: &mut String, char_code: u32) {
    let hex_char_code = format!("{char_code:X}");
    b.push_str("\\u");
    for _ in hex_char_code.len()..4 {
        b.push('0');
    }
    b.push_str(&hex_char_code);
}

// Go: printer/utilities.go:77 escapeStringWorker
// Based heavily on the abstract 'Quote'/'QuoteJSONString' operation from ECMA-262 (24.3.2.2),
// but augmented for a few select characters (e.g. lineSeparator, paragraphSeparator, nextLine)
// Note that this doesn't actually wrap the input in double quotes.
// PORT: `s` is the port form of the Go string (see
// `scanner_util::GO_STRING_MARKER`). `decode_go_js_string_rune` reads one
// unit and also gives its Go size, so the invalid byte check is Go's. A lone
// surrogate unit prints as `\uD800` like Go. Text that this worker copies
// keeps the port form, and the file write writes its Go bytes. `ch` is the
// `char` form of `code`. For a surrogate it is U+FFFD, which matches no `ch`
// case, the same as the surrogate rune in Go.
fn escape_string_worker(
    s: &str,
    quote_char: QuoteChar,
    flags: GetLiteralTextFlags,
    b: &mut String,
) {
    // PERF: most strings are only bytes that the loop copies as they are,
    // so the loop starts at the first other byte. The bytes before it are
    // copied with the rest of the run (effect d.ts emit: 3.2 ms of the
    // slowest checker's d.ts part, 5.1 ms on another).
    let start = s
        .bytes()
        .position(|byte| !copies_as_is(byte, quote_char))
        .unwrap_or(s.len());
    escape_string_worker_from(s, quote_char, flags, b, start);
}

/// True when the loop of `escape_string_worker` copies `byte` as it is,
/// whatever the flags and the bytes around it: printable ASCII (and DEL)
/// other than a backslash, the quote char and, in a template, `$`.
fn copies_as_is(byte: u8, quote_char: QuoteChar) -> bool {
    (0x20..0x80).contains(&byte)
        && byte != b'\\'
        && char::from(byte) != quote_char.0
        && !(byte == b'$' && quote_char == QuoteChar::BACKTICK)
}

/// The loop of Go `escapeStringWorker` from byte `start` of `s` on. Every
/// byte before `start` must be one that it copies as it is
/// (`copies_as_is`): the loop would only step over it.
fn escape_string_worker_from(
    s: &str,
    quote_char: QuoteChar,
    flags: GetLiteralTextFlags,
    b: &mut String,
    start: usize,
) {
    let bytes = s.as_bytes();
    debug_assert!(
        bytes[..start]
            .iter()
            .all(|&byte| copies_as_is(byte, quote_char)),
        "escape_string_worker_from: a byte before start needs the loop"
    );
    let mut pos = 0usize;
    let mut i = start;
    while i < s.len() {
        // PORT: an ASCII byte is its own code with size 1. The Go string
        // marker is not ASCII, so marked units still go through the decoder.
        let (code, mut size, go_size) = if bytes[i] < 0x80 {
            let fast = (u32::from(bytes[i]), 1, 1);
            debug_assert_eq!(fast, decode_go_js_string_rune(&s[i..]));
            fast
        } else {
            decode_go_js_string_rune(&s[i..])
        };
        let invalid_byte = code == char::REPLACEMENT_CHARACTER as u32 && go_size == 1;
        let ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);

        let mut escape = false;
        if (0xD800..=0xDFFF).contains(&code) {
            escape = true;
        } else if invalid_byte {
            // A stray byte that is not valid UTF-8 (for example, a fragment of a
            // surrogate sentinel left behind by code that sliced the string by
            // byte). Escape it as the Unicode replacement character so the output
            // is always well-formed rather than containing raw invalid bytes.
            escape = true;
        }

        // This consists of the first 19 unprintable ASCII characters, canonical escapes, lineSeparator,
        // paragraphSeparator, and nextLine. The latter three are just desirable to suppress new lines in
        // the language service. These characters should be escaped when printing, and if any characters are added,
        // `escapedCharsMap` and/or `jsxEscapedCharsMap` must be updated. Note that this *does not* include the 'delete'
        // character. There is no reason for this other than that JSON.stringify does not handle it either.
        // PORT: the Go switch becomes an if chain in the same case order.
        if ch == '\\' {
            if !flags.intersects(GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE) {
                escape = true;
            }
        } else if ch == '$' {
            if quote_char == QuoteChar::BACKTICK && i + 1 < s.len() && bytes[i + 1] == b'{' {
                escape = true;
            }
        } else if ch == quote_char.0
            || ch == '\u{2028}'
            || ch == '\u{2029}'
            || ch == '\u{0085}'
            || ch == '\r'
        {
            escape = true;
        } else if ch == '\n' {
            if quote_char != QuoteChar::BACKTICK {
                // Template strings preserve simple LF newlines, still encode CRLF (or CR).
                escape = true;
            }
        } else if code <= 0x1f
            || !flags.intersects(GetLiteralTextFlags::NEVER_ASCII_ESCAPE) && code > 0x7f
        {
            escape = true;
        }

        if escape {
            if pos < i {
                // Write string up to this point
                b.push_str(&s[pos..i]);
            }

            if flags.intersects(GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE) {
                if code == 0 {
                    b.push_str("&#0;");
                } else if let Some(m) = jsx_escaped_chars_map(ch) {
                    b.push_str(m);
                } else {
                    encode_jsx_character_entity(b, code);
                }
            } else if ch == '\r'
                && quote_char == QuoteChar::BACKTICK
                && i + 1 < s.len()
                && bytes[i + 1] == b'\n'
            {
                // Template strings preserve simple LF newlines, but still must escape CRLF. Left alone, the
                // above cases for `\r` and `\n` would inadvertently escape CRLF as two independent characters.
                size += 1;
                b.push_str("\\r\\n");
            } else if code > 0xffff {
                // encode as surrogate pair
                let c = code - 0x10000;
                encode_utf16_escape_sequence(b, ((c & 0b1111_1111_1100_0000_0000) >> 10) + 0xD800);
                encode_utf16_escape_sequence(b, (c & 0b0000_0000_0011_1111_1111) + 0xDC00);
            } else if (0xD800..=0xDFFF).contains(&code) {
                encode_utf16_escape_sequence(b, code);
            } else if code == 0 {
                if i + 1 < s.len() && is_digit(char::from(bytes[i + 1])) {
                    // If the null character is followed by digits, print as a hex escape to prevent the result from
                    // parsing as an octal (which is forbidden in strict mode)
                    b.push_str("\\x00");
                } else {
                    // Otherwise, keep printing a literal \0 for the null character
                    b.push_str("\\0");
                }
            } else if let Some(m) = escaped_chars_map(ch) {
                b.push_str(m);
            } else {
                encode_utf16_escape_sequence(b, code);
            }
            pos = i + size;
        }

        i += size;
    }

    if pos < i {
        b.push_str(&s[pos..]);
    }
}

// Go: printer/utilities.go:178 EscapeString
pub fn escape_string(s: &str, quote_char: QuoteChar) -> String {
    let mut b = String::with_capacity(s.len() + 2);
    escape_string_worker(
        s,
        quote_char,
        GetLiteralTextFlags::NEVER_ASCII_ESCAPE,
        &mut b,
    );
    b
}

// Go: printer/utilities.go:185 escapeNonAsciiString
pub(crate) fn escape_non_ascii_string(s: &str, quote_char: QuoteChar) -> String {
    let mut b = String::with_capacity(s.len() + 2);
    escape_string_worker(s, quote_char, GetLiteralTextFlags::NONE, &mut b);
    b
}

// Go: printer/utilities.go:192 escapeJsxAttributeString
pub(crate) fn escape_jsx_attribute_string(s: &str, quote_char: QuoteChar) -> String {
    let mut b = String::with_capacity(s.len() + 2);
    escape_string_worker(
        s,
        quote_char,
        GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE | GetLiteralTextFlags::NEVER_ASCII_ESCAPE,
        &mut b,
    );
    b
}

// Go: printer/utilities.go:199 canUseOriginalText
fn can_use_original_text(node: Node, flags: GetLiteralTextFlags) -> bool {
    // A synthetic node has no original text, nor does a node without a parent as we would be unable to find the
    // containing SourceFile. We also cannot use the original text if the literal was unterminated and the caller has
    // requested proper termination of unterminated literals
    if node_is_synthesized(node)
        || node.parent().is_nil()
        || flags.intersects(GetLiteralTextFlags::TERMINATE_UNTERMINATED_LITERALS)
            && is_unterminated_literal(node)
    {
        return false;
    }

    if node.kind() == SyntaxKind::NumericLiteral {
        let token_flags = node.token_flags();
        // For a numeric literal, we cannot use the original text if the original text was an invalid literal
        if token_flags.intersects(TokenFlags::IS_INVALID) {
            return false;
        }
        // We also cannot use the original text if the literal contains numeric separators, but numeric separators
        // are not permitted
        if token_flags.intersects(TokenFlags::CONTAINS_SEPARATOR) {
            return flags.intersects(GetLiteralTextFlags::ALLOW_NUMERIC_SEPARATOR);
        }
    }

    // Finally, we do not use the original text of a BigInt literal
    // TODO(rbuckton): The reason as to why we do not use the original text for bigints is not mentioned in the
    // original compiler source. It could be that this is no longer necessary, in which case bigint literals should
    // use the same code path as numeric literals, above
    node.kind() != SyntaxKind::BigIntLiteral
}

// Go: printer/utilities.go:227 getLiteralText
pub(crate) fn get_literal_text(
    node: Node,
    source_file: Node,
    flags: GetLiteralTextFlags,
) -> String {
    // If we don't need to downlevel and we can reach the original source text using
    // the node's parent reference, then simply get the text as it was originally written.
    if source_file.is_some() && can_use_original_text(node, flags) {
        return get_source_text_of_node_from_source_file(
            source_file,
            node,
            false, /*includeTrivia*/
        );
    }

    // If we can't reach the original source text, use the canonical form if it's a number,
    // or a (possibly escaped) quoted form of the original text if it's string-like.
    match node.kind() {
        SyntaxKind::StringLiteral => {
            let quote_char = if node.token_flags().intersects(TokenFlags::SINGLE_QUOTE) {
                QuoteChar::SINGLE_QUOTE
            } else {
                QuoteChar::DOUBLE_QUOTE
            };

            let text = node.text();

            // Write leading quote character
            let mut b = String::with_capacity(text.len() + 2);
            b.push(quote_char.0);

            // Write text
            escape_string_worker(text, quote_char, flags, &mut b);

            // Write trailing quote character
            b.push(quote_char.0);
            b
        }

        SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::TemplateHead
        | SyntaxKind::TemplateMiddle
        | SyntaxKind::TemplateTail => {
            // If a NoSubstitutionTemplateLiteral appears to have a substitution in it, the original text
            // had to include a backslash: `not \${a} substitution`.
            let text = node.text();
            let raw_text = node.raw_text();
            let raw = !raw_text.is_empty() || text.is_empty();

            let text_len = if raw { raw_text.len() } else { text.len() };

            // Write leading quote character
            let mut b = String::new();
            match node.kind() {
                SyntaxKind::NoSubstitutionTemplateLiteral => {
                    b.reserve(2 + text_len);
                    b.push('`');
                }
                SyntaxKind::TemplateHead => {
                    b.reserve(3 + text_len);
                    b.push('`');
                }
                SyntaxKind::TemplateMiddle => {
                    b.reserve(3 + text_len);
                    b.push('}');
                }
                SyntaxKind::TemplateTail => {
                    b.reserve(2 + text_len);
                    b.push('}');
                }
                _ => {}
            }

            // Write text
            if !raw_text.is_empty() || text.is_empty() {
                // If rawText is set, it is expected to be valid.
                b.push_str(raw_text);
            } else {
                escape_string_worker(text, QuoteChar::BACKTICK, flags, &mut b);
            }

            // Write trailing quote character
            match node.kind() {
                SyntaxKind::NoSubstitutionTemplateLiteral => b.push('`'),
                SyntaxKind::TemplateHead => b.push_str("${"),
                SyntaxKind::TemplateMiddle => b.push_str("${"),
                SyntaxKind::TemplateTail => b.push('`'),
                _ => {}
            }
            b
        }

        SyntaxKind::NumericLiteral | SyntaxKind::BigIntLiteral => node.text().to_string(),

        SyntaxKind::RegularExpressionLiteral => {
            if flags.intersects(GetLiteralTextFlags::TERMINATE_UNTERMINATED_LITERALS)
                && is_unterminated_literal(node)
            {
                let text = node.text();
                let mut b;
                if !text.is_empty() && text.as_bytes()[text.len() - 1] == b'\\' {
                    b = String::with_capacity(2 + text.len());
                    b.push_str(text);
                    b.push_str(" /");
                } else {
                    b = String::with_capacity(1 + text.len());
                    b.push_str(text);
                    b.push('/');
                }
                return b;
            }
            node.text().to_string()
        }

        _ => panic!("Unsupported LiteralLikeNode"),
    }
}

// Go: printer/utilities.go:341 isNotPrologueDirective
pub(crate) fn is_not_prologue_directive(node: Node) -> bool {
    !is_prologue_directive(node)
}

// Go: printer/utilities.go:345 RangeIsOnSingleLine
pub fn range_is_on_single_line(r: TextRange, source_file: Node) -> bool {
    range_start_is_on_same_line_as_range_end(r, r, source_file)
}

// Go: printer/utilities.go:349 RangeStartPositionsAreOnSameLine
pub fn range_start_positions_are_on_same_line(
    range1: TextRange,
    range2: TextRange,
    source_file: Node,
) -> bool {
    positions_are_on_same_line(
        get_start_position_of_range(range1, source_file, false /*includeComments*/),
        get_start_position_of_range(range2, source_file, false /*includeComments*/),
        source_file,
    )
}

// Go: printer/utilities.go:357 rangeEndPositionsAreOnSameLine
pub(crate) fn range_end_positions_are_on_same_line(
    range1: TextRange,
    range2: TextRange,
    source_file: Node,
) -> bool {
    positions_are_on_same_line(range1.end(), range2.end(), source_file)
}

// Go: printer/utilities.go:361 rangeStartIsOnSameLineAsRangeEnd
pub(crate) fn range_start_is_on_same_line_as_range_end(
    range1: TextRange,
    range2: TextRange,
    source_file: Node,
) -> bool {
    positions_are_on_same_line(
        get_start_position_of_range(range1, source_file, false /*includeComments*/),
        range2.end(),
        source_file,
    )
}

// Go: printer/utilities.go:365 rangeEndIsOnSameLineAsRangeStart
pub(crate) fn range_end_is_on_same_line_as_range_start(
    range1: TextRange,
    range2: TextRange,
    source_file: Node,
) -> bool {
    positions_are_on_same_line(
        range1.end(),
        get_start_position_of_range(range2, source_file, false /*includeComments*/),
        source_file,
    )
}

// Go: printer/utilities.go:369 getStartPositionOfRange
pub(crate) fn get_start_position_of_range(
    r: TextRange,
    source_file: Node,
    include_comments: bool,
) -> i32 {
    if position_is_synthesized(r.pos()) {
        return -1;
    }
    skip_trivia_ex(
        &source_file_text(source_file),
        r.pos(),
        Some(&SkipTriviaOptions {
            stop_at_comments: include_comments,
            ..Default::default()
        }),
    )
}

// Go: printer/utilities.go:376 PositionsAreOnSameLine
pub fn positions_are_on_same_line(pos1: i32, pos2: i32, source_file: Node) -> bool {
    get_lines_between_positions(source_file, pos1, pos2) == 0
}

// Go: printer/utilities.go:380 GetLinesBetweenPositions
pub fn get_lines_between_positions(source_file: Node, pos1: i32, pos2: i32) -> i32 {
    if pos1 == pos2 {
        return 0;
    }
    let line_starts = &*get_ecma_line_starts(source_file);
    let lower = if pos1 < pos2 { pos1 } else { pos2 };
    let is_negative = lower == pos2;
    let upper = if is_negative { pos1 } else { pos2 };
    let lower_line = compute_line_of_position(line_starts, lower);
    let upper_line =
        lower_line + compute_line_of_position(&line_starts[lower_line as usize..], upper);
    if is_negative {
        lower_line - upper_line
    } else {
        upper_line - lower_line
    }
}

// Go: printer/utilities.go:397 getLinesBetweenRangeEndAndRangeStart
pub(crate) fn get_lines_between_range_end_and_range_start(
    range1: TextRange,
    range2: TextRange,
    source_file: Node,
    include_second_range_comments: bool,
) -> i32 {
    let range2_start =
        get_start_position_of_range(range2, source_file, include_second_range_comments);
    get_lines_between_positions(source_file, range1.end(), range2_start)
}

// Go: printer/utilities.go:402 getLinesBetweenPositionAndPrecedingNonWhitespaceCharacter
pub(crate) fn get_lines_between_position_and_preceding_non_whitespace_character(
    pos: i32,
    stop_pos: i32,
    source_file: Node,
    include_comments: bool,
) -> i32 {
    let start_pos = skip_trivia_ex(
        &source_file_text(source_file),
        pos,
        Some(&SkipTriviaOptions {
            stop_at_comments: include_comments,
            ..Default::default()
        }),
    );
    let prev_pos = get_previous_non_whitespace_position(start_pos, stop_pos, source_file);
    get_lines_between_positions(
        source_file,
        if prev_pos >= 0 { prev_pos } else { stop_pos },
        start_pos,
    )
}

// Go: printer/utilities.go:408 getLinesBetweenPositionAndNextNonWhitespaceCharacter
pub(crate) fn get_lines_between_position_and_next_non_whitespace_character(
    pos: i32,
    stop_pos: i32,
    source_file: Node,
    include_comments: bool,
) -> i32 {
    let next_pos = skip_trivia_ex(
        &source_file_text(source_file),
        pos,
        Some(&SkipTriviaOptions {
            stop_at_comments: include_comments,
            ..Default::default()
        }),
    );
    get_lines_between_positions(
        source_file,
        pos,
        if stop_pos < next_pos {
            stop_pos
        } else {
            next_pos
        },
    )
}

// Go: printer/utilities.go:413 getPreviousNonWhitespacePosition
fn get_previous_non_whitespace_position(mut pos: i32, stop_pos: i32, source_file: Node) -> i32 {
    let text_text = source_file_text(source_file);
    let text = text_text.as_bytes();
    while pos >= stop_pos {
        // PORT: Go `rune(text[pos])` converts one byte, like `char::from(u8)`.
        if !is_white_space_like(char::from(text[pos as usize])) {
            return pos;
        }
        pos -= 1;
    }
    -1
}

// Go: printer/utilities.go:422 siblingNodePositionsAreComparable
pub(crate) fn sibling_node_positions_are_comparable(
    emit_context: &EmitContext,
    previous_node: Node,
    next_node: Node,
) -> bool {
    if next_node.pos() < previous_node.end() {
        return false;
    }

    let previous_node = emit_context.most_original(previous_node);
    let next_node = emit_context.most_original(next_node);
    let parent = previous_node.parent();
    if parent.is_nil() || parent != next_node.parent() {
        return false;
    }

    let parent_node_array = get_containing_node_array(previous_node);
    if parent_node_array.is_some() {
        let nodes = parent_node_array.nodes();
        let prev_node_index = nodes.iter().position(|n| n == previous_node);
        return match prev_node_index {
            Some(prev) => nodes.iter().position(|n| n == next_node) == Some(prev + 1),
            None => false,
        };
    }

    false
}

// Go: printer/utilities.go:443 getContainingNodeArray
pub(crate) fn get_containing_node_array(node: Node) -> NodeList {
    let parent = node.parent();
    if parent.is_nil() {
        return NodeList::NIL;
    }

    match node.kind() {
        SyntaxKind::TypeParameter => {
            if is_function_like(parent)
                || is_class_like(parent)
                || is_interface_declaration(parent)
                || is_type_or_js_type_alias_declaration(parent)
            {
                return parent.type_parameter_list();
            } else if is_infer_type_node(parent) {
                // infer type nodes have no associated type parameter list
            } else {
                // Go `%#v` of a Kind (an int16 with no `GoString` method) is
                // the number, which `SyntaxKind` keeps.
                panic!("Unexpected TypeParameter parent: {}", parent.kind() as i16);
            }
        }

        SyntaxKind::Parameter => return node.parent().parameter_list(),
        SyntaxKind::TemplateLiteralTypeSpan => return node.parent().template_spans(),
        SyntaxKind::TemplateSpan => return node.parent().template_spans(),
        SyntaxKind::Decorator => {
            if can_have_decorators(node.parent()) {
                let modifiers = node.parent().modifiers();
                if modifiers.is_some() {
                    return modifiers.node_list();
                }
            }
            return NodeList::NIL;
        }
        SyntaxKind::HeritageClause => {
            // PORT: Go reads ClassLikeData().HeritageClauses or
            // AsInterfaceDeclaration().HeritageClauses; one accessor covers both.
            return node.parent().heritage_clauses();
        }
        _ => {}
    }

    // TODO(rbuckton)
    // if ast.IsJSDocTag(node) {
    //     if ast.IsJSDocTypeLiteral(node.parent) {
    // 		return nil
    // 	 }
    // 	 return node.parent.tags
    // }

    match parent.kind() {
        SyntaxKind::TypeLiteral | SyntaxKind::InterfaceDeclaration => {
            if is_type_element(node) {
                return parent.member_list();
            }
        }
        SyntaxKind::UnionType => return parent.types(),
        SyntaxKind::IntersectionType => return parent.types(),
        SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::TupleType
        | SyntaxKind::NamedImports
        | SyntaxKind::NamedExports => return parent.element_list(),
        SyntaxKind::ObjectLiteralExpression | SyntaxKind::JsxAttributes => {
            return parent.property_list();
        }
        SyntaxKind::CallExpression | SyntaxKind::NewExpression => {
            // PORT: Go has one case each for CallExpression and NewExpression
            // with the same body.
            if is_type_node(node) {
                return parent.type_argument_list();
            } else if node != parent.expression() {
                return parent.argument_list();
            }
        }
        SyntaxKind::JsxElement | SyntaxKind::JsxFragment => {
            if is_jsx_child(node) {
                return parent.children();
            }
        }
        SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => {
            if is_type_node(node) {
                return parent.type_argument_list();
            }
        }
        SyntaxKind::Block
        | SyntaxKind::ModuleBlock
        | SyntaxKind::CaseClause
        | SyntaxKind::DefaultClause => {
            return parent.statement_list();
        }
        SyntaxKind::CaseBlock => return parent.clauses(),
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
            if is_class_element(node) {
                return parent.member_list();
            }
        }
        SyntaxKind::EnumDeclaration => {
            if is_enum_member(node) {
                return parent.member_list();
            }
        }
        SyntaxKind::SourceFile => {
            if is_statement(node) {
                return parent.statement_list();
            }
        }
        _ => {}
    }

    if is_modifier(node) {
        let modifiers = parent.modifiers();
        if modifiers.is_some() {
            return modifiers.node_list();
        }
    }

    NodeList::NIL
}

// Go: printer/utilities.go:553 canHaveDecorators
// PORT: printer-local; differs from ast::can_have_decorators, so it stays
// private to avoid a glob-import clash.
fn can_have_decorators(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::ClassExpression
            | SyntaxKind::ClassDeclaration
    )
}

// Go: printer/utilities.go:567 originalNodesHaveSameParent
pub(crate) fn original_nodes_have_same_parent(
    emit_context: &EmitContext,
    node_a: Node,
    node_b: Node,
) -> bool {
    let node_a = emit_context.most_original(node_a);
    if node_a.parent().is_some() {
        // For performance, do not call `MostOriginal` for `nodeB` if `nodeA` doesn't even
        // have a parent node.
        let node_b = emit_context.most_original(node_b);
        return node_a.parent() == node_b.parent();
    }
    false
}

/// Go `interface{ End() int }` as used by `tryGetEnd` and `greatestEnd`.
// PORT: Go switches on the dynamic type and panics on other types; here the
// trait limits callers at compile time. Go `*core.TextRange` is
// `Option<TextRange>`.
pub(crate) trait EndLike {
    fn try_get_end(&self) -> (i32, bool);
}

impl EndLike for Node {
    fn try_get_end(&self) -> (i32, bool) {
        if self.is_some() {
            (self.end(), true)
        } else {
            (0, false)
        }
    }
}

impl EndLike for NodeList {
    fn try_get_end(&self) -> (i32, bool) {
        if self.is_some() {
            (self.end(), true)
        } else {
            (0, false)
        }
    }
}

impl EndLike for ModifierList {
    fn try_get_end(&self) -> (i32, bool) {
        if self.is_some() {
            (self.end(), true)
        } else {
            (0, false)
        }
    }
}

impl EndLike for Option<TextRange> {
    fn try_get_end(&self) -> (i32, bool) {
        match self {
            Some(v) => (v.end(), true),
            None => (0, false),
        }
    }
}

impl EndLike for TextRange {
    fn try_get_end(&self) -> (i32, bool) {
        (self.end(), true)
    }
}

// Go: printer/utilities.go:578 tryGetEnd
pub(crate) fn try_get_end(node: &dyn EndLike) -> (i32, bool) {
    // avoid using reflect (via core.IsNil) for common cases
    node.try_get_end()
}

// Go: printer/utilities.go:605 greatestEnd
pub(crate) fn greatest_end(mut end: i32, nodes: &[&dyn EndLike]) -> i32 {
    for i in (0..nodes.len()).rev() {
        let node = nodes[i];
        let (node_end, ok) = try_get_end(node);
        if ok && end < node_end {
            end = node_end;
        }
    }
    end
}

// Go: printer/utilities.go:614 skipSynthesizedParentheses
pub(crate) fn skip_synthesized_parentheses(mut node: Node) -> Node {
    while node.kind() == SyntaxKind::ParenthesizedExpression && node_is_synthesized(node) {
        node = node.expression();
    }
    node
}

// Go: printer/utilities.go:621 isNewExpressionWithoutArguments
pub(crate) fn is_new_expression_without_arguments(node: Node) -> bool {
    node.kind() == SyntaxKind::NewExpression && node.argument_list().is_nil()
}

// Go: printer/utilities.go:625 isBinaryOperation
pub(crate) fn is_binary_operation(node: Node, token: SyntaxKind) -> bool {
    let node = skip_partially_emitted_expressions(node);
    node.kind() == SyntaxKind::BinaryExpression && node.operator_token().kind() == token
}

// Go: printer/utilities.go:631 mixingBinaryOperatorsRequiresParentheses
pub(crate) fn mixing_binary_operators_requires_parentheses(a: SyntaxKind, b: SyntaxKind) -> bool {
    if a == SyntaxKind::QuestionQuestionToken {
        return b == SyntaxKind::AmpersandAmpersandToken || b == SyntaxKind::BarBarToken;
    }
    if b == SyntaxKind::QuestionQuestionToken {
        return a == SyntaxKind::AmpersandAmpersandToken || a == SyntaxKind::BarBarToken;
    }
    false
}

// Go: printer/utilities.go:641 isImmediatelyInvokedFunctionExpressionOrArrowFunction
pub(crate) fn is_immediately_invoked_function_expression_or_arrow_function(node: Node) -> bool {
    let node = skip_partially_emitted_expressions(node);
    if !is_call_expression(node) {
        return false;
    }
    let node = skip_partially_emitted_expressions(node.expression());
    is_function_expression(node) || is_arrow_function(node)
}

// Go: printer/utilities.go:650 hasLeadingHash
fn has_leading_hash(text: &str) -> bool {
    !text.is_empty() && text.as_bytes()[0] == b'#'
}

// Go: printer/utilities.go:654 removeLeadingHash
pub(crate) fn remove_leading_hash(text: &str) -> &str {
    if has_leading_hash(text) {
        &text[1..]
    } else {
        text
    }
}

// Go: printer/utilities.go:662 ensureLeadingHash
pub(crate) fn ensure_leading_hash(text: &str) -> String {
    if has_leading_hash(text) {
        text.to_string()
    } else {
        format!("#{text}")
    }
}

// Go: printer/utilities.go:670 FormatGeneratedName
pub fn format_generated_name(private_name: bool, prefix: &str, base: &str, suffix: &str) -> String {
    let name = format!(
        "{}{}{}",
        remove_leading_hash(prefix),
        remove_leading_hash(base),
        remove_leading_hash(suffix)
    );
    if private_name {
        return ensure_leading_hash(&name);
    }
    name
}

// Go: printer/utilities.go:678 isASCIIWordCharacter
fn is_ascii_word_character(ch: char) -> bool {
    is_ascii_letter(ch) || is_digit(ch) || ch == '_'
}

// Go: printer/utilities.go:682 makeIdentifierFromModuleName
// PORT: Go reads the bytes of the Go string. `module_name` is a port form
// (see `scanner_util::GO_STRING_MARKER`), so this reads its Go bytes. Each
// byte that is not ASCII becomes '_', so the kept bytes are ASCII.
pub(crate) fn make_identifier_from_module_name(module_name: &str) -> String {
    let module_name = crate::frontend::tspath::get_base_file_name(module_name);
    let bytes = go_string_bytes(&module_name);
    let push_ascii = |builder: &mut String, kept: &[u8]| {
        builder.extend(kept.iter().map(|&b| char::from(b)));
    };
    let mut builder = String::new();
    let mut start = 0usize;
    let mut pos = 0usize;
    while pos < bytes.len() {
        // PORT: Go `rune(moduleName[pos])` converts one byte.
        let ch = char::from(bytes[pos]);
        if pos == 0 && is_digit(ch) {
            builder.push('_');
        } else if !is_ascii_word_character(ch) {
            if start < pos {
                push_ascii(&mut builder, &bytes[start..pos]);
            }
            builder.push('_');
            start = pos + 1;
        }
        pos += 1;
    }
    if start < pos {
        push_ascii(&mut builder, &bytes[start..pos]);
    }
    builder
}

// Go: printer/utilities.go:706 findSpanEndWithEmitContext
pub(crate) fn find_span_end_with_emit_context<T: Copy>(
    c: &EmitContext,
    array: &[T],
    test: impl Fn(&EmitContext, T) -> bool,
    start: i32,
) -> i32 {
    let mut i = start;
    while (i as usize) < array.len() && test(c, array[i as usize]) {
        i += 1;
    }
    i
}

// Go: printer/utilities.go:714 findSpanEnd
pub(crate) fn find_span_end<T: Copy>(array: &[T], test: impl Fn(T) -> bool, start: i32) -> i32 {
    let mut i = start;
    while (i as usize) < array.len() && test(array[i as usize]) {
        i += 1;
    }
    i
}

/// Go `utf8.DecodeRuneInString(text[pos:])`: `(RuneError, 0)` at the end.
// PORT: positions here always fall on char boundaries.
fn decode_rune_in_string(text: &str, pos: usize) -> (char, usize) {
    match text[pos..].chars().next() {
        Some(ch) => (ch, ch.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 0),
    }
}

// Go: printer/utilities.go:722 skipWhiteSpaceSingleLine
// PORT: byte positions are `usize` in these private scanning helpers.
fn skip_white_space_single_line(text: &str, pos: &mut usize) {
    while *pos < text.len() {
        let (ch, size) = decode_rune_in_string(text, *pos);
        if !is_white_space_single_line(ch) {
            break;
        }
        *pos += size;
    }
}

// Go: printer/utilities.go:732 matchWhiteSpaceSingleLine
fn match_white_space_single_line(text: &str, pos: &mut usize) -> bool {
    let start_pos = *pos;
    skip_white_space_single_line(text, pos);
    *pos != start_pos
}

// Go: printer/utilities.go:738 matchRune
fn match_rune(text: &str, pos: &mut usize, expected: char) -> bool {
    let (ch, size) = decode_rune_in_string(text, *pos);
    if ch == expected {
        *pos += size;
        return true;
    }
    false
}

// Go: printer/utilities.go:747 matchString
fn match_string(text: &str, pos: &mut usize, expected: &str) -> bool {
    let mut text_pos = *pos;
    let mut expected_pos = 0usize;
    while expected_pos < expected.len() {
        if text_pos >= text.len() {
            return false;
        }

        let (expected_rune, expected_size) = decode_rune_in_string(expected, expected_pos);
        if !match_rune(text, &mut text_pos, expected_rune) {
            return false;
        }

        expected_pos += expected_size;
    }

    *pos = text_pos;
    true
}

// Go: printer/utilities.go:767 matchQuotedString
fn match_quoted_string(text: &str, pos: &mut usize) -> bool {
    let mut text_pos = *pos;
    let quote_char;
    if match_rune(text, &mut text_pos, '\'') {
        quote_char = '\'';
    } else if match_rune(text, &mut text_pos, '"') {
        quote_char = '"';
    } else {
        return false;
    }
    while text_pos < text.len() {
        let (ch, size) = decode_rune_in_string(text, text_pos);
        text_pos += size;
        if ch == quote_char {
            *pos = text_pos;
            return true;
        }
    }
    false
}

// Go: printer/utilities.go:795 IsRecognizedTripleSlashComment
// /// <reference path="..." />
// /// <reference types="..." />
// /// <reference lib="..." />
// /// <reference no-default-lib="..." />
// /// <amd-dependency path="..." />
// /// <amd-module />
pub fn is_recognized_triple_slash_comment(text: &str, comment_range: CommentRange) -> bool {
    let bytes = text.as_bytes();
    if comment_range.kind == SyntaxKind::SingleLineCommentTrivia
        && comment_range.len() > 2
        && bytes[(comment_range.pos() + 1) as usize] == b'/'
        && bytes[(comment_range.pos() + 2) as usize] == b'/'
    {
        let text = &text[(comment_range.pos() + 3) as usize..comment_range.end() as usize];
        let mut pos = 0usize;
        skip_white_space_single_line(text, &mut pos);
        if !match_rune(text, &mut pos, '<') {
            return false;
        }
        if match_string(text, &mut pos, "reference") {
            if !match_white_space_single_line(text, &mut pos) {
                return false;
            }
            if !match_string(text, &mut pos, "path")
                && !match_string(text, &mut pos, "types")
                && !match_string(text, &mut pos, "lib")
                && !match_string(text, &mut pos, "no-default-lib")
            {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_rune(text, &mut pos, '=') {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_quoted_string(text, &mut pos) {
                return false;
            }
        } else if match_string(text, &mut pos, "amd-dependency") {
            if !match_white_space_single_line(text, &mut pos) {
                return false;
            }
            if !match_string(text, &mut pos, "path") {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_rune(text, &mut pos, '=') {
                return false;
            }
            skip_white_space_single_line(text, &mut pos);
            if !match_quoted_string(text, &mut pos) {
                return false;
            }
        } else if match_string(text, &mut pos, "amd-module") {
            skip_white_space_single_line(text, &mut pos);
        } else {
            return false;
        }
        let index = text[pos..].find("/>");
        return index.is_some();
    }

    false
}

// Go: printer/utilities.go:852 isJSDocLikeText
// PORT: Go `comment.Len()` counts Go bytes (`comment_go_len_at_least`).
// The bytes at `pos+2` and `pos+3` follow the ASCII `/*`: a byte there is
// ASCII in the port form exactly when it is in Go, so the compares agree.
pub(crate) fn is_js_doc_like_text(text: &str, comment: CommentRange) -> bool {
    let bytes = text.as_bytes();
    comment.kind == SyntaxKind::MultiLineCommentTrivia
        && comment_go_len_at_least(text, comment, 5)
        && bytes[(comment.pos() + 2) as usize] == b'*'
        && bytes[(comment.pos() + 3) as usize] != b'/'
}

// Go: printer/utilities.go:859 IsPinnedComment
// PORT: Go `comment.Len() > 5` counts Go bytes (`comment_go_len_at_least`).
pub fn is_pinned_comment(text: &str, comment: CommentRange) -> bool {
    comment.kind == SyntaxKind::MultiLineCommentTrivia
        && comment_go_len_at_least(text, comment, 6)
        && text.as_bytes()[(comment.pos() + 2) as usize] == b'!'
}

/// Reports whether Go `comment.Len()`, the Go bytes of the comment, is at
/// least `n`.
// PORT: a marker unit (see `scanner_util::GO_STRING_MARKER`) has more port
// bytes than Go bytes, at most 7 for 1. A port length below `n` or at least
// `7 * n` decides it; only a short comment counts its Go bytes.
fn comment_go_len_at_least(text: &str, comment: CommentRange, n: i32) -> bool {
    let len = comment.len();
    len >= n
        && (len >= 7 * n
            || crate::scanner_util::go_len(&text[comment.pos() as usize..comment.end() as usize])
                >= n as usize)
}

// Go: printer/utilities.go:865 calculateIndent
pub(crate) fn calculate_indent(text: &str, mut pos: i32, end: i32) -> i32 {
    let mut current_line_indent = 0;
    let indent_size = get_default_indent_size();
    while pos < end {
        let (ch, size) = decode_rune_in_string(text, pos as usize);
        if !is_white_space_single_line(ch) {
            break;
        }
        if ch == '\t' {
            // Tabs = TabSize = indent size and go to next tabStop
            current_line_indent += indent_size - (current_line_indent % indent_size);
        } else {
            // Single space
            current_line_indent += 1;
        }
        pos += size as i32;
    }

    current_line_indent
}

// Go: printer/utilities.go:894 lineCharacterCache
// lineCharacterCache provides cached line/character lookups for a source file,
// optimized for monotonically increasing positions (e.g., during source map emit).
//
// When positions increase within the same line, only the delta between the last
// position and the new position needs to be scanned for UTF-16 code unit counts,
// turning what would be O(n²) into O(n) for long lines.
//
// Character offsets are measured in UTF-16 code units per the source map specification.
pub(crate) struct LineCharacterCache {
    source: LineCharacterSource,
    cached_line: i32,
    cached_pos: i32,
    cached_char: i32,
    has_cached: bool,
}

/// Go `source.ECMALineMap()` and `source.Text()` of a `lineCharacterCache`.
// PORT: a source file node keeps its frozen line map and text. Another
// `sourcemap.Source` (ts#63936) keeps the source and reads both from it.
enum LineCharacterSource {
    File {
        line_map: FileRef<[i32]>,
        text: FileText,
    },
    Other(Rc<dyn crate::sourcemap::source::Source>),
}

// Go: printer/utilities.go:903 newLineCharacterCache
// PORT: a node source must be a parsed source file; the printer passes the
// original file of a transformed one (see `set_source_map_source`).
pub(crate) fn new_line_character_cache(source: &SourceMapSource) -> LineCharacterCache {
    let source = match source {
        SourceMapSource::Node(node) => LineCharacterSource::File {
            line_map: get_ecma_line_starts(*node),
            text: source_file_text(*node),
        },
        SourceMapSource::Other(source) => LineCharacterSource::Other(source.clone()),
    };
    LineCharacterCache {
        source,
        cached_line: 0,
        cached_pos: 0,
        cached_char: 0,
        has_cached: false,
    }
}

impl LineCharacterCache {
    // Go: printer/utilities.go:912 getLineAndCharacter
    // getLineAndCharacter returns the 0-based line number and UTF-16 code unit
    // offset from the start of that line for the given byte position.
    // PORT: `pos` can be inside a char (a skipped token of a parse error can
    // end there). Go slices the bytes and counts each byte of a cut char as
    // one unit (`utf16_len_of_range`). In the port form `pos` can also be
    // inside a marker unit, where it is the Go offset that `go_byte_offset`
    // gives, and the count is of the Go bytes. The count depends on the
    // cache split, as in Go: a char cut at the cached position counts its
    // bytes on both sides.
    pub(crate) fn get_line_and_character(&mut self, pos: i32) -> (i32, i32) {
        let (line_map, text): (&[i32], &str) = match &self.source {
            LineCharacterSource::File { line_map, text } => (&**line_map, &**text),
            LineCharacterSource::Other(source) => (source.ecma_line_map(), source.text()),
        };
        let line = Self::line_of_position(line_map, self.cached_line, pos);
        let line_start = line_map[line as usize];
        // When pos is beyond the source text (e.g., for error-recovery tokens like
        // missing closing braces), we can't slice past the text end. Compute the
        // UTF-16 length up to EOF and add the remaining byte offset arithmetically,
        // matching TypeScript's computeLineAndCharacterOfPosition which uses
        // arithmetic (position - lineStarts[lineNumber]) and handles this implicitly.
        let end_pos = std::cmp::min(pos, text.len() as i32);
        let mut character;
        if self.has_cached && line == self.cached_line && end_pos >= self.cached_pos {
            // Incremental: only count UTF-16 code units from the last cached position.
            character = self.cached_char
                + utf16_len_of_range(text, self.cached_pos as usize, end_pos as usize);
        } else {
            // Full computation from line start.
            character = utf16_len_of_range(text, line_start as usize, end_pos as usize);
        }
        let cached_char = character;
        character += pos - end_pos;
        self.cached_line = line;
        self.cached_pos = end_pos;
        self.cached_char = cached_char;
        self.has_cached = true;
        (line, character)
    }

    // PORT: Go calls ComputeLineOfPosition. Source map positions mostly stay
    // on the cached line or move to the next one, so test those two lines
    // before the binary search. Line starts strictly increase, so a line `l`
    // with `line_map[l] <= pos < line_map[l + 1]` is the binary search result.
    fn line_of_position(map: &[i32], cached_line: i32, pos: i32) -> i32 {
        let holds = |l: usize| {
            map.get(l).is_some_and(|&start| start <= pos)
                && map.get(l + 1).is_none_or(|&next| pos < next)
        };
        let cached = cached_line as usize;
        let line = if holds(cached) {
            cached as i32
        } else if holds(cached + 1) {
            cached as i32 + 1
        } else {
            return compute_line_of_position(map, pos);
        };
        debug_assert_eq!(line, compute_line_of_position(map, pos));
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go prints `%#v` of the parent's Kind in this panic
    // (printer/utilities.go:457 `panic(fmt.Sprintf("Unexpected TypeParameter
    // parent: %#v", parent.Kind))`). Kind is an int16 with no `GoString`
    // method, so Go prints the number: 202 is `KindMappedType` at pin
    // fed0bf24149f (ast/kind_generated.go; 201 before ts#63915 added
    // KindSourceKeyword).
    #[test]
    fn unexpected_type_parameter_parent_panic_prints_the_kind_number() {
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        use std::panic::AssertUnwindSafe;
        let opts = SourceFileParseOptions {
            file_name: "/mapped.ts".to_string(),
            ..Default::default()
        };
        let root = parse_source_file(&opts, "type M = { [K in string]: K };", ScriptKind::TS).root;
        let alias = root.statements().iter().next().expect("the alias");
        let type_parameter = alias.type_().type_parameter();
        let payload = std::panic::catch_unwind(AssertUnwindSafe(|| {
            get_containing_node_array(type_parameter)
        }))
        .expect_err("no panic");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("Unexpected TypeParameter parent: 202")
        );
    }

    /// The escape with the skipped start (`escape_string_worker`) gives the
    /// same text as the Go loop from the first byte, for every quote char,
    /// flag set and ASCII byte, alone and next to the bytes that the loop
    /// reads ahead.
    #[test]
    fn skipped_start_gives_the_same_text() {
        let quotes = [
            QuoteChar::SINGLE_QUOTE,
            QuoteChar::DOUBLE_QUOTE,
            QuoteChar::BACKTICK,
        ];
        let flag_sets = [
            GetLiteralTextFlags::NONE,
            GetLiteralTextFlags::NEVER_ASCII_ESCAPE,
            GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE,
            GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE | GetLiteralTextFlags::NEVER_ASCII_ESCAPE,
        ];
        let mut texts: Vec<String> = [
            "",
            "plain text",
            "a\u{e9}b",
            "a\u{2028}b",
            "\u{1F600}",
            "${x}",
            "a$b",
            "a\r\nb",
            "\u{0}1",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        for byte in 0u8..0x80 {
            let c = char::from(byte);
            texts.push(c.to_string());
            texts.push(format!("ab{c}cd"));
            for next in ['{', '\n', '0', '\u{e9}'] {
                texts.push(format!("x{c}{next}"));
            }
        }
        for text in &texts {
            for quote in quotes {
                for flags in flag_sets {
                    let mut full = String::new();
                    escape_string_worker_from(text, quote, flags, &mut full, 0);
                    let mut skipped = String::new();
                    escape_string_worker(text, quote, flags, &mut skipped);
                    assert_eq!(skipped, full, "{text:?} {quote:?} {flags:?}");
                }
            }
        }
    }

    /// `isJSDocLikeText` and `IsPinnedComment` compare Go `comment.Len()`,
    /// the Go bytes (printer/utilities.go:852 and :859). An invalid byte is
    /// 1 Go byte and 7 port bytes, so `/**` + FF is 4 Go bytes and not JSDoc
    /// like, and `/*!` + FF + `x` is 5 and not pinned. Declaration emit of a
    /// file that ends in `/**` + FF wrote the comment where Go writes none.
    #[test]
    fn comment_checks_count_go_bytes() {
        let comment = |go: &[u8]| {
            let text = crate::scanner_util::go_string_from_bytes(go.to_vec());
            let range = CommentRange {
                text_range: TextRange::new(0, text.len() as i32),
                kind: SyntaxKind::MultiLineCommentTrivia,
                has_trailing_new_line: false,
            };
            (text, range)
        };
        for (go, jsdoc_like, pinned) in [
            (&b"/**\xFF"[..], false, false),
            (b"/**\xFF*", true, false),
            (b"/*!\xFFx", false, false),
            (b"/*!\xFFx*", false, true),
            (b"/*!\xED\xA0\x80", false, true),
            (b"/**\xEF\xB7\x90", true, false),
        ] {
            let (text, range) = comment(go);
            assert_eq!(is_js_doc_like_text(&text, range), jsdoc_like, "{go:?}");
            assert_eq!(is_pinned_comment(&text, range), pinned, "{go:?}");
        }
    }

    /// A text source for `LineCharacterCache`.
    struct TextSource {
        text: String,
        line_map: Vec<i32>,
    }

    impl crate::sourcemap::source::Source for TextSource {
        fn text(&self) -> &str {
            &self.text
        }
        fn file_name(&self) -> &str {
            "parserSkippedTokens16.ts"
        }
        fn ecma_line_map(&self) -> &[i32] {
            &self.line_map
        }
    }

    /// A source map position inside a char counts as in Go
    /// (`printer/utilities.go:912` at pin N): Go slices the bytes from the
    /// cached position, so a cut char counts its bytes on both sides of the
    /// split. The text is sweepN2 case 11133 (`parserSkippedTokens16.ts`).
    /// Line 2 (0-based) starts at byte 36, and its `\u{AC}` is bytes 57 and
    /// 58. Go maps pos 57 to column 21 and pos 58 to column 22.
    #[test]
    fn line_character_cache_counts_cut_chars_as_go() {
        let text = "// @target: es2015\r\nfoo(): Bar { }\r\nfunction Foo      () \u{AC}   { }\r\n4+:5\r\nnamespace M {\r\nfunction a(\r\n    : T) { }\r\n}\r\nvar x       =";
        let source = SourceMapSource::Other(Rc::new(TextSource {
            line_map: compute_ecma_line_starts(text),
            text: text.to_string(),
        }));
        let mut cache = new_line_character_cache(&source);
        // Each call starts after the cached position: [57:58] counts 1 and
        // [58:61] counts 3 (the cut byte and 2 spaces).
        assert_eq!(cache.get_line_and_character(57), (2, 21));
        assert_eq!(cache.get_line_and_character(58), (2, 22));
        // The same position again: the empty range [58:58] counts 0.
        assert_eq!(cache.get_line_and_character(58), (2, 22));
        assert_eq!(cache.get_line_and_character(61), (2, 25));
        // A position before the cached one counts from the line start
        // ([36:56] counts 20). Then [56:61] has the whole char, which
        // counts 1.
        assert_eq!(cache.get_line_and_character(56), (2, 20));
        assert_eq!(cache.get_line_and_character(61), (2, 24));
        let mut fresh = new_line_character_cache(&source);
        assert_eq!(fresh.get_line_and_character(58), (2, 22));
    }
}
