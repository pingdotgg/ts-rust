//! Port of internal/api/encoder/encoder_test.go and decoder_test.go.
//!
//! Baselines: `api/encodeSourceFile.txt` and
//! `api/encodeSourceFileWithUnicodeEscapes.txt`.
//!
//! PORT: Go `*ast.SourceFile` is the SourceFile `Node`. Go `assert.NilError`
//! on a `GoError` result is `expect`. Go `node.AsX()` (which panics on a
//! node of another type) is `as_kind`, then the Rust field accessor. The
//! benchmarks `BenchmarkEncodeSourceFile`, `BenchmarkBuildNodeIndexTable`
//! and `BenchmarkDecodeSourceFile` are not ported.

use super::Subtests;
use crate::support::baseline;
use std::cell::Cell;
use std::rc::Rc;
use ts_goport::api::encoder::{
    HEADER_OFFSET_EXTENDED_DATA, HEADER_OFFSET_NODES, HEADER_OFFSET_STRING_DATA,
    HEADER_OFFSET_STRING_OFFSETS, HEADER_OFFSET_STRUCTURED_DATA, NODE_DATA_STRING_INDEX_MASK,
    NODE_DATA_TYPE_MASK, NODE_DATA_TYPE_STRING, NODE_OFFSET_DATA, NODE_OFFSET_END,
    NODE_OFFSET_KIND, NODE_OFFSET_NEXT, NODE_OFFSET_PARENT, NODE_OFFSET_POS, NODE_SIZE,
    PROTOCOL_VERSION, SYNTAX_KIND_NODE_LIST, build_node_index_table, decode_nodes,
    decode_source_file, encode_node, encode_source_file, go_kind_string,
};
use ts_goport::ast::{
    ContentMapperSourceFileInfo, MappedDiagnosticDirective, MappedDiagnosticDirectivePolicy,
    NodeVisitor, NodeVisitorHooks, TextRange, new_node_visitor, source_file_file_name,
    source_file_parser_fields, source_file_text, with_ast_data,
};
use ts_goport::astdata::{NodeData, SyntaxKind};
use ts_goport::core::Node;
use ts_goport::flags::{NodeFlags, ScriptKind};
use ts_goport::frontend::parser::{self, SourceFileParseOptions};
use ts_goport::frontend::tspath::Path;
use ts_goport::program;
use ts_goport::scanner_util::go_string_from_bytes;

/// Go `node.AsX()` for the node type of `kind`: panics on another node.
fn as_kind(node: Node, kind: SyntaxKind) -> Node {
    assert!(
        node.is_some() && node.kind() == kind,
        "As{} on {}",
        kind.as_str(),
        if node.is_nil() {
            "nil".to_string()
        } else {
            go_kind_string(node.kind() as i16)
        }
    );
    node
}

// Go: api/encoder/decoder_test.go:17 parseSourceFile
// PORT: the encoder tests call `parser.ParseSourceFile` with the same
// options inline; they use this helper too. Go returns the `*ast.SourceFile`
// with its parser fields. Here the parse is recorded
// (`program::note_parsed_source_file`), so the encoder finds the parser
// fields of the root node.
fn parse_source_file(code: &'static str) -> Node {
    let file = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/test.ts".to_string(),
            path: Path("/test.ts".to_string()),
            ..Default::default()
        },
        code,
        ScriptKind::TS,
    ));
    program::note_parsed_source_file(&file);
    file.root
}

// ── encoder_test.go ──────────────────────────────────────────────────────

// Go: api/encoder/encoder_test.go:20 TestEncodeSourceFile
#[test]
fn test_encode_source_file() {
    let source_file = parse_source_file(
        "import { bar } from \"bar\";\nexport function foo<T, U>(a: string, b: string): any {}\nfoo();",
    );
    let mut t = Subtests::new("TestEncodeSourceFile");
    t.run("baseline", || {
        let (buf, _) = encode_source_file(source_file).expect("assert.NilError");

        let str = format_encoded_source_file(&buf);
        baseline::run(
            "encodeSourceFile.txt",
            &str,
            &baseline::Options {
                subfolder: "api".into(),
                ..Default::default()
            },
        )
    });
    t.finish();
}

