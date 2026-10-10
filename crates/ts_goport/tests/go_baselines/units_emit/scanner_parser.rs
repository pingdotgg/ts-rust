//! Ports of internal/scanner/scanner_test.go and
//! internal/parser/parser_test.go (TestHeritageClauseElementKinds,
//! TestParseStaticSourcePhaseImport, TestParseSourceAsImportEqualsBinding,
//! TestParseInvalidStaticSourcePhaseImports,
//! TestParseDynamicSourcePhaseImport, TestParseEscapedDynamicImportPhase,
//! TestJSDocImportTypeParentChain,
//! TestJSDocTypeSourceSurvivesReparse,
//! TestJSDocTypeSourcePropagatesToConstructedReparse and
//! TestSourceFilePositionMapWithNonASCIIStringLiteral; the Go benchmark and
//! fuzz target are not ported). TestNormalizeJSDocTypeSourceText,
//! TestIsJSDocTypeExpressionOrChild and
//! TestGetTextOfNodeFromJSDocTypePreservesAsteriskType call private scanner
//! functions, so they are in `src/scanner_util/scanner_test.rs`.

use super::Subtests;
use super::childprog::in_child;
use super::parsetestutil::{check_diagnostics, parse_type_script, parse_type_script_file};
use super::{leak, must};
use ts_goport::ast::{
    get_reparsed_node_for_node, get_source_file_of_node, source_file_diagnostics,
    source_file_get_position_map,
};
use ts_goport::frontend::parser::{ParsedSourceFile, SourceFileParseOptions, parse_source_file};
use ts_goport::frontend::scanner::new_scanner;
use ts_goport::frontend::tspath::Path;
use ts_goport::prelude::*;
use ts_goport::program::{note_parsed_source_file, publish_parsed_files};