// Go: api/encoder/encoder_test.go:38 TestEncodeContentMapperSourceFileMetadata (tsgo#4712)
// PORT: Go sets the info on the parsed `*ast.SourceFile`. Here it is set on
// the recorded `ParsedSourceFile`, as the content mapper transform does.
#[test]
fn test_encode_content_mapper_source_file_metadata() {
    assert_eq!(
        PROTOCOL_VERSION, 9,
        "protocol version = {PROTOCOL_VERSION}, want 9"
    );
    let file = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/component.vue".to_string(),
            path: Path("/component.vue".to_string()),
            ..Default::default()
        },
        "😀virtual",
        ScriptKind::TS,
    ));
    program::note_parsed_source_file(&file);
    file.set_content_mapper_info(ContentMapperSourceFileInfo {
        original_text: "😀original".to_string(),
        content_mapper: "mapper@1.0.0".to_string(),
        virtual_file_name: "/component.vue.ts".to_string(),
        diagnostic_directives: vec![MappedDiagnosticDirective {
            original_range: TextRange::new(4, 5),
            virtual_range: TextRange::new(4, 11),
            policy: MappedDiagnosticDirectivePolicy::EXPECT,
            unused_code: 2578,
            unused_message_text: "Unused framework directive.".to_string(),
            source: "mapper".to_string(),
        }],
        ..Default::default()
    });
    let source_file = file.root;

    let (buf, _) = encode_source_file(source_file).expect("assert.NilError");
    let nodes_offset = read_uint32(&buf, HEADER_OFFSET_NODES);
    let root_data = read_uint32(&buf, nodes_offset as usize + NODE_SIZE + NODE_OFFSET_DATA);
    let extended_offset =
        read_uint32(&buf, HEADER_OFFSET_EXTENDED_DATA) + (root_data & NODE_DATA_STRING_INDEX_MASK);
    assert!(
        extended_offset as usize + 76 <= buf.len(),
        "invalid extended offset {} (nodes={} rootData={:#x} extendedData={} len={})",
        extended_offset,
        nodes_offset,
        root_data,
        read_uint32(&buf, HEADER_OFFSET_EXTENDED_DATA),
        buf.len()
    );
    let content_mapper_index = read_uint32(&buf, extended_offset as usize + 64);
    let virtual_file_name_index = read_uint32(&buf, extended_offset as usize + 68);
    let diagnostic_directives_offset = read_uint32(&buf, extended_offset as usize + 72);
    assert_eq!(encoded_string(&buf, content_mapper_index), "mapper@1.0.0");
    assert_eq!(
        encoded_string(&buf, virtual_file_name_index),
        "/component.vue.ts"
    );
    let structured_data_offset = read_uint32(&buf, HEADER_OFFSET_STRUCTURED_DATA);
    let directive_offset = (structured_data_offset + diagnostic_directives_offset) as usize;
    assert_eq!(
        &buf[directive_offset..directive_offset + 10],
        &[
            0x91, // one directive
            0x96, // six-element tuple
            2, 1, // original range [2, 3) in UTF-16
            2, 7, // virtual range [2, 9) in UTF-16
            1, // expect policy
            0xcd, 10, 18, // unused diagnostic code 2578
        ]
    );
}

// Go: api/encoder/encoder_test.go:86 encodedString
fn encoded_string(buf: &[u8], index: u32) -> String {
    let string_offsets = read_uint32(buf, HEADER_OFFSET_STRING_OFFSETS);
    let string_data = read_uint32(buf, HEADER_OFFSET_STRING_DATA);
    let start = read_uint32(buf, (string_offsets + index * 4) as usize);
    let end = read_uint32(buf, (string_offsets + index * 4 + 4) as usize);
    go_string_from_bytes(buf[(string_data + start) as usize..(string_data + end) as usize].to_vec())
}

// Go: api/encoder/encoder_test.go:94 TestEncodeSourceFileWithUnicodeEscapes
#[test]
fn test_encode_source_file_with_unicode_escapes() {
    let source_file = parse_source_file(
        r#"let a = "😃"; let b = "\ud83d\ude03"; let c = "\udc00\ud83d\ude03"; let d = "\ud83d\ud83d\ude03""#,
    );
    let mut t = Subtests::new("TestEncodeSourceFileWithUnicodeEscapes");
    t.run("baseline", || {
        let (buf, _) = encode_source_file(source_file).expect("assert.NilError");

        let str = format_encoded_source_file(&buf);
        baseline::run(
            "encodeSourceFileWithUnicodeEscapes.txt",
            &str,
            &baseline::Options {
                subfolder: "api".into(),
                ..Default::default()
            },
        )
    });
    t.finish();
}

/// The kind, range and string bytes of each encoded node from the end of
/// file token on, one line per node.
fn encoded_nodes_from_end_of_file(encoded: &[u8]) -> Vec<String> {
    let offset_nodes = read_uint32(encoded, HEADER_OFFSET_NODES) as usize;
    let offset_string_offsets = read_uint32(encoded, HEADER_OFFSET_STRING_OFFSETS);
    let offset_strings = read_uint32(encoded, HEADER_OFFSET_STRING_DATA) as usize;
    let mut lines = Vec::new();
    for i in (offset_nodes + NODE_SIZE..encoded.len()).step_by(NODE_SIZE) {
        let kind = read_uint32(encoded, i + NODE_OFFSET_KIND);
        if lines.is_empty() && kind as i16 != SyntaxKind::EndOfFile as i16 {
            continue;
        }
        let mut line = if kind == SYNTAX_KIND_NODE_LIST {
            "NodeList".to_string()
        } else {
            go_kind_string(kind as i16)
        };
        line.push_str(&format!(
            " [{}, {})",
            read_uint32(encoded, i + NODE_OFFSET_POS),
            read_uint32(encoded, i + NODE_OFFSET_END)
        ));
        let data = read_uint32(encoded, i + NODE_OFFSET_DATA);
        if data & NODE_DATA_TYPE_MASK == NODE_DATA_TYPE_STRING {
            let at = (offset_string_offsets + (data & NODE_DATA_STRING_INDEX_MASK) * 4) as usize;
            let (start, end) = (read_uint32(encoded, at), read_uint32(encoded, at + 4));
            let bytes = &encoded[offset_strings + start as usize..offset_strings + end as usize];
            line.push_str(&format!(" \"{}\"", bytes.escape_ascii()));
        }
        lines.push(line);
    }
    lines
}

// PORT: no Go counterpart (optapifuzz1 A). An unterminated JSDoc comment at
// the end of a file whose last 2 bytes split a char. Go cuts the comment
// text 2 bytes before the end (parser/jsdoc.go:163) and keeps the bytes of
// the split char, which its scanner reads as RuneError. The port panicked on
// that cut. Each expected list is Go N's getSourceFile answer for the same
// text. A JS file parses JSDoc at once, a TS file when the encoder reads it.
#[test]
fn test_encode_source_file_jsdoc_cut_inside_a_char() {
    let cases: [(&str, ScriptKind, &[u8], &[&str]); 5] = [
        (
            "/a.js",
            ScriptKind::JS,
            "/** x 😀".as_bytes(),
            &[
                "KindEndOfFile [30, 39)",
                "KindJSDoc [30, 39)",
                "NodeList [31, 39)",
                "KindJSDocText [31, 39) \"x \\xf0\\x9f\"",
            ],
        ),
        (
            "/a.ts",
            ScriptKind::TS,
            b"/** x \xe6\x97\xa5\xff",
            &[
                "KindEndOfFile [30, 39)",
                "KindJSDoc [30, 39)",
                "NodeList [31, 39)",
                "KindJSDocText [31, 39) \"x \\xe6\\x97\"",
            ],
        ),
        (
            "/b.js",
            ScriptKind::JS,
            b"/** x \xc3\xa9\xe6",
            &[
                "KindEndOfFile [30, 39)",
                "KindJSDoc [30, 39)",
                "NodeList [31, 38)",
                "KindJSDocText [31, 38) \"x \\xc3\"",
            ],
        ),
        (
            "/c.js",
            ScriptKind::JS,
            b"/** @param {\xe6\x97\xa5\xff",
            &[
                "KindEndOfFile [30, 45)",
                "KindJSDoc [30, 45)",
                "NodeList [31, 35)",
                "NodeList [35, 45)",
                "KindJSDocParameterTag [35, 45)",
                "KindIdentifier [36, 41) \"param\"",
                "KindIdentifier [43, 43) \"\"",
                "KindJSDocTypeExpression [42, 43)",
                "KindTypeReference [43, 43)",
                "KindIdentifier [43, 43) \"\"",
                "NodeList [43, 45)",
                "KindJSDocText [43, 45) \"\\xe6\\x97\"",
            ],
        ),
        (
            "/d.js",
            ScriptKind::JS,
            "/** @param {string} a 😀".as_bytes(),
            &[
                "KindEndOfFile [30, 55)",
                "KindJSDoc [30, 55)",
                "NodeList [31, 35)",
                "NodeList [35, 55)",
                "KindJSDocParameterTag [35, 55)",
                "KindIdentifier [36, 41) \"param\"",
                "KindIdentifier [51, 52) \"a\"",
                "KindJSDocTypeExpression [42, 50)",
                "KindStringKeyword [43, 49)",
                "NodeList [53, 55)",
                "KindJSDocText [53, 55) \"\\xf0\\x9f\"",
            ],
        ),
    ];
    for (name, script_kind, comment, want) in cases {
        let mut bytes = b"function f(a, b) { return a; }\n".to_vec();
        bytes.extend_from_slice(comment);
        let text: &'static str = Box::leak(go_string_from_bytes(bytes).into_boxed_str());
        let file = Rc::new(parser::parse_source_file(
            &SourceFileParseOptions {
                file_name: name.to_string(),
                path: Path(name.to_string()),
                ..Default::default()
            },
            text,
            script_kind,
        ));
        program::note_parsed_source_file(&file);
        let (buf, _) = encode_source_file(file.root).expect("assert.NilError");
        assert_eq!(encoded_nodes_from_end_of_file(&buf), want, "{name}");
    }
}

// PORT: no Go counterpart (jsdocapi1 t18). Go reads the 3 WTF-8 bytes of a
// lone surrogate as one rune of one UTF-16 unit, and a position k bytes
// into it is its UTF-16 offset plus k (ast/positionmap.go). Source text
// holds the 3 bytes as 3 units of the port form, so the JSDoc cut 2 bytes
// before the end leaves a position after the first unit. The port counted
// the port bytes of that unit. Each expected list is Go N's getSourceFile
// answer for the same text.
#[test]
fn test_encode_source_file_position_inside_a_wtf8_surrogate() {
    let cases: [(&str, ScriptKind, &[u8], &[&str]); 4] = [
        (
            "/a.js",
            ScriptKind::JS,
            b"/** x \xed\xa0\x80",
            &[
                "KindEndOfFile [30, 38)",
                "KindJSDoc [30, 38)",
                "NodeList [31, 38)",
                "KindJSDocText [31, 38) \"x \\xed\"",
            ],
        ),
        (
            "/a.ts",
            ScriptKind::TS,
            b"/** x \xed\xa0\x80",
            &[
                "KindEndOfFile [30, 38)",
                "KindJSDoc [30, 38)",
                "NodeList [31, 38)",
                "KindJSDocText [31, 38) \"x \\xed\"",
            ],
        ),
        (
            "/b.js",
            ScriptKind::JS,
            b"/** x \xed\xa0\x80\xed\xb0\x80",
            &[
                "KindEndOfFile [30, 39)",
                "KindJSDoc [30, 39)",
                "NodeList [31, 39)",
                "KindJSDocText [31, 39) \"x \\xed\\xa0\\x80\\xed\"",
            ],
        ),
        (
            "/c.js",
            ScriptKind::JS,
            b"/** @param {\xed\xa0\x80y",
            &[
                "KindEndOfFile [30, 45)",
                "KindJSDoc [30, 45)",
                "NodeList [31, 35)",
                "NodeList [35, 45)",
                "KindJSDocParameterTag [35, 45)",
                "KindIdentifier [36, 41) \"param\"",
                "KindIdentifier [43, 43) \"\"",
                "KindJSDocTypeExpression [42, 43)",
                "KindTypeReference [43, 43)",
                "KindIdentifier [43, 43) \"\"",
                "NodeList [43, 45)",
                "KindJSDocText [43, 45) \"\\xed\\xa0\"",
            ],
        ),
    ];
    for (name, script_kind, comment, want) in cases {
        let mut bytes = b"function f(a, b) { return a; }\n".to_vec();
        bytes.extend_from_slice(comment);
        let text: &'static str = Box::leak(go_string_from_bytes(bytes).into_boxed_str());
        let file = Rc::new(parser::parse_source_file(
            &SourceFileParseOptions {
                file_name: name.to_string(),
                path: Path(name.to_string()),
                ..Default::default()
            },
            text,
            script_kind,
        ));
        program::note_parsed_source_file(&file);
        let (buf, _) = encode_source_file(file.root).expect("assert.NilError");
        assert_eq!(encoded_nodes_from_end_of_file(&buf), want, "{name}");
    }
}