// Go: scanner/scanner_test.go:13 TestScanStringPreservesLoneSurrogates
// PORT: Go strings hold lone surrogates as WTF-8 bytes. The port keeps them
// in its Go string form (`scanner_util::GO_STRING_MARKER`), which
// `encode_js_string_rune` also writes.
#[test]
fn test_scan_string_preserves_lone_surrogates() {
    let mut s = new_scanner();
    s.set_text(r#""🦀퟿\ud800\ud801🦀""#);
    assert_eq!(s.scan(), SyntaxKind::StringLiteral);
    let expected = "🦀".to_string()
        + &encode_js_string_rune(0xD7FF)
        + &encode_js_string_rune(0xD800)
        + &encode_js_string_rune(0xD801)
        + "🦀";
    assert_eq!(s.token_value(), expected);
}

// Go: scanner/scanner_test.go:95 TestScanSourceKeyword
#[test]
fn test_scan_source_keyword() {
    let mut s = new_scanner();
    s.set_text("source sourceValue");

    assert_eq!(s.scan(), SyntaxKind::SourceKeyword);
    assert_eq!(token_to_string(SyntaxKind::SourceKeyword), "source");
    assert_eq!(string_to_token("source"), SyntaxKind::SourceKeyword);
    assert_eq!(s.scan(), SyntaxKind::Identifier);
    assert_eq!(s.token_value(), "sourceValue");
}

// Go: parser/parser_test.go:158 TestHeritageClauseElementKinds
#[test]
fn test_heritage_clause_element_kinds() {
    let source_text = r#"
class C extends Base<number> implements Contract<string> {}
interface I extends Parent<boolean> {}
interface Invalid implements Recovery {}
interface MissingExtends extends A. {}
class MissingImplements implements B. {}
"#;
    let opts = SourceFileParseOptions {
        file_name: "/index.ts".to_string(),
        path: Path("/index.ts".to_string()),
        ..Default::default()
    };

    let file = parse_source_file(&opts, leak(source_text), ScriptKind::TS);
    let statements = file.statements().nodes();
    // Go `decl.HeritageClauses.Nodes[clause].AsHeritageClause().Types.Nodes[0].Kind`.
    let first_element_kind = |decl: Node, clause: usize| {
        decl.heritage_clauses()
            .nodes()
            .get(clause)
            .types()
            .nodes()
            .get(0)
            .kind()
    };

    let class_decl = statements.get(0);
    assert!(is_class_declaration(class_decl));
    assert_eq!(
        first_element_kind(class_decl, 0),
        SyntaxKind::ExpressionWithTypeArguments
    );
    assert_eq!(first_element_kind(class_decl, 1), SyntaxKind::TypeReference);

    let interface_decl = statements.get(1);
    assert!(is_interface_declaration(interface_decl));
    assert_eq!(
        first_element_kind(interface_decl, 0),
        SyntaxKind::TypeReference
    );

    let invalid_interface_decl = statements.get(2);
    assert!(is_interface_declaration(invalid_interface_decl));
    assert_eq!(
        first_element_kind(invalid_interface_decl, 0),
        SyntaxKind::ExpressionWithTypeArguments
    );

    let missing_extends_decl = statements.get(3);
    assert!(is_interface_declaration(missing_extends_decl));
    assert_eq!(
        first_element_kind(missing_extends_decl, 0),
        SyntaxKind::ExpressionWithTypeArguments
    );

    let missing_implements_decl = statements.get(4);
    assert!(is_class_declaration(missing_implements_decl));
    assert_eq!(
        first_element_kind(missing_implements_decl, 0),
        SyntaxKind::ExpressionWithTypeArguments
    );
}

// Go: parser/parser_test.go:191 TestParseStaticSourcePhaseImport (ts#63915)
#[test]
fn test_parse_static_source_phase_import() {
    // (name, source, phaseModifier, bindingName, hasAttributes)
    let tests: &[(&str, &str, SyntaxKind, &str, bool)] = &[
        (
            "source phase",
            r#"import source a from "./a.wasm";"#,
            SyntaxKind::SourceKeyword,
            "a",
            false,
        ),
        (
            "source phase with import attributes",
            r#"import source a from "./a.wasm" with { type: "webassembly" };"#,
            SyntaxKind::SourceKeyword,
            "a",
            true,
        ),
        (
            "from as source phase binding",
            r#"import source from from "./module.js";"#,
            SyntaxKind::SourceKeyword,
            "from",
            false,
        ),
        (
            "source as ordinary default binding",
            r#"import source from "./module.js";"#,
            SyntaxKind::Unknown,
            "source",
            false,
        ),
        (
            "source as ordinary default binding with named imports",
            r#"import source, { value } from "./module.js";"#,
            SyntaxKind::Unknown,
            "source",
            false,
        ),
        (
            "escaped source as ordinary default binding",
            r#"import s\u006furce from "./module.js";"#,
            SyntaxKind::Unknown,
            "source",
            false,
        ),
        (
            "escaped defer as ordinary default binding",
            r#"import d\u0065fer from "./module.js";"#,
            SyntaxKind::Unknown,
            "defer",
            false,
        ),
    ];

    let mut t = Subtests::new("TestParseStaticSourcePhaseImport");
    for &(name, source, phase_modifier, binding_name, has_attributes) in tests {
        t.run(name, || {
            let file = parse_type_script(source, false);
            check_diagnostics(file)?;
            let statements = file.statements();
            assert_eq!(statements.len(), 1);

            let statement = statements.get(0);
            assert!(is_import_declaration(statement));

            let clause = statement.import_clause();
            assert!(clause.is_some());

            assert_eq!(clause.phase_modifier(), phase_modifier);
            assert!(clause.name().is_some());
            assert_eq!(clause.name().text(), binding_name);
            assert_eq!(statement.attributes().is_some(), has_attributes);
            Ok(())
        });
    }
    t.finish();
}

// Go: parser/parser_test.go:267 TestParseSourceAsImportEqualsBinding (ts#63915)
#[test]
fn test_parse_source_as_import_equals_binding() {
    let file = parse_type_script(r#"import source = require("./module.js");"#, false);
    must(check_diagnostics(file));
    let statements = file.statements();
    assert_eq!(statements.len(), 1);

    let statement = statements.get(0);
    assert!(is_import_equals_declaration(statement));

    assert_eq!(statement.name().text(), "source");
}

// Go: parser/parser_test.go:279 TestParseInvalidStaticSourcePhaseImports (ts#63915)
#[test]
fn test_parse_invalid_static_source_phase_imports() {
    // (source, hasName, hasNamedBindings)
    let tests: &[(&str, bool, bool)] = &[
        (r#"import source "./a.js";"#, false, false),
        (r#"import source * as a from "./a.js";"#, false, true),
        (r#"import source { a } from "./a.js";"#, false, true),
        (r#"import source a, { b } from "./a.js";"#, true, true),
    ];

    for &(source, has_name, has_named_bindings) in tests {
        let file = parse_type_script(source, false);
        must(check_diagnostics(file));
        let statements = file.statements();
        assert_eq!(statements.len(), 1);

        let statement = statements.get(0);
        assert!(is_import_declaration(statement));

        let clause = statement.import_clause();
        assert!(clause.is_some());

        assert_eq!(clause.phase_modifier(), SyntaxKind::SourceKeyword);
        assert_eq!(clause.name().is_some(), has_name);
        assert_eq!(clause.named_bindings().is_some(), has_named_bindings);
    }
}

// Go: parser/parser_test.go:310 TestParseDynamicSourcePhaseImport (ts#63915)
#[test]
fn test_parse_dynamic_source_phase_import() {
    let parsed = parse_type_script_file(
        r#"import.source("./a.wasm", { with: { type: "webassembly" } });"#,
        false,
    );
    let file = parsed.root;
    must(check_diagnostics(file));
    let statements = file.statements();
    assert_eq!(statements.len(), 1);

    let statement = statements.get(0);
    assert!(is_expression_statement(statement));

    let call = statement.expression();
    assert!(is_call_expression(call));
    assert!(is_import_call(call));
    assert!(
        call.subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_DYNAMIC_IMPORT)
    );

    let meta_property = call.expression();
    assert!(is_meta_property(meta_property));
    assert_eq!(meta_property.keyword_token(), SyntaxKind::ImportKeyword);
    assert_eq!(meta_property.text(), "source");
    assert_eq!(call.arguments().len(), 2);
    assert!(
        file.flags()
            .intersects(NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT)
    );
    assert!(
        !file
            .flags()
            .intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META)
    );
    assert!(parsed.external_module_indicator.is_nil());
}

// Go: parser/parser_test.go:334 TestParseEscapedDynamicImportPhase (ts#63915)
#[test]
fn test_parse_escaped_dynamic_import_phase() {
    // (source, phaseName)
    let tests: &[(&str, &str)] = &[
        (r#"import.d\u0065fer("./a.js");"#, "defer"),
        (r#"import.s\u006furce("./a.wasm");"#, "source"),
    ];

    let mut t = Subtests::new("TestParseEscapedDynamicImportPhase");
    for &(source, phase_name) in tests {
        t.run(phase_name, || {
            let file = parse_type_script(source, false);
            let diagnostics = source_file_diagnostics(file);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].code(),
                diag::Keywords_cannot_contain_escape_characters.code() as i32
            );
            let statements = file.statements();
            assert_eq!(statements.len(), 1);

            let statement = statements.get(0);
            assert!(is_expression_statement(statement));

            let call = statement.expression();
            assert!(is_call_expression(call));
            assert!(is_import_call(call));

            let meta_property = call.expression();
            assert!(is_meta_property(meta_property));
            assert_eq!(meta_property.text(), phase_name);
            assert!(
                file.flags()
                    .intersects(NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT)
            );
            assert!(
                !file
                    .flags()
                    .intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META)
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: parser/parser_test.go:189 TestJSDocImportTypeParentChain
// PORT: `GetSourceFileOfNode` reads the Go file data of the file, which
// exists once its node store is published. Publishing is process-wide, so
// the test runs in a child process of its own.
#[test]
fn test_js_doc_import_type_parent_chain() {
    in_child(
        module_path!(),
        "test_js_doc_import_type_parent_chain",
        js_doc_import_type_parent_chain,
    );
}

fn js_doc_import_type_parent_chain() {
    let source_text = r#"test("", async function () {
  ;(/** @type {typeof import("a")} */ ({}))
})

test("", async function () {
  ;(/** @type {typeof import("a")} */ a)
})

test("", async function () {
  (/** @type {typeof import("a")} */ ({}))
  ;(/** @type {typeof import("a")} */ ({}))
})

test("", async function () {
  (/** @type {typeof import("a")} */ a)
  ;(/** @type {typeof import("a")} */ a)
})

test("", async function () {
  (/** @type {typeof import("a")} */ ({}))
  ;(/** @type {typeof import("a")} */ ({}))
})
"#;
    let opts = SourceFileParseOptions {
        file_name: "/index.js".to_string(),
        path: Path("/index.js".to_string()),
        ..Default::default()
    };

    let file = Rc::new(parse_source_file(&opts, leak(source_text), ScriptKind::JS));
    note_parsed_source_file(&file);
    publish_parsed_files("/");

    let mut errors = Vec::new();
    for i in 1..file.reparsed_clones.len() {
        let (a, b) = (file.reparsed_clones[i - 1], file.reparsed_clones[i]);
        if a.pos() == b.pos() && a.end() == b.end() && a.kind() == b.kind() {
            errors.push(format!(
                "duplicate ReparsedClones at [{}] and [{i}]: {:?} pos={} end={}",
                i - 1,
                a.kind(),
                a.pos(),
                a.end()
            ));
        }
    }
    for &imp in &file.imports {
        let reparsed = get_reparsed_node_for_node(imp);
        if get_source_file_of_node(reparsed).is_nil() {
            errors.push(format!(
                "reparsed import at pos={} has broken parent chain",
                imp.pos()
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

// Go: parser/parser_test.go:236 TestJSDocTypeSourceSurvivesReparse
// PORT: `GetTextOfNode` reads the source file of the node, which exists once
// its node store is published, so the test runs in a child process (see
// `test_js_doc_import_type_parent_chain`).
#[test]
fn test_js_doc_type_source_survives_reparse() {
    in_child(
        module_path!(),
        "test_js_doc_type_source_survives_reparse",
        js_doc_type_source_survives_reparse,
    );
}

fn js_doc_type_source_survives_reparse() {
    let source_text = r#"/**
 * @typedef {(
 *   "a" |
 *   "b"
 * )[]} T
 */
const value = 0;"#;
    let opts = SourceFileParseOptions {
        file_name: "/index.js".to_string(),
        path: Path("/index.js".to_string()),
        ..Default::default()
    };

    let file = Rc::new(parse_source_file(&opts, leak(source_text), ScriptKind::JS));
    note_parsed_source_file(&file);
    publish_parsed_files("/");
    let mut type_alias = Node::NIL;
    for statement in file.statements().nodes().iter() {
        if is_js_type_alias_declaration(statement) {
            type_alias = statement;
            break;
        }
    }
    assert!(type_alias.is_some());

    let js_docs = type_alias.js_doc(file.root);
    assert_eq!(js_docs.len(), 1);
    let tags = js_docs.get(0).tags();
    assert!(tags.is_some());
    assert_eq!(tags.nodes().len(), 1);

    let type_expression = tags.nodes().get(0).type_expression();
    assert!(type_expression.is_some());

    let expected =
        ["(", r#""a" |"#, r#""b""#, ")[]"].join(NewLineKind::LF.get_new_line_character());
    let tests = [
        ("original", type_expression.type_()),
        ("reparsed", type_alias.type_()),
    ];
    let mut t = Subtests::new("TestJSDocTypeSourceSurvivesReparse");
    for (name, node) in tests {
        t.run(name, || {
            let text = get_text_of_node(node);
            if text != expected {
                return Err(format!("assertion failed: {text:?} != {expected:?}"));
            }
            Ok(())
        });
    }
    t.finish();
}

// PORT: no Go counterpart. Go cuts a JSDoc comment that ends the file 2
// bytes before its end (jsdoc.go:163), here inside the 3 bytes of `日`. The
// string literal type keeps the first byte of the char, so its node ends
// inside the char, and Go `GetTextOfNodeFromSourceText` (utilities.go:72)
// slices the bytes there. Declaration emit of the reparsed type alias reads
// it: the d.ts holds `"` and the byte E6, as Go writes it. R159 panicked on
// the `&str` slice (CLI exit 70).
#[test]
fn test_text_of_js_doc_node_that_ends_inside_a_char() {
    let source_text = "/** @typedef {\"日";
    let opts = SourceFileParseOptions {
        file_name: "/index.js".to_string(),
        path: Path("/index.js".to_string()),
        ..Default::default()
    };
    let file = parse_source_file(&opts, leak(source_text), ScriptKind::JS);
    let type_alias = file
        .statements()
        .nodes()
        .iter()
        .find(|&s| is_js_type_alias_declaration(s))
        .expect("the reparsed @typedef");
    let literal = type_alias.type_().literal();
    assert!(is_string_literal(literal));
    // Go: the literal ends 1 byte into `日` (bytes 15 to 17).
    assert_eq!((literal.pos(), literal.end()), (14, 16));
    assert_eq!(
        get_text_of_node_from_source_text(source_text, literal, false),
        go_string_from_bytes(b"\"\xE6".to_vec())
    );
}

// PORT: no Go counterpart. A missing `@typedef` name reports "Identifier
// expected" on the char before the name (reparser.go:49), Go `pos-1`: one
// Go byte back. The char before is the invalid byte FF, 1 Go byte and 7
// port bytes (`scanner_util::GO_STRING_MARKER`), so the error starts at the
// unit, not 1 port byte back inside it. Go N: (15, 16) in Go bytes. The
// port's start inside the unit ended after its start, and `--pretty` then
// panicked (a negative squiggle length).
#[test]
fn test_missing_typedef_name_error_starts_one_go_byte_back() {
    let source_text = go_string_from_bytes(b"/** @typedef {\"\xFFyz".to_vec());
    let opts = SourceFileParseOptions {
        file_name: "/index.js".to_string(),
        path: Path("/index.js".to_string()),
        ..Default::default()
    };
    let file = parse_source_file(&opts, leak(&source_text), ScriptKind::JS);
    let found: Vec<_> = file
        .diagnostics
        .iter()
        .filter(|d| d.code() == 1003)
        .map(|d| (d.pos(), d.end()))
        .collect();
    // The unit of FF is port bytes 15 to 22.
    assert_eq!(found, [(15, 22)]);
}

// Go: parser/parser_test.go:284 TestJSDocTypeSourcePropagatesToConstructedReparse
#[test]
fn test_js_doc_type_source_propagates_to_constructed_reparse() {
    in_child(
        module_path!(),
        "test_js_doc_type_source_propagates_to_constructed_reparse",
        js_doc_type_source_propagates_to_constructed_reparse,
    );
}

fn js_doc_type_source_propagates_to_constructed_reparse() {
    let source_text = r#"/**
 * @param {{
 *   value: string
 * }} options
 */
function foo(options) {}"#;
    let opts = SourceFileParseOptions {
        file_name: "/index.js".to_string(),
        path: Path("/index.js".to_string()),
        ..Default::default()
    };

    let file = Rc::new(parse_source_file(&opts, leak(source_text), ScriptKind::JS));
    note_parsed_source_file(&file);
    publish_parsed_files("/");
    let function = file.statements().nodes().get(0);
    assert!(is_function_declaration(function));
    assert_eq!(function.parameters().len(), 1);

    let type_node = function.parameters().get(0).type_();
    assert!(type_node.is_some());
    assert!(type_node.flags().intersects(NodeFlags::REPARSED));

    let expected = ["{", "value: string", "}"].join(NewLineKind::LF.get_new_line_character());
    assert_eq!(get_text_of_node(type_node), expected);
    assert_eq!(
        get_token_pos_of_node(type_node, file.root, false /*includeJSDoc*/),
        (source_text.find("{{").unwrap() + 1) as i32
    );
}

// PORT: no Go counterpart. Go `JSDoc` of a TS file parses a comment
// without `@see` or `@link` at the first call and keeps the nodes in
// `jsdocCache` (ast.go:2745 resolveJSDoc). So a second call gives the same
// nodes, and `EagerJSDoc` (ast.go:1581), which never parses, gives them from
// then on. The file is published (see `test_js_doc_import_type_parent_chain`),
// so the lazy parse is `program::resolve_lazy_js_doc`.
#[test]
fn test_lazy_js_doc_second_call_gives_the_same_nodes() {
    in_child(
        module_path!(),
        "test_lazy_js_doc_second_call_gives_the_same_nodes",
        || {
            let file = Rc::new(parse_lazy_js_doc_file());
            note_parsed_source_file(&file);
            publish_parsed_files("/");
            check_lazy_js_doc_calls(file.root);
        },
    );
}

// PORT: no Go counterpart. `test_lazy_js_doc_second_call_gives_the_same_nodes`
// for a file that is not published, as after Go `parser.ParseSourceFile`
// outside a program. Its lazy parse is `ast::resolve_file_store_js_doc`.
#[test]
fn test_lazy_js_doc_before_publish_second_call_gives_the_same_nodes() {
    let file = parse_lazy_js_doc_file();
    check_lazy_js_doc_calls(file.root);
}

/// A TS file with one lazy JSDoc comment (on `f`) and one that the parser
/// parses at once (on `g`, Go `withJSDoc`: it has `{@link}`).
fn parse_lazy_js_doc_file() -> ParsedSourceFile {
    let source_text = "/** Lazy. */\nfunction f() {}\n/** See {@link f}. */\nfunction g() {}\n";
    let opts = SourceFileParseOptions {
        file_name: "/index.ts".to_string(),
        path: Path("/index.ts".to_string()),
        ..Default::default()
    };
    parse_source_file(&opts, leak(source_text), ScriptKind::TS)
}

fn check_lazy_js_doc_calls(file: Node) {
    let statements = file.statements();
    let (f, g) = (statements.get(0), statements.get(1));
    assert!(f.eager_js_doc(file).is_empty());
    let eager_g = g.eager_js_doc(file).to_vec();
    assert_eq!(eager_g.len(), 1);
    assert_eq!(g.js_doc(file).to_vec(), eager_g);

    let first = f.js_doc(file).to_vec();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].parent(), f);
    assert_eq!(f.js_doc(file).to_vec(), first);
    assert_eq!(f.eager_js_doc(file).to_vec(), first);
}

// Go: parser/parser_test.go:311 TestSourceFilePositionMapWithNonASCIIStringLiteral
// PORT: renamed from TestSourceFileContainsNonASCIIInStringLiteralFastPath
// (old Rust name `test_source_file_contains_non_ascii_in_string_literal_fast_path`)
// by tsgo#4776, which also dropped the `ContainsNonASCII` assert.
#[test]
fn test_source_file_position_map_with_non_ascii_string_literal() {
    let source_text = "const x = \"─\";

namespace N {
  export const y = x;
}
";
    let opts = SourceFileParseOptions {
        file_name: "/index.ts".to_string(),
        path: Path("/index.ts".to_string()),
        ..Default::default()
    };

    let file = parse_source_file(&opts, leak(source_text), ScriptKind::TS);

    let position_map = source_file_get_position_map(file.root);
    assert!(!position_map.is_ascii_only());
    let after_box_drawing_character = (source_text.find('─').unwrap() + '─'.len_utf8()) as i32;
    assert_eq!(
        position_map.utf8_to_utf16(after_box_drawing_character),
        after_box_drawing_character - 2
    );
    assert_eq!(
        position_map.utf8_to_utf16(source_text.len() as i32),
        source_text.len() as i32 - 2
    );
}