// Go: api/encoder/encoder_test.go:112 TestBuildNodeIndexTableMatchesEncode
#[test]
fn test_build_node_index_table_matches_encode() {
    let source_file = parse_source_file(
        "import { bar } from \"bar\";\nexport function foo<T, U>(a: string, b: string): any {}\nfoo();",
    );

    let (_, encode_table) = encode_source_file(source_file).expect("assert.NilError");

    let build_table = build_node_index_table(source_file);

    // Both tables should produce identical Nodes slices
    assert_eq!(
        build_table.nodes.len(),
        encode_table.nodes.len(),
        "Nodes slice length mismatch"
    );

    // Every index should map to the same node
    for i in 0..encode_table.nodes.len() {
        assert_eq!(
            build_table.nodes[i], encode_table.nodes[i],
            "node mismatch at index {i}"
        );
    }

    // GetIndex on both tables should agree for every non-nil node
    for (i, &node) in encode_table.nodes.iter().enumerate() {
        if node.is_nil() {
            continue;
        }
        let enc_idx = encode_table.get_index(node);
        let build_idx = build_table.get_index(node);
        assert_eq!(
            enc_idx,
            i as u32,
            "encodeTable.GetIndex mismatch at index {i}, node kind={}",
            go_kind_string(node.kind() as i16)
        );
        assert_eq!(
            build_idx,
            enc_idx,
            "buildTable.GetIndex mismatch for node kind={}",
            go_kind_string(node.kind() as i16)
        );
    }
}

// Go: api/encoder/encoder_test.go:144 BenchmarkEncodeSourceFile
// Go: api/encoder/encoder_test.go:159 BenchmarkBuildNodeIndexTable
// PORT: not ported (benchmarks).

// Go: api/encoder/encoder_test.go:173 readUint32
fn read_uint32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buf[offset..offset + 4].try_into().expect("four bytes"))
}

// Go: api/encoder/encoder_test.go:177 formatEncodedSourceFile
fn format_encoded_source_file(encoded: &[u8]) -> String {
    let mut result = String::new();
    let offset_nodes = read_uint32(encoded, HEADER_OFFSET_NODES);
    let offset_string_offsets = read_uint32(encoded, HEADER_OFFSET_STRING_OFFSETS);
    let offset_strings = read_uint32(encoded, HEADER_OFFSET_STRING_DATA);
    // PORT: the recursive Go closure `getIndent` is a nested fn.
    fn get_indent(encoded: &[u8], offset_nodes: u32, parent_index: u32) -> String {
        if parent_index == 0 {
            return String::new();
        }
        "  ".to_string()
            + &get_indent(
                encoded,
                offset_nodes,
                read_uint32(
                    encoded,
                    offset_nodes as usize + parent_index as usize * NODE_SIZE + NODE_OFFSET_PARENT,
                ),
            )
    }
    let mut j = 1;
    let mut i = offset_nodes as usize + NODE_SIZE;
    while i < encoded.len() {
        let kind = read_uint32(encoded, i + NODE_OFFSET_KIND);
        let pos = read_uint32(encoded, i + NODE_OFFSET_POS);
        let end = read_uint32(encoded, i + NODE_OFFSET_END);
        let parent_index = read_uint32(encoded, i + NODE_OFFSET_PARENT);
        result.push_str(&get_indent(encoded, offset_nodes, parent_index));
        if kind == SYNTAX_KIND_NODE_LIST {
            result.push_str("NodeList");
        } else {
            // Go `ast.Kind(kind).String()`: Kind is an int16.
            result.push_str(&go_kind_string(kind as i16));
        }
        let data = read_uint32(encoded, i + NODE_OFFSET_DATA);
        let data_type = data & NODE_DATA_TYPE_MASK;
        if kind as i16 == SyntaxKind::Identifier as i16 || data_type == NODE_DATA_TYPE_STRING {
            let string_index = data & NODE_DATA_STRING_INDEX_MASK;
            let str_start =
                read_uint32(encoded, (offset_string_offsets + string_index * 4) as usize);
            let str_end = read_uint32(
                encoded,
                (offset_string_offsets + string_index * 4) as usize + 4,
            );
            let str = go_string_from_bytes(
                encoded[(offset_strings + str_start) as usize..(offset_strings + str_end) as usize]
                    .to_vec(),
            );
            result.push_str(&format!(" \"{str}\""));
        }
        result.push_str(&format!(
            " [{pos}, {end}), i={j}, next={}",
            encoded[i + NODE_OFFSET_NEXT]
        ));
        result.push('\n');
        j += 1;
        i += NODE_SIZE;
    }
    result
}

// ── decoder_test.go ──────────────────────────────────────────────────────

/// Go `decoded.Statements.Nodes[i].AsVariableStatement().DeclarationList.
/// AsVariableDeclarationList().Declarations.Nodes[0].AsVariableDeclaration()`.
fn first_variable_declaration(decoded: Node, i: usize) -> Node {
    let var_stmt = as_kind(decoded.statements().get(i), SyntaxKind::VariableStatement);
    let decl_list = as_kind(
        var_stmt.declaration_list(),
        SyntaxKind::VariableDeclarationList,
    );
    as_kind(
        decl_list.declarations().nodes().get(0),
        SyntaxKind::VariableDeclaration,
    )
}

// Go: api/encoder/decoder_test.go:24 TestDecodeSourceFile_Basic
#[test]
fn test_decode_source_file_basic() {
    let sf = parse_source_file("let x = 1;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");
    assert_eq!(decoded.kind(), SyntaxKind::SourceFile);
    assert_eq!(source_file_file_name(decoded), "/test.ts");
    assert_eq!(source_file_text(decoded), "let x = 1;");
    assert!(decoded.statement_list().is_some());
    assert!(decoded.end_of_file_token().is_some());
}

// Go: api/encoder/decoder_test.go:39 TestDecodeSourceFile_Metadata (ts#64320)
// PORT: Go reads the fields of the parsed and the decoded `*ast.SourceFile`.
// The parsed fields are on the `ParsedSourceFile`; the decoded root is a
// factory SourceFile, whose fields `source_file_parser_fields` reads.
#[test]
fn test_decode_source_file_metadata() {
    struct Test {
        name: &'static str,
        file_name: &'static str,
        script_kind: ScriptKind,
        code: &'static str,
    }
    let tests = [
        Test {
            name: "JSON",
            file_name: "/test.json",
            script_kind: ScriptKind::JSON,
            code: r#"{"x": 1}"#,
        },
        Test {
            name: "JSX",
            file_name: "/test.jsx",
            script_kind: ScriptKind::JSX,
            code: "const x = <div />;",
        },
        Test {
            name: "declaration",
            file_name: "/test.d.ts",
            script_kind: ScriptKind::TS,
            code: "declare const x: number;",
        },
    ];

    let mut t = Subtests::new("TestDecodeSourceFile_Metadata");
    for tt in tests {
        t.run(tt.name, || {
            let file = Rc::new(parser::parse_source_file(
                &SourceFileParseOptions {
                    file_name: tt.file_name.to_string(),
                    path: Path(tt.file_name.to_string()),
                    ..Default::default()
                },
                tt.code,
                tt.script_kind,
            ));
            program::note_parsed_source_file(&file);
            let (buf, _) = encode_source_file(file.root).expect("assert.NilError");

            let decoded = decode_source_file(&buf).expect("assert.NilError");
            let decoded = source_file_parser_fields(decoded);
            assert_eq!(decoded.script_kind, file.script_kind);
            assert_eq!(decoded.language_variant, file.language_variant);
            assert_eq!(decoded.is_declaration_file, file.is_declaration_file);
            Ok(())
        });
    }
    t.finish();
}

// Go: api/encoder/decoder_test.go:73 TestDecodeSourceFileRejectsInvalidFileName (ts#64159; N' line)
#[test]
fn test_decode_source_file_rejects_invalid_file_name() {
    let file = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/Test.ts".to_string(),
            path: Path("/test.ts".to_string()),
            ..Default::default()
        },
        "",
        ScriptKind::TS,
    ));
    program::note_parsed_source_file(&file);
    let (mut buf, _) = encode_source_file(file.root).expect("assert.NilError");

    let invalid_file_name = b"Test/.ts";
    let index = buf
        .windows(b"/Test.ts".len())
        .position(|window| window == b"/Test.ts")
        .expect("index >= 0");
    buf[index..index + invalid_file_name.len()].copy_from_slice(invalid_file_name);
    let err = decode_source_file(&buf).expect_err("expected an error");
    assert!(
        err.error()
            .contains(r#"invalid source file name "Test/.ts""#),
        "{}",
        err.error()
    );
}

// Go: api/encoder/decoder_test.go:72 TestDecodeSourceFile_Statements
#[test]
fn test_decode_source_file_statements() {
    let sf = parse_source_file("let a = 1;\nlet b = 2;\nlet c = 3;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");
    assert_eq!(decoded.statements().len(), 3);
    for (i, stmt) in decoded.statements().iter().enumerate() {
        assert_eq!(stmt.kind(), SyntaxKind::VariableStatement, "statement {i}");
    }
}

// Go: api/encoder/decoder_test.go:86 TestDecodeSourceFile_VariableDeclaration
#[test]
fn test_decode_source_file_variable_declaration() {
    let sf = parse_source_file("let x = 1;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let var_stmt = as_kind(decoded.statements().get(0), SyntaxKind::VariableStatement);
    assert!(var_stmt.declaration_list().is_some());
    let decl_list = as_kind(
        var_stmt.declaration_list(),
        SyntaxKind::VariableDeclarationList,
    );
    assert!(decl_list.declarations().is_some());
    assert_eq!(decl_list.declarations().nodes().len(), 1);

    let decl = as_kind(
        decl_list.declarations().nodes().get(0),
        SyntaxKind::VariableDeclaration,
    );
    assert_eq!(decl.name().kind(), SyntaxKind::Identifier);
    assert_eq!(as_kind(decl.name(), SyntaxKind::Identifier).text(), "x");
    assert!(decl.initializer().is_some());
    assert_eq!(decl.initializer().kind(), SyntaxKind::NumericLiteral);
    assert_eq!(
        as_kind(decl.initializer(), SyntaxKind::NumericLiteral).text(),
        "1"
    );
}

// Go: api/encoder/decoder_test.go:109 TestDecodeSourceFile_VariableDeclarationListFlags
#[test]
fn test_decode_source_file_variable_declaration_list_flags() {
    struct Test {
        name: &'static str,
        code: &'static str,
        expected: NodeFlags,
    }
    let tests = [
        Test {
            name: "const",
            code: "const x = 1;",
            expected: NodeFlags::CONST,
        },
        Test {
            name: "let",
            code: "let x = 1;",
            expected: NodeFlags::LET,
        },
        Test {
            name: "var",
            code: "var x = 1;",
            expected: NodeFlags::NONE,
        },
    ];

    let mut t = Subtests::new("TestDecodeSourceFile_VariableDeclarationListFlags");
    for tt in &tests {
        t.run(tt.name, || {
            let sf = parse_source_file(tt.code);
            let (buf, _) = encode_source_file(sf).expect("assert.NilError");

            let decoded = decode_source_file(&buf).expect("assert.NilError");

            let decl_list = as_kind(
                as_kind(decoded.statements().get(0), SyntaxKind::VariableStatement)
                    .declaration_list(),
                SyntaxKind::VariableDeclarationList,
            );
            let got = decl_list.flags() & (NodeFlags::LET | NodeFlags::CONST);
            assert_eq!(
                got, tt.expected,
                "flags for {:?}: got {}, want {}",
                tt.code, got.0, tt.expected.0
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: api/encoder/decoder_test.go:139 TestDecodeSourceFile_FunctionDeclaration
#[test]
fn test_decode_source_file_function_declaration() {
    let sf = parse_source_file("function add(a: number, b: number): number { return a + b; }");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let func_decl = as_kind(decoded.statements().get(0), SyntaxKind::FunctionDeclaration);
    assert!(func_decl.name().is_some());
    assert_eq!(
        as_kind(func_decl.name(), SyntaxKind::Identifier).text(),
        "add"
    );
    assert!(func_decl.parameter_list().is_some());
    assert_eq!(func_decl.parameters().len(), 2);
    assert!(func_decl.type_().is_some());
    assert!(func_decl.body().is_some());

    let param0 = as_kind(func_decl.parameters().get(0), SyntaxKind::Parameter);
    assert_eq!(as_kind(param0.name(), SyntaxKind::Identifier).text(), "a");
    assert!(param0.type_().is_some());
}

// Go: api/encoder/decoder_test.go:161 TestDecodeSourceFile_ImportDeclaration
#[test]
fn test_decode_source_file_import_declaration() {
    let sf = parse_source_file(r#"import { bar } from "bar";"#);
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let imp = as_kind(decoded.statements().get(0), SyntaxKind::ImportDeclaration);
    assert!(imp.import_clause().is_some());
    assert!(imp.module_specifier().is_some());
    assert_eq!(
        as_kind(imp.module_specifier(), SyntaxKind::StringLiteral).text(),
        "bar"
    );

    let clause = as_kind(imp.import_clause(), SyntaxKind::ImportClause);
    assert!(clause.named_bindings().is_some());
    let named_imports = as_kind(clause.named_bindings(), SyntaxKind::NamedImports);
    assert!(named_imports.element_list().is_some());
    assert_eq!(named_imports.elements().len(), 1);
    let spec = as_kind(named_imports.elements().get(0), SyntaxKind::ImportSpecifier);
    assert_eq!(as_kind(spec.name(), SyntaxKind::Identifier).text(), "bar");
}

// Go: api/encoder/decoder_test.go:202 TestDecodeSourceFile_SourcePhaseImport (ts#63915; N' line)
#[test]
fn test_decode_source_file_source_phase_import() {
    let sf = parse_source_file(r#"import source a from "./a.wasm";"#);
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let imp = as_kind(decoded.statements().get(0), SyntaxKind::ImportDeclaration);
    let clause = as_kind(imp.import_clause(), SyntaxKind::ImportClause);
    assert_eq!(clause.phase_modifier(), SyntaxKind::SourceKeyword);
    assert_eq!(clause.name().text(), "a");
}

// Go: api/encoder/decoder_test.go:184 TestDecodeSourceFile_IfStatement
#[test]
fn test_decode_source_file_if_statement() {
    let sf = parse_source_file("if (true) { } else { }");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let if_stmt = as_kind(decoded.statements().get(0), SyntaxKind::IfStatement);
    assert!(if_stmt.expression().is_some());
    assert!(if_stmt.then_statement().is_some());
    assert!(if_stmt.else_statement().is_some());
    assert_eq!(if_stmt.then_statement().kind(), SyntaxKind::Block);
    assert_eq!(if_stmt.else_statement().kind(), SyntaxKind::Block);
}

// Go: api/encoder/decoder_test.go:201 TestDecodeSourceFile_TemplateExpression
#[test]
fn test_decode_source_file_template_expression() {
    let sf = parse_source_file("let x = `hello ${name} world`;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let var_decl = first_variable_declaration(decoded, 0);
    let tmpl_expr = as_kind(var_decl.initializer(), SyntaxKind::TemplateExpression);
    assert!(tmpl_expr.head().is_some());
    assert_eq!(
        as_kind(tmpl_expr.head(), SyntaxKind::TemplateHead).text(),
        "hello "
    );
    assert!(tmpl_expr.template_spans().is_some());
    assert_eq!(tmpl_expr.template_spans().nodes().len(), 1);

    let span = as_kind(
        tmpl_expr.template_spans().nodes().get(0),
        SyntaxKind::TemplateSpan,
    );
    assert!(span.expression().is_some());
    assert_eq!(span.expression().kind(), SyntaxKind::Identifier);
    assert!(span.literal().is_some());
    assert_eq!(
        as_kind(span.literal(), SyntaxKind::TemplateTail).text(),
        " world"
    );
}

// Go: api/encoder/decoder_test.go:224 TestDecodeSourceFile_ExportModifier
#[test]
fn test_decode_source_file_export_modifier() {
    let sf = parse_source_file("export function foo() {}");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let func_decl = as_kind(decoded.statements().get(0), SyntaxKind::FunctionDeclaration);
    assert!(func_decl.modifiers().is_some());
    assert_eq!(func_decl.modifiers().nodes().len(), 1);
    assert_eq!(
        func_decl.modifiers().nodes().get(0).kind(),
        SyntaxKind::ExportKeyword
    );
}

// Go: api/encoder/decoder_test.go:239 TestDecodeSourceFile_Positions
#[test]
fn test_decode_source_file_positions() {
    let code = "let x = 1;";
    let sf = parse_source_file(code);
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    assert_eq!(decoded.pos(), 0);
    assert_eq!(decoded.end(), code.len() as i32);
}

// Go: api/encoder/decoder_test.go:253 TestDecodeSourceFile_ClassDeclaration
#[test]
fn test_decode_source_file_class_declaration() {
    let sf = parse_source_file("class Foo { bar(): void {} }");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let class_decl = as_kind(decoded.statements().get(0), SyntaxKind::ClassDeclaration);
    assert!(class_decl.name().is_some());
    assert_eq!(
        as_kind(class_decl.name(), SyntaxKind::Identifier).text(),
        "Foo"
    );
    assert!(class_decl.member_list().is_some());
    assert_eq!(class_decl.members().len(), 1);
    assert_eq!(
        class_decl.members().get(0).kind(),
        SyntaxKind::MethodDeclaration
    );
}

// Go: api/encoder/decoder_test.go:270 TestDecodeNodes_SubtreeRoundTrip
#[test]
fn test_decode_nodes_subtree_round_trip() {
    let sf = parse_source_file("function greet(name: string) { return `Hello, ${name}!`; }");

    let func_node: Cell<Node> = Cell::new(Node::NIL);
    // PORT: Go builds `&ast.NodeVisitor{}` and sets `Visit` after; the
    // Rust visitor takes the visit callback and the (default) hooks at once.
    let mut visitor = new_node_visitor(
        |node: Node, _v: &mut NodeVisitor<'_, ()>| {
            if node.kind() == SyntaxKind::FunctionDeclaration && func_node.get().is_nil() {
                func_node.set(node);
            }
            node
        },
        None,
        NodeVisitorHooks::default(),
        (),
    );
    let _ = visitor.visit_each_child(sf);
    let func_node = func_node.get();
    assert!(func_node.is_some());

    let (buf, _) = encode_node(func_node, sf).expect("assert.NilError");

    let decoded = decode_nodes(&buf).expect("assert.NilError");

    assert_eq!(decoded.kind(), SyntaxKind::FunctionDeclaration);
    let func_decl = as_kind(decoded, SyntaxKind::FunctionDeclaration);
    assert!(func_decl.name().is_some());
    assert_eq!(
        as_kind(func_decl.name(), SyntaxKind::Identifier).text(),
        "greet"
    );
    assert!(func_decl.parameter_list().is_some());
    assert_eq!(func_decl.parameters().len(), 1);
    assert!(func_decl.body().is_some());
}

// Go: api/encoder/decoder_test.go:300 TestDecodeSourceFile_BinaryExpression
#[test]
fn test_decode_source_file_binary_expression() {
    let sf = parse_source_file("let x = 1 + 2;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let decl = first_variable_declaration(decoded, 0);
    let bin_expr = as_kind(decl.initializer(), SyntaxKind::BinaryExpression);
    assert!(bin_expr.left().is_some());
    assert!(bin_expr.right().is_some());
    assert!(bin_expr.operator_token().is_some());
    assert_eq!(bin_expr.left().kind(), SyntaxKind::NumericLiteral);
    assert_eq!(bin_expr.right().kind(), SyntaxKind::NumericLiteral);
}

// Go: api/encoder/decoder_test.go:318 TestDecodeSourceFile_KeywordExpressions
#[test]
fn test_decode_source_file_keyword_expressions() {
    // "this" must decode as KeywordExpression, not Token, or the printer panics
    let sf = parse_source_file("const x = this;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    // Navigate: const x = this -> VariableStatement -> declaration -> initializer
    let decl = first_variable_declaration(decoded, 0);
    let this_expr = decl.initializer();
    assert_eq!(this_expr.kind(), SyntaxKind::ThisKeyword);
    // This would panic if decoded as Token instead of KeywordExpression
    // PORT: Go `thisExpr.AsKeywordExpression() != nil` checks the node data type.
    assert!(with_ast_data(this_expr, |d| matches!(
        d,
        NodeData::KeywordExpression(_)
    )));
}

// Go: api/encoder/decoder_test.go:336 TestDecodeSourceFile_EmptyModuleBlock
#[test]
fn test_decode_source_file_empty_module_block() {
    let sf = parse_source_file("namespace N { }");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    // Navigate: namespace N { } -> ModuleDeclaration -> ModuleBlock
    let module = as_kind(decoded.statements().get(0), SyntaxKind::ModuleDeclaration);
    assert!(module.body().is_some());
    let block = as_kind(module.body(), SyntaxKind::ModuleBlock);
    // Statements must be non-nil even when empty, otherwise the printer panics
    assert!(block.statement_list().is_some());
    assert_eq!(block.statements().len(), 0);
}

// Go: api/encoder/decoder_test.go:354 TestDecodeSourceFile_EmptyBlockAndParams
#[test]
fn test_decode_source_file_empty_block_and_params() {
    // Empty blocks and parameter lists must decode with non-nil NodeLists (not nil),
    // matching parser behavior. Previously the decoder left them nil, crashing the printer.
    let sf = parse_source_file("function foo() {}");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let func_decl = as_kind(decoded.statements().get(0), SyntaxKind::FunctionDeclaration);
    assert!(
        func_decl.parameter_list().is_some(),
        "FunctionDeclaration.Parameters must be non-nil for foo()"
    );
    assert_eq!(func_decl.parameters().len(), 0);
    assert!(func_decl.body().is_some());
    let block = as_kind(func_decl.body(), SyntaxKind::Block);
    assert!(
        block.statement_list().is_some(),
        "Block.Statements must be non-nil for empty blocks"
    );
    assert_eq!(block.statements().len(), 0);
}

// Go: api/encoder/decoder_test.go:374 TestDecodeSourceFile_ArrowFunctionEmptyParams
#[test]
fn test_decode_source_file_arrow_function_empty_params() {
    // `() => {}` must decode with non-nil Parameters (empty NodeList),
    // matching parser behavior. Previously the decoder left it nil, crashing the printer.
    let sf = parse_source_file("const f = () => {};");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let decl = first_variable_declaration(decoded, 0);
    let arrow = as_kind(decl.initializer(), SyntaxKind::ArrowFunction);
    assert!(
        arrow.parameter_list().is_some(),
        "ArrowFunction.Parameters must be non-nil for () => {{}}"
    );
    assert_eq!(arrow.parameters().len(), 0);
    assert!(arrow.body().is_some());
    let block = as_kind(arrow.body(), SyntaxKind::Block);
    assert!(
        block.statement_list().is_some(),
        "Block.Statements must be non-nil for empty body"
    );
    assert_eq!(block.statements().len(), 0);
}

// Go: api/encoder/decoder_test.go:395 TestDecodeSourceFile_FunctionExpressionEmptyParams
#[test]
fn test_decode_source_file_function_expression_empty_params() {
    // `function() {}` must decode with non-nil Parameters (empty NodeList).
    let sf = parse_source_file("const f = function() {};");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let decl = first_variable_declaration(decoded, 0);
    let func_expr = as_kind(decl.initializer(), SyntaxKind::FunctionExpression);
    assert!(
        func_expr.parameter_list().is_some(),
        "FunctionExpression.Parameters must be non-nil for function() {{}}"
    );
    assert_eq!(func_expr.parameters().len(), 0);
}

// Go: api/encoder/decoder_test.go:411 TestDecodeSourceFile_PostfixUnaryOperator
#[test]
fn test_decode_source_file_postfix_unary_operator() {
    let sf = parse_source_file("let i = 0; i++;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let expr_stmt = as_kind(decoded.statements().get(1), SyntaxKind::ExpressionStatement);
    let postfix = as_kind(expr_stmt.expression(), SyntaxKind::PostfixUnaryExpression);
    assert_eq!(postfix.operator(), SyntaxKind::PlusPlusToken);
    assert_eq!(postfix.operand().kind(), SyntaxKind::Identifier);
}

// Go: api/encoder/decoder_test.go:426 TestDecodeSourceFile_PrefixUnaryOperator
#[test]
fn test_decode_source_file_prefix_unary_operator() {
    let sf = parse_source_file("let x = true; !x;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let expr_stmt = as_kind(decoded.statements().get(1), SyntaxKind::ExpressionStatement);
    let prefix = as_kind(expr_stmt.expression(), SyntaxKind::PrefixUnaryExpression);
    assert_eq!(prefix.operator(), SyntaxKind::ExclamationToken);
    assert_eq!(prefix.operand().kind(), SyntaxKind::Identifier);
}

// Go: api/encoder/decoder_test.go:441 TestDecodeSourceFile_PostfixDecrement
#[test]
fn test_decode_source_file_postfix_decrement() {
    let sf = parse_source_file("let n = 5; n--;");
    let (buf, _) = encode_source_file(sf).expect("assert.NilError");

    let decoded = decode_source_file(&buf).expect("assert.NilError");

    let expr_stmt = as_kind(decoded.statements().get(1), SyntaxKind::ExpressionStatement);
    let postfix = as_kind(expr_stmt.expression(), SyntaxKind::PostfixUnaryExpression);
    assert_eq!(postfix.operator(), SyntaxKind::MinusMinusToken);
}

// Go: api/encoder/decoder_test.go:455 BenchmarkDecodeSourceFile
// PORT: not ported (benchmark).
