//! Ports of internal/ls/format_test.go, internal/ls/lsconv/converters_test.go,
//! internal/ls/lsutil/utilities_test.go and
//! internal/ls/lsutil/userpreferences_test.go.
//!
//! Not ported (blocked, see bugs/S3.md): TestUserPreferencesParseUnstable
//! and the `withConfig` subtest of TestUserPreferencesRoundtrip
//! (`UserPreferences::with_config` is `pub(crate)`).

use super::{Subtests, assert_equal, leak};
use indexmap::IndexMap;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::rc::Rc;
use ts_goport::astnav;
use ts_goport::format;
use ts_goport::frontend::json::{json_marshal, json_unmarshal};
use ts_goport::frontend::json_ext::LspAny;
use ts_goport::frontend::parser::{SourceFileParseOptions, parse_source_file};
use ts_goport::frontend::tspath::Path;
use ts_goport::gostd::context;
use ts_goport::ls::is_in_comment;
use ts_goport::ls::lsconv::{
    Converters, Script, ScriptText, compute_lsp_line_starts, file_name_to_document_uri,
    from_lsp_position, new_converters,
};
use ts_goport::ls::lsutil::{
    self, EditorSettings, FormatCodeSettings, IncludeInlayParameterNameHints, IndentStyle,
    InlayHintsPreferences, JsxAttributeCompletionStyle, OrganizeImportsCaseFirst,
    OrganizeImportsCollation, OrganizeImportsSort, OrganizeImportsTypeOrder, QuotePreference,
    SemicolonPreference, UserPreferences, new_default_user_preferences, parse_user_preferences,
    probably_uses_semicolons, resolve_organize_imports_sort,
};
use ts_goport::lsp::lsproto;
use ts_goport::modulespecifiers::{
    ImportModuleSpecifierEndingPreference, ImportModuleSpecifierPreference,
};
use ts_goport::options::Tristate;
use ts_goport::prelude::{Node, ScriptKind, TextRange};
use ts_goport::scanner_util::port_byte_offset;

/// Go `parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: name,
/// Path: name}, text, kind)`.
fn parse(name: &str, text: &str, kind: ScriptKind) -> Node {
    parse_source_file(
        &SourceFileParseOptions {
            file_name: name.to_string(),
            path: Path(name.to_string()),
            ..Default::default()
        },
        leak(text),
        kind,
    )
    .root
}

// ---------------------------------------------------------------------------
// ls/format_test.go
// ---------------------------------------------------------------------------

// Go: ls/format.go:226 getFormattingEditsAfterKeystroke
// PORT: the Go tests call the unexported LanguageService method with a nil
// program; the Rust method is private. The method only reads its arguments,
// so this is its body, with the same public format entry points.
fn get_formatting_edits_after_keystroke(
    file: Node,
    options: &FormatCodeSettings,
    position: i32,
    key: &str,
) -> Vec<ts_goport::frontend::core_textchange::TextChange> {
    let ctx = &format::with_format_code_settings(
        &context::background(),
        options,
        &options.editor_settings.new_line_character,
    );
    let token_at_position = astnav::get_token_at_position(file, position);
    if is_in_comment(file, position, token_at_position).is_none() {
        return match key {
            "{" => format::format_on_opening_curly(ctx, file, position),
            "}" => format::format_on_closing_curly(ctx, file, position),
            ";" => format::format_on_semicolon(ctx, file, position),
            "\n" => format::format_on_enter(ctx, file, position),
            _ => Vec::new(),
        };
    }
    Vec::new()
}

// Go: ls/format.go:207 getFormattingEditsForRange
// PORT: see `get_formatting_edits_after_keystroke`.
fn get_formatting_edits_for_range(
    file: Node,
    options: &FormatCodeSettings,
    r: TextRange,
) -> Vec<ts_goport::frontend::core_textchange::TextChange> {
    let ctx = &format::with_format_code_settings(
        &context::background(),
        options,
        &options.editor_settings.new_line_character,
    );
    format::format_selection(ctx, file, r.pos(), r.end())
}

// Go: ls/format_test.go:59 TestGetFormattingEditsAfterKeystroke_EmptyFile
/// Test for issue: Panic Handling textDocument/onTypeFormatting
/// This reproduces the panic when pressing enter in an empty file
#[test]
fn test_get_formatting_edits_after_keystroke_empty_file() {
    // Create an empty file
    let text = "";
    let source_file = parse("/index.ts", text, ScriptKind::TS);

    let options = lsutil::get_default_format_code_settings();

    // This should not panic
    let _edits = get_formatting_edits_after_keystroke(
        source_file,
        &options,
        0, // position
        "\n",
    );
}

// Go: ls/format_test.go:89 TestGetFormattingEditsAfterKeystroke_SimpleStatement
/// Test with a simple statement
#[test]
fn test_get_formatting_edits_after_keystroke_simple_statement() {
    let text = "const x = 1";
    let source_file = parse("/index.ts", text, ScriptKind::TS);

    let options = lsutil::get_default_format_code_settings();

    // This should not panic
    let _edits = get_formatting_edits_after_keystroke(
        source_file,
        &options,
        text.len() as i32, // position at end of file
        "\n",
    );
}

// Go: ls/format_test.go:120 TestGetFormattingEditsForRange_FunctionBody
/// Test for issue: Crash in range formatting when requested on a line that is different from the containing function
/// This reproduces the panic when formatting a range inside a function body
#[test]
fn test_get_formatting_edits_for_range_function_body() {
    // (name, text, startPos, endPos)
    #[rustfmt::skip]
    let test_cases: &[(&str, &str, i32, i32)] = &[
        (
            "return statement in function",
            "function foo() {\n    return (1  + 2);\n}",
            21, // Start of "return"
            38, // End of ");"
        ),
        (
            "function with newline after keyword",
            "function\nf() {\n}",
            9,  // After "function\n"
            13, // Inside or after function
        ),
        (
            "empty function body",
            "function f() {\n  \n}",
            15, // Inside body
            17, // Inside body
        ),
        (
            "after function closing brace",
            "function f() {\n}",
            15, // After closing brace
            15,
        ),
    ];
    let mut t = Subtests::new("TestGetFormattingEditsForRange_FunctionBody");
    for &(name, text, start_pos, end_pos) in test_cases {
        t.run(name, || {
            let source_file = parse("/test.ts", text, ScriptKind::TS);
            let options = lsutil::get_default_format_code_settings();
            // This should not panic
            let _edits = get_formatting_edits_for_range(
                source_file,
                &options,
                TextRange::new(start_pos, end_pos),
            );
            Ok(())
        });
    }
    t.finish();
}

// ---------------------------------------------------------------------------
// ls/lsconv/converters_test.go
// ---------------------------------------------------------------------------

// Go: ls/lsconv/converters_test.go:22 TestDocumentURIToFileName
#[test]
fn test_document_uri_to_file_name() {
    #[rustfmt::skip]
    let tests: &[(&str, &str)] = &[
        ("file:///path/to/file.ts", "/path/to/file.ts"),
        ("file://server/share/file.ts", "//server/share/file.ts"),
        ("file:///d%3A/work/tsgo932/lib/utils.ts", "d:/work/tsgo932/lib/utils.ts"),
        ("file:///D%3A/work/tsgo932/lib/utils.ts", "D:/work/tsgo932/lib/utils.ts"),
        ("file:///d%3A/work/tsgo932/app/%28test%29/comp/comp-test.tsx", "d:/work/tsgo932/app/(test)/comp/comp-test.tsx"),
        ("file:///path/to/file.ts#section", "/path/to/file.ts"),
        ("file:///c:/test/me", "c:/test/me"),
        ("file://shares/files/c%23/p.cs", "//shares/files/c#/p.cs"),
        ("file:///c:/Source/Z%C3%BCrich%20or%20Zurich%20(%CB%88zj%CA%8A%C9%99r%C9%AAk,/Code/resources/app/plugins/c%23/plugin.json", "c:/Source/Zürich or Zurich (ˈzjʊərɪk,/Code/resources/app/plugins/c#/plugin.json"),
        ("file:///c:/test %25/path", "c:/test %/path"),
        // {"file:?q", "/"},
        ("file:///_:/path", "/_:/path"),
        ("file:///users/me/c%23-projects/", "/users/me/c#-projects"),
        ("file:///a/../b.ts", "/b.ts"),
        ("file://localhost/c%24/GitDevelopment/express", "//localhost/c$/GitDevelopment/express"),
        ("file:///c%3A/test%20with%20%2525/c%23code", "c:/test with %25/c#code"),

        ("untitled:Untitled-1", "^/~ts-uri~/untitled/ts-nul-authority/Untitled-1"),
        ("untitled:Untitled-1#fragment", "^/~ts-uri~/untitled/ts-nul-authority/~ts-uri-escape~556e7469746c65642d310023667261676d656e74~"),
        ("untitled:c:/Users/jrieken/Code/abc.txt", "^/~ts-uri~/untitled/ts-nul-authority/~ts-uri-escape~633a~/Users/jrieken/Code/abc.txt"),
        ("untitled:C:/Users/jrieken/Code/abc.txt", "^/~ts-uri~/untitled/ts-nul-authority/~ts-uri-escape~433a~/Users/jrieken/Code/abc.txt"),
        ("untitled://wsl%2Bubuntu/home/jabaile/work/TypeScript/newfile.ts", "^/~ts-uri~/untitled/wsl%2Bubuntu/home/jabaile/work/TypeScript/newfile.ts"),
    ];
    let mut t = Subtests::new("TestDocumentURIToFileName");
    for &(uri, file_name) in tests {
        t.run(uri, || {
            assert_equal(
                lsproto::DocumentUri(uri.to_string()).file_name().as_str(),
                file_name,
                "uri.FileName()",
            )
        });
    }
    t.finish();
}

// Go: ls/lsconv/converters_test.go:97 TestNonFileDocumentURIRoundTripsThroughNormalizedFileName (ts#64544)
// PORT: the Directory and PathKey asserts (:117-189) need the typed
// RootedFilePath methods; the port compares the file names.
#[test]
fn test_non_file_document_uri_round_trips_through_normalized_file_name() {
    let file_name = |uri: &str| lsproto::DocumentUri(uri.to_string()).file_name();
    assert_eq!(
        file_name("custom:folder/../~ts-uri~/caf\u{e9}\\file.ts"),
        "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~2e2e~/~ts-uri~/~ts-uri-escape~636166c3a95c66696c65~.ts"
    );
    assert_eq!(
        file_name("custom:.git/file.ts"),
        "^/~ts-uri~/custom/ts-nul-authority/.git/file.ts"
    );
    assert_eq!(
        file_name("custom:~ts-uri-escape~dir.js/file.ts?x=1"),
        "^/~ts-uri~/custom/ts-nul-authority/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~/~ts-uri-escape~66696c65003f783d31~.ts"
    );
    let mut t = Subtests::new("TestNonFileDocumentURIRoundTripsThroughNormalizedFileName");
    for uri in [
        "untitled:folder/../file.ts",
        "vscode-vfs://github/path//file.ts",
        "custom:/path/./file.ts/",
        "custom:",
        "custom:///path",
        "custom://authority",
        "custom://authority/",
        "custom:path/file.ts?rev=a/b#frag/c",
        "custom://authority/path/file.ts#frag/a",
        "custom:path\\file.ts",
        "custom:.git/file.ts",
        "custom:..hidden/file.ts",
        "custom://~ts-uri~/path",
        "custom://ts-nul-authority/path",
        "custom:~ts-uri-escape~file.ts",
        "custom:~ts-uri-escape~no-path",
        "custom://authority/~ts-uri-no-path~~",
        "custom:~ts-uri-spec~666f6f~/file.ts?x=1",
        "custom:folder/../~ts-uri~/caf\u{e9}\\file.ts",
        "custom:name.ts\\",
        "custom:name..ts",
    ] {
        t.run(uri, || {
            assert_equal(
                file_name_to_document_uri(&file_name(uri)).0.as_str(),
                uri,
                "lsconv.FileNameToDocumentURI(uri.FileName())",
            )
        });
    }
    t.finish();
    for uri in [
        "custom:path\\file.ts",
        "custom:~ts-uri~file.ts",
        "custom:~ts-uri-escape~file.ts",
    ] {
        assert_eq!(
            ts_goport::frontend::tspath::try_get_extension_from_path(&file_name(uri)),
            ".ts",
            "{uri}"
        );
    }
    assert!(file_name("custom:~ts-uri-escape~types.d.css.ts").ends_with(".d.css.ts"));
    // Go :191-203: a dynamic name without the `~ts-uri~` part keeps its
    // escape text, and an escape that is not UTF-8 stays an escape.
    assert_eq!(
        file_name_to_document_uri("^/custom/ts-nul-authority/~ts-uri-escape~666f6f~.ts").0,
        "custom:~ts-uri-escape~666f6f~.ts"
    );
    assert_eq!(
        file_name_to_document_uri("^/~ts-uri~/custom/ts-nul-authority/~ts-uri-escape~ff~").0,
        "custom:~ts-uri-escape~ff~"
    );
    assert_ne!(file_name("custom:name.ts\\"), file_name("custom:name..ts"));
}

// Go: ls/lsconv/converters_test.go:61 TestFileNameToDocumentURI
#[test]
fn test_file_name_to_document_uri() {
    #[rustfmt::skip]
    let tests: &[(&str, &str)] = &[
        ("/path/to/file.ts", "file:///path/to/file.ts"),
        ("//server/share/file.ts", "file://server/share/file.ts"),
        ("d:/work/tsgo932/lib/utils.ts", "file:///d%3A/work/tsgo932/lib/utils.ts"),
        ("D:/work/tsgo932/lib/utils.ts", "file:///d%3A/work/tsgo932/lib/utils.ts"),
        ("d:/work/tsgo932/app/(test)/comp/comp-test.tsx", "file:///d%3A/work/tsgo932/app/%28test%29/comp/comp-test.tsx"),
        ("/path/to/file.ts", "file:///path/to/file.ts"),
        ("c:/test/me", "file:///c%3A/test/me"),
        ("//shares/files/c#/p.cs", "file://shares/files/c%23/p.cs"),
        ("c:/Source/Zürich or Zurich (ˈzjʊərɪk,/Code/resources/app/plugins/c#/plugin.json", "file:///c%3A/Source/Z%C3%BCrich%20or%20Zurich%20%28%CB%88zj%CA%8A%C9%99r%C9%AAk%2C/Code/resources/app/plugins/c%23/plugin.json"),
        ("c:/test %/path", "file:///c%3A/test%20%25/path"),
        ("/", "file:///"),
        ("/_:/path", "file:///_%3A/path"),
        // PORT: Go N' passes RootedFilePathFromAbsolute("/users/me/c#-projects/"),
        // which drops the trailing separator (ts#64159).
        ("/users/me/c#-projects", "file:///users/me/c%23-projects"),
        ("//localhost/c$/GitDevelopment/express", "file://localhost/c%24/GitDevelopment/express"),
        ("c:/test with %25/c#code", "file:///c%3A/test%20with%20%2525/c%23code"),

        ("^/untitled/ts-nul-authority/Untitled-1", "untitled:Untitled-1"),
        ("^/untitled/ts-nul-authority/c:/Users/jrieken/Code/abc.txt", "untitled:c:/Users/jrieken/Code/abc.txt"),
        ("^/untitled/wsl%2Bubuntu/home/jabaile/work/TypeScript/newfile.ts", "untitled://wsl%2Bubuntu/home/jabaile/work/TypeScript/newfile.ts"),
    ];
    let mut t = Subtests::new("TestFileNameToDocumentURI");
    for &(file_name, uri) in tests {
        t.run(file_name, || {
            assert_equal(
                file_name_to_document_uri(file_name).0.as_str(),
                uri,
                "lsconv.FileNameToDocumentURI(fileName)",
            )
        });
    }
    t.finish();
}

/// Go `testScript`.
struct TestScript {
    name: String,
    text: String,
}

impl Script for TestScript {
    fn file_name(&self) -> &str {
        &self.name
    }
    fn text(&self) -> ScriptText<'_> {
        ScriptText::Borrowed(&self.text)
    }
}

// Go: ls/lsconv/converters_test.go:232 newTestConverters
fn new_test_converters(text: &str) -> (Rc<Converters>, TestScript) {
    let script = TestScript {
        name: "test.ts".to_string(),
        text: text.to_string(),
    };
    let line_map = compute_lsp_line_starts(text);
    let conv = new_converters(lsproto::PositionEncodingKind::UTF16, move |_: &str| {
        Some(Rc::clone(&line_map))
    });
    (conv, script)
}

fn position(line: u32, character: u32) -> lsproto::Position {
    lsproto::Position { line, character }
}

/// Go `lsconv.FromLSPPosition(conv, script, lc, spanmap.FeatureAll)` with the
/// test's `assert.Equal(t, len(positions), 1)`: the one position, or an error.
fn from_lsp_position_all(
    conv: &Converters,
    script: &TestScript,
    lc: lsproto::Position,
) -> Result<i32, String> {
    let positions = from_lsp_position(conv, script, lc, ts_goport::spanmap::Feature::ALL);
    match positions.as_slice() {
        [mapped] => Ok(mapped.position),
        _ => Err(format!(
            "FromLSPPosition({lc:?}) gave {} positions, want 1",
            positions.len()
        )),
    }
}

// Go: ls/lsconv/converters_test.go:279 TestConvertersInvalidUTF8
/// TestConvertersInvalidUTF8 verifies behavior on text containing invalid UTF-8
/// sequences (e.g. lone continuation bytes). Node's TextDecoder substitutes such
/// bytes with U+FFFD, so the JS-reference test cannot cover this; we assert the
/// expected Go-side behavior directly. Each invalid byte advances the byte
/// position by 1 and the UTF-16 character by 1 (RuneError = 1 code unit).
// PORT: a Rust `&str` cannot hold the byte 0x80. The text is the port form
// of the Go bytes (`go_string_from_bytes`), where an invalid byte is a unit
// of several bytes, so each Go byte position is converted to its port
// offset (`port_byte_offset`). Lines and characters are Go values.
#[test]
fn test_converters_invalid_utf8() {
    // Text with invalid UTF-8 byte 0x80 (continuation byte without start byte).
    // Old code used utf8.RuneLen(RuneError)==3, overshooting the byte offset.
    let go_bytes: &[u8] = b"a\x80b\ncd";
    let text = ts_goport::scanner_util::go_string_from_bytes(go_bytes.to_vec());
    let (conv, script) = new_test_converters(&text);
    let port = |go_pos: i32| port_byte_offset(&text, go_pos);

    // (line, char) → byte position. Each row asserts both directions where the
    // position lies on a character boundary.
    #[rustfmt::skip]
    let mappings: &[(u32, u32, i32)] = &[
        (0, 0, 0), // 'a'
        (0, 1, 1), // invalid byte 0x80
        (0, 2, 2), // 'b'
        (0, 3, 3), // newline (line end)
        (1, 0, 4), // 'c'
        (1, 1, 5), // 'd'
        (1, 2, 6), // EOF
    ];
    let mut errors = Vec::new();
    for &(line, char, byte_pos) in mappings {
        let lc = position(line, char);
        match from_lsp_position_all(&conv, &script, lc) {
            Ok(got) if got != port(byte_pos) => errors.push(format!(
                "LineAndCharacterToPosition({line},{char}) = {got} (port offset), want Go {byte_pos} (port {})",
                port(byte_pos)
            )),
            Ok(_) => {}
            Err(err) => errors.push(err),
        }
        let (got, _) = conv.to_lsp_position(&script, port(byte_pos));
        if got != lc {
            errors.push(format!(
                "PositionToLineAndCharacter({byte_pos}) = {got:?}, want {lc:?}"
            ));
        }
    }

    // Byte-by-byte round-trip across the entire text.
    for byte_pos in 0..=go_bytes.len() as i32 {
        let (lc, _) = conv.to_lsp_position(&script, port(byte_pos));
        match from_lsp_position_all(&conv, &script, lc) {
            Ok(rt) if rt != port(byte_pos) => errors.push(format!(
                "round-trip byte {byte_pos}: got port offset {rt}, want {}",
                port(byte_pos)
            )),
            Ok(_) => {}
            Err(err) => errors.push(err),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

// Go: ls/lsconv/converters_test.go:340 jsReferenceScript
/// jsReferenceScript is a Node.js script that, given a list of UTF-8 byte buffers,
/// computes the authoritative mapping between (line, character in UTF-16 code units)
/// and UTF-8 byte offsets. See the Go file for the full description.
const JS_REFERENCE_SCRIPT: &str = r#"
const inChunks = [];
process.stdin.on('data', c => inChunks.push(c));
process.stdin.on('end', () => {
  const buf = Buffer.concat(inChunks);
  let off = 0;
  const readU32 = () => { const v = buf.readUInt32LE(off); off += 4; return v; };
  const n = readU32();
  const buffers = [];
  for (let i = 0; i < n; i++) {
    const len = readU32();
    buffers.push(buf.subarray(off, off + len));
    off += len;
  }

  const decoder = new TextDecoder('utf-8', { fatal: true });
  const out = buffers.map(bytes => {
    // Decode the raw UTF-8 bytes to a JS string (this is what sys.ts does with file contents).
    const text = decoder.decode(bytes);

    // LSP line starts in the *decoded* JS string: \n, \r, \r\n only.
    const lineStartsJs = [0];
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      if (c === 13) {
        if (i + 1 < text.length && text.charCodeAt(i + 1) === 10) i++;
        lineStartsJs.push(i + 1);
      } else if (c === 10) {
        lineStartsJs.push(i + 1);
      }
    }

    // Walk the original UTF-8 byte buffer to find codepoint boundaries. Inputs are
    // valid UTF-8, so we advance bytePos by the sequence length of each lead byte
    // and jsIdx by the corresponding UTF-16 code unit count (1 for BMP, 2 for
    // surrogate pair) of the codepoint at jsIdx in the decoded string.
    const boundaries = [{ bytePos: 0, jsIdx: 0 }];
    let bytePos = 0, jsIdx = 0;
    while (bytePos < bytes.length) {
      const seq = utf8SeqLen(bytes[bytePos]);
      const cp = text.codePointAt(jsIdx);
      bytePos += seq;
      jsIdx += cp > 0xFFFF ? 2 : 1;
      boundaries.push({ bytePos, jsIdx });
    }

    return boundaries.map(({ bytePos, jsIdx }) => {
      let lo = 0, hi = lineStartsJs.length - 1;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (lineStartsJs[mid] <= jsIdx) lo = mid;
        else hi = mid - 1;
      }
      return { bytePos, line: lo, char: jsIdx - lineStartsJs[lo] };
    });
  });

  process.stdout.write(JSON.stringify(out));
});

function utf8SeqLen(b) {
  if (b < 0x80) return 1;
  if ((b & 0xE0) === 0xC0) return 2;
  if ((b & 0xF0) === 0xE0) return 3;
  if ((b & 0xF8) === 0xF0) return 4;
  throw new Error('invalid UTF-8 lead byte 0x' + b.toString(16));
}
"#;

/// Go `jsTuple`: (bytePos, line, char).
type JsTuple = (i32, u32, u32);

// Go: ls/lsconv/converters_test.go:415 runJSReference
/// None when node is not available (Go `t.Skipf`).
fn run_js_reference(texts: &[&str]) -> Option<Vec<Vec<JsTuple>>> {
    // Build a length-prefixed binary stream of the raw UTF-8 bytes:
    // [uint32 LE count] then for each: [uint32 LE length][bytes].
    let mut input = Vec::new();
    input.extend_from_slice(&(texts.len() as u32).to_le_bytes());
    for s in texts {
        input.extend_from_slice(&(s.len() as u32).to_le_bytes());
        input.extend_from_slice(s.as_bytes());
    }

    let mut child = match Command::new("node")
        .args(["-e", JS_REFERENCE_SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            println!("SKIP TestConvertersAgainstJSReference: node not available: {err}");
            return None;
        }
    };
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&input)
        .expect("write node stdin");
    let output = child.wait_with_output().expect("node output");
    assert!(
        output.status.success(),
        "node failed: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    // PORT: Go unmarshals `[][]jsTuple`. The output is JSON.stringify of
    // arrays of `{bytePos, line, char}` objects, whose keys come in that
    // order, so the numbers are read in groups of three per inner array.
    let text = String::from_utf8(output.stdout).expect("node output is UTF-8");
    let mut out: Vec<Vec<JsTuple>> = Vec::new();
    let mut depth = 0;
    let mut nums: Vec<i64> = Vec::new();
    let mut num = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '-' {
            num.push(ch);
            continue;
        }
        if !num.is_empty() {
            nums.push(num.parse().expect("number"));
            num.clear();
        }
        match ch {
            '[' => {
                depth += 1;
                if depth == 2 {
                    nums.clear();
                }
            }
            ']' => {
                if depth == 2 {
                    assert!(nums.len() % 3 == 0, "unexpected node output: {text}");
                    out.push(
                        nums.chunks(3)
                            .map(|c| (c[0] as i32, c[1] as u32, c[2] as u32))
                            .collect(),
                    );
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    Some(out)
}

// Go: ls/lsconv/converters_test.go:449 TestConvertersAgainstJSReference
/// TestConvertersAgainstJSReference cross-checks the Go UTF-16 conversions against
/// authoritative results computed by Node.js using real UTF-16 string semantics.
#[test]
fn test_converters_against_js_reference() {
    #[rustfmt::skip]
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("ascii", "hello\nworld"),
        ("ascii_crlf", "hello\r\nworld\r\n!"),
        ("ascii_cr_only", "a\rb\rc"),
        ("trailing_newline", "abc\n"),
        ("bmp_em_dash", "ab\u{2014}cd\nef"),
        ("bmp_multi", "α\nβ\nγδε\nzz"),
        ("supplementary_emoji", "x\u{1F600}y\nz"), // 😀 is 4 UTF-8 bytes, 2 UTF-16 units
        ("supplementary_at_lineend", "ab\u{1F600}\ncd\u{1F60A}"),
        ("supplementary_only", "\u{1F600}\u{1F601}\u{1F602}"),
        ("mixed", "α — \u{1F600}\r\nβ\nγ\r"),
        ("long_mixed_ws", "  \tαβ\n\t\u{1F600}  end\n"),
        ("zwj_emoji", "\u{1F468}\u{200D}\u{1F4BB}\nnext"),
        ("only_newlines", "\n\n\r\n\r"),
    ];

    let texts: Vec<&str> = cases.iter().map(|c| c.1).collect();
    let Some(refs) = run_js_reference(&texts) else {
        return;
    };
    assert_eq!(refs.len(), cases.len());

    let mut t = Subtests::new("TestConvertersAgainstJSReference");
    for (i, &(name, text)) in cases.iter().enumerate() {
        let reference = &refs[i];
        t.run(name, || {
            let (conv, script) = new_test_converters(text);
            for &(byte_pos, line, char) in reference {
                let expected_lc = position(line, char);

                let (got_lc, _) = conv.to_lsp_position(&script, byte_pos);
                if got_lc != expected_lc {
                    return Err(format!(
                        "PositionToLineAndCharacter({byte_pos}) mismatch in {text:?}: got {got_lc:?}, want {expected_lc:?}"
                    ));
                }

                let got_pos = from_lsp_position_all(&conv, &script, expected_lc)?;
                if got_pos != byte_pos {
                    return Err(format!(
                        "LineAndCharacterToPosition({line},{char}) mismatch in {text:?}: got {got_pos}, want {byte_pos}"
                    ));
                }
            }
            Ok(())
        });
    }
    t.finish();
}

// ---------------------------------------------------------------------------
// ls/lsutil/utilities_test.go
// ---------------------------------------------------------------------------

// Go: ls/lsutil/utilities_test.go:19 TestProbablyUsesSemicolons
#[test]
fn test_probably_uses_semicolons() {
    #[rustfmt::skip]
    let tests: &[(&str, &str, bool)] = &[
        (
            "mixed semicolons and ASI favors semicolons when ratio exceeds one fifth",
            // First five observations: 2 with semicolon, 3 without. Real ratio 2/3 > 1/5.
            // Integer division bug compared against 1/5==0 and used with/without as ints,
            // so the old check was effectively (with/without) > 0, which failed here.
            "let a = 1;
let b = 2;
let c = 3
let d = 4
let e = 5
",
            true,
        ),
        (
            "consistent ASI with no semicolons",
            "let a = 1
let b = 2
let c = 3
",
            false,
        ),
        (
            "consistent semicolons",
            "let a = 1;
let b = 2;
let c = 3;
",
            true,
        ),
    ];
    let mut t = Subtests::new("TestProbablyUsesSemicolons");
    for &(name, src, want) in tests {
        t.run(name, || {
            let file = parse("/test.ts", src, ScriptKind::TS);
            let got = probably_uses_semicolons(file);
            if got != want {
                return Err(format!("ProbablyUsesSemicolons() = {got}, want {want}"));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: ls/lsutil/utilities_test.go:69 TestResolveOrganizeImportsSort
#[test]
fn test_resolve_organize_imports_sort() {
    let unicode = OrganizeImportsCollation::UNICODE;
    let tests: Vec<(&str, UserPreferences, OrganizeImportsSort)> = vec![
        (
            "explicit sort wins",
            UserPreferences {
                organize_imports_sort: OrganizeImportsSort::ORDINAL,
                organize_imports_collation: unicode,
                organize_imports_ignore_case: Tristate::True,
                ..Default::default()
            },
            OrganizeImportsSort::ORDINAL,
        ),
        (
            "unicode case-sensitive maps to natural",
            UserPreferences {
                organize_imports_collation: unicode,
                organize_imports_ignore_case: Tristate::False,
                ..Default::default()
            },
            OrganizeImportsSort::NATURAL,
        ),
        (
            "unicode ignore case maps to natural ignore case",
            UserPreferences {
                organize_imports_collation: unicode,
                organize_imports_ignore_case: Tristate::True,
                ..Default::default()
            },
            OrganizeImportsSort::NATURAL_IGNORE_CASE,
        ),
        (
            "unicode unknown case sensitivity stays auto for detection",
            UserPreferences {
                organize_imports_collation: unicode,
                ..Default::default()
            },
            OrganizeImportsSort::AUTO,
        ),
        (
            "ordinal ignore case maps to ordinal ignore case",
            UserPreferences {
                organize_imports_ignore_case: Tristate::True,
                ..Default::default()
            },
            OrganizeImportsSort::ORDINAL_IGNORE_CASE,
        ),
        (
            "ordinal case sensitive maps to ordinal",
            UserPreferences {
                organize_imports_ignore_case: Tristate::False,
                ..Default::default()
            },
            OrganizeImportsSort::ORDINAL,
        ),
        (
            "unknown ordinal stays auto",
            UserPreferences::default(),
            OrganizeImportsSort::AUTO,
        ),
    ];

    let mut t = Subtests::new("TestResolveOrganizeImportsSort");
    for (name, preferences, want) in &tests {
        t.run(name, || {
            let got = resolve_organize_imports_sort(preferences);
            if got != *want {
                return Err(format!(
                    "ResolveOrganizeImportsSort() = {got:?}, want {want:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: ls/lsutil/utilities_test.go:139 TestCompareOrganizeImportsNaturalStrings
// PORT: in `src/ls/lsutil/organizeimports.rs` (it calls an unexported Go
// function).

// ---------------------------------------------------------------------------
// ls/lsutil/userpreferences_test.go
// ---------------------------------------------------------------------------

// Go: ls/lsutil/userpreferences_test.go:101 fillNonZeroValues
// Go: ls/lsutil/userpreferences_test.go:128 getValidStringValue
// PORT: Go fills every settable field by reflection: bool true, ints 1,
// unsigned ints (core.Tristate) 1 (TSFalse), a valid value for the string
// enums, "test" for other strings, []string{"test"}, and nested structs the
// same way. Rust has no reflection, so each field of the Go struct is set
// here by name. A Go field with no Rust field fails to compile.
fn fill_non_zero_values() -> UserPreferences {
    let f = Tristate::False; // Go uint8 1
    let test = || vec!["test".to_string()];
    UserPreferences {
        format_code_settings: FormatCodeSettings {
            editor_settings: EditorSettings {
                base_indent_size: 1,
                indent_size: 1,
                tab_size: 1,
                new_line_character: "test".to_string(),
                convert_tabs_to_spaces: f,
                indent_style: IndentStyle(1),
                trim_trailing_whitespace: f,
            },
            insert_space_after_comma_delimiter: f,
            insert_space_after_semicolon_in_for_statements: f,
            insert_space_before_and_after_binary_operators: f,
            insert_space_after_constructor: f,
            insert_space_after_keywords_in_control_flow_statements: f,
            insert_space_after_function_keyword_for_anonymous_functions: f,
            insert_space_after_opening_and_before_closing_nonempty_parenthesis: f,
            insert_space_after_opening_and_before_closing_nonempty_brackets: f,
            insert_space_after_opening_and_before_closing_nonempty_braces: f,
            insert_space_after_opening_and_before_closing_empty_braces: f,
            insert_space_after_opening_and_before_closing_template_string_braces: f,
            insert_space_after_opening_and_before_closing_jsx_expression_braces: f,
            insert_space_after_type_assertion: f,
            insert_space_before_function_parenthesis: f,
            place_open_brace_on_new_line_for_functions: f,
            place_open_brace_on_new_line_for_control_blocks: f,
            insert_space_before_type_annotation: f,
            indent_multi_line_object_literal_beginning_on_blank_line: f,
            semicolons: SemicolonPreference::INSERT,
            indent_switch_case: f,
        },
        quote_preference: QuotePreference::SINGLE,
        lazy_configured_projects_from_external_project: f,
        maximum_hover_length: 1,
        include_completions_for_module_exports: f,
        include_completions_for_import_statements: f,
        include_automatic_optional_chain_completions: f,
        include_completions_with_class_member_snippets: f,
        include_completions_with_object_literal_method_snippets: f,
        jsx_attribute_completion_style: JsxAttributeCompletionStyle::BRACES,
        enable_auto_closing_tags: f,
        enable_js_doc_completions: f,
        generate_return_in_doc_template: f,
        import_module_specifier_preference: ImportModuleSpecifierPreference::Relative,
        import_module_specifier_ending: ImportModuleSpecifierEndingPreference::Js,
        auto_import_specifier_exclude_regexes: test(),
        auto_import_file_exclude_patterns: test(),
        auto_import_entrypoint_directory_search: f,
        prefer_type_only_auto_imports: f,
        organize_imports_sort: OrganizeImportsSort(1),
        organize_imports_ignore_case: f,
        organize_imports_collation: OrganizeImportsCollation(true),
        organize_imports_locale: "test".to_string(),
        organize_imports_numeric_collation: f,
        organize_imports_accent_collation: f,
        organize_imports_case_first: OrganizeImportsCaseFirst(1),
        organize_imports_type_order: OrganizeImportsTypeOrder(1),
        allow_text_changes_in_new_files: f,
        use_aliases_for_rename: f,
        allow_rename_of_import_path: f,
        provide_refactor_not_applicable_reason: f,
        inlay_hints: InlayHintsPreferences {
            include_inlay_parameter_name_hints: IncludeInlayParameterNameHints::ALL,
            include_inlay_parameter_name_hints_when_argument_matches_name: f,
            include_inlay_function_parameter_type_hints: f,
            include_inlay_variable_type_hints: f,
            include_inlay_variable_type_hints_when_type_matches_name: f,
            include_inlay_property_declaration_type_hints: f,
            include_inlay_function_like_return_type_hints: f,
            include_inlay_enum_member_value_hints: f,
        },
        code_lens: lsutil::CodeLensUserPreferences {
            references_code_lens_enabled: f,
            implementations_code_lens_enabled: f,
            references_code_lens_show_on_all_functions: f,
            implementations_code_lens_show_on_interface_methods: f,
            implementations_code_lens_show_on_all_class_methods: f,
        },
        prefer_go_to_source_definition: true,
        exclude_library_symbols_in_nav_to: f,
        // ts#64554: Go getValidStringValue gives lsutil.WorkspaceSymbolsScope a valid value.
        workspace_symbols_scope: lsutil::WorkspaceSymbolsScope::ALL_OPEN_PROJECTS,
        enable_formatting: f,
        enable_validation: f,
        disable_suggestions: f,
        disable_line_text_in_references: f,
        display_parts_for_js_doc: f,
        report_style_checks_as_warnings: f,
        locale: "test".to_string(),
        disable_automatic_type_acquisition: f,
        automatic_type_acquisition_enabled: f,
        custom_config_file_name: "test".to_string(),
    }
}

// Go: ls/lsutil/userpreferences_test.go:150 TestUserPreferencesRoundtrip
// PORT: the `withConfig` subtest is not ported (see the module comment).
#[test]
fn test_user_preferences_roundtrip() {
    let original = fill_non_zero_values();

    let json_bytes = json_marshal(&original, &[]).expect("marshal");

    let mut t = Subtests::new("TestUserPreferencesRoundtrip");
    t.run("UnmarshalJSONFrom", || {
        let mut parsed = UserPreferences::default();
        json_unmarshal(json_bytes.as_bytes(), &mut parsed, &[]).map_err(|err| format!("{err:?}"))?;
        if original != parsed {
            return Err(format!(
                "assert.DeepEqual(original, parsed) failed\njson: {json_bytes}\noriginal: {original:#?}\nparsed: {parsed:#?}"
            ));
        }
        Ok(())
    });
    t.finish();
}

/// Go `json.Unmarshal(json.Marshal(prefs), &actual)` with `actual
/// map[string]any`.
fn marshal_to_map(prefs: &UserPreferences) -> IndexMap<String, LspAny> {
    let json_bytes = json_marshal(prefs, &[]).expect("marshal");
    let mut actual: IndexMap<String, LspAny> = IndexMap::new();
    json_unmarshal(json_bytes.as_bytes(), &mut actual, &[]).expect("unmarshal");
    actual
}

/// Go `m[key].(map[string]any)`.
fn object<'a>(
    m: &'a IndexMap<String, LspAny>,
    key: &str,
) -> Result<&'a IndexMap<String, LspAny>, String> {
    match m.get(key) {
        Some(LspAny::Object(o)) => Ok(o),
        other => Err(format!(
            "interface conversion: {key} is {other:?}, not map[string]any"
        )),
    }
}

// Go: ls/lsutil/userpreferences_test.go:177 TestUserPreferencesSerialize
#[test]
fn test_user_preferences_serialize() {
    let mut t = Subtests::new("TestUserPreferencesSerialize");

    t.run("config path field serializes to nested path", || {
        let prefs = UserPreferences {
            quote_preference: QuotePreference::SINGLE,
            ..Default::default()
        };
        let actual = marshal_to_map(&prefs);

        let preferences = object(&actual, "preferences")?;
        assert_equal(
            preferences.get("quoteStyle"),
            Some(&LspAny::String("single".into())),
            "quoteStyle",
        )
    });

    t.run("raw-only field serializes to unstable section", || {
        let prefs = UserPreferences {
            disable_suggestions: Tristate::True,
            ..Default::default()
        };
        let actual = marshal_to_map(&prefs);

        let unstable = object(&actual, "unstable")?;
        assert_equal(
            unstable.get("disableSuggestions"),
            Some(&LspAny::Bool(true)),
            "disableSuggestions",
        )
    });

    t.run("inlay hint inversion on serialize", || {
        let prefs = UserPreferences {
            inlay_hints: InlayHintsPreferences {
                include_inlay_parameter_name_hints: IncludeInlayParameterNameHints::ALL,
                include_inlay_parameter_name_hints_when_argument_matches_name: Tristate::True,
                ..Default::default()
            },
            ..Default::default()
        };
        let actual = marshal_to_map(&prefs);

        let inlay_hints = object(&actual, "inlayHints")?;
        let parameter_names = object(inlay_hints, "parameterNames")?;
        assert_equal(
            parameter_names.get("enabled"),
            Some(&LspAny::String("all".into())),
            "enabled",
        )?;
        assert_equal(
            parameter_names.get("suppressWhenArgumentMatchesName"),
            Some(&LspAny::Bool(false)), // inverted
            "suppressWhenArgumentMatchesName",
        )
    });

    t.run("mixed config and unstable fields", || {
        let prefs = UserPreferences {
            quote_preference: QuotePreference::SINGLE,
            disable_suggestions: Tristate::True,
            display_parts_for_js_doc: Tristate::True,
            ..Default::default()
        };
        let actual = marshal_to_map(&prefs);

        let preferences = object(&actual, "preferences")?;
        assert_equal(
            preferences.get("quoteStyle"),
            Some(&LspAny::String("single".into())),
            "quoteStyle",
        )?;

        let unstable = object(&actual, "unstable")?;
        assert_equal(
            unstable.get("disableSuggestions"),
            Some(&LspAny::Bool(true)),
            "disableSuggestions",
        )?;
        assert_equal(
            unstable.get("displayPartsForJSDoc"),
            Some(&LspAny::Bool(true)),
            "displayPartsForJSDoc",
        )
    });

    t.finish();
}

/// Go `map[string]any{...}` literal.
fn obj(entries: &[(&str, LspAny)]) -> LspAny {
    LspAny::Object(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn items(entries: &[(&str, LspAny)]) -> IndexMap<String, LspAny> {
    match obj(entries) {
        LspAny::Object(m) => m,
        _ => unreachable!(),
    }
}

// Go: ls/lsutil/userpreferences_test.go:517 TestUserPreferencesLocale
#[test]
fn test_user_preferences_locale() {
    let prefs = parse_user_preferences(&items(&[
        (
            "typescript",
            obj(&[("locale", LspAny::String("de".into()))]),
        ),
        ("js/ts", obj(&[("locale", LspAny::String("fr".into()))])),
    ]));

    assert_eq!(prefs.locale, "fr");
}

// Go: ls/lsutil/userpreferences_test.go:532 TestUserPreferencesReportStyleChecksAsWarnings
#[test]
fn test_user_preferences_report_style_checks_as_warnings() {
    let mut t = Subtests::new("TestUserPreferencesReportStyleChecksAsWarnings");

    t.run("reportStyleChecksAsWarnings via config path", || {
        let prefs = parse_user_preferences(&items(&[(
            "js/ts",
            obj(&[("reportStyleChecksAsWarnings", LspAny::Bool(false))]),
        )]));
        assert_equal(
            prefs.report_style_checks_as_warnings,
            Tristate::False,
            "ReportStyleChecksAsWarnings",
        )
    });

    t.run("reportStyleChecksAsWarnings defaults to true", || {
        let prefs = new_default_user_preferences();
        assert_equal(
            prefs.report_style_checks_as_warnings,
            Tristate::True,
            "ReportStyleChecksAsWarnings",
        )
    });

    t.run("reportStyleChecksAsWarnings via unstable section", || {
        let prefs = parse_user_preferences(&items(&[(
            "js/ts",
            obj(&[(
                "unstable",
                obj(&[("reportStyleChecksAsWarnings", LspAny::Bool(false))]),
            )]),
        )]));
        assert_equal(
            prefs.report_style_checks_as_warnings,
            Tristate::False,
            "ReportStyleChecksAsWarnings",
        )
    });

    t.finish();
}

// Go: ls/lsutil/userpreferences_test.go:711 TestUserPreferencesParseATA
#[test]
fn test_user_preferences_parse_ata() {
    let mut t = Subtests::new("TestUserPreferencesParseATA");

    t.run(
        "ParseUserPreferences with unified ATA setting in js/ts section",
        || {
            let prefs = parse_user_preferences(&items(&[(
                "js/ts",
                obj(&[(
                    "tsserver",
                    obj(&[(
                        "automaticTypeAcquisition",
                        obj(&[("enabled", LspAny::Bool(false))]),
                    )]),
                )]),
            )]));
            if !prefs.is_ata_disabled() {
                return Err("assertion failed: prefs.IsATADisabled()".to_string());
            }
            assert_equal(
                prefs.automatic_type_acquisition_enabled,
                Tristate::False,
                "AutomaticTypeAcquisitionEnabled",
            )
        },
    );

    t.run("ParseUserPreferences with deprecated disableAutomaticTypeAcquisition in typescript section", || {
        let prefs = parse_user_preferences(&items(&[(
            "typescript",
            obj(&[("disableAutomaticTypeAcquisition", LspAny::Bool(true))]),
        )]));
        if !prefs.is_ata_disabled() {
            return Err("assertion failed: prefs.IsATADisabled()".to_string());
        }
        assert_equal(prefs.disable_automatic_type_acquisition, Tristate::True, "DisableAutomaticTypeAcquisition")
    });

    t.run(
        "unified setting takes precedence over deprecated setting",
        || {
            // Both settings set: unified (js/ts) should take precedence
            let prefs = parse_user_preferences(&items(&[
                (
                    "typescript",
                    obj(&[("disableAutomaticTypeAcquisition", LspAny::Bool(true))]),
                ),
                (
                    "js/ts",
                    obj(&[(
                        "tsserver",
                        obj(&[(
                            "automaticTypeAcquisition",
                            obj(&[("enabled", LspAny::Bool(true))]),
                        )]),
                    )]),
                ),
            ]));
            if prefs.is_ata_disabled() {
                return Err("assertion failed: !prefs.IsATADisabled()".to_string());
            }
            assert_equal(
                prefs.automatic_type_acquisition_enabled,
                Tristate::True,
                "AutomaticTypeAcquisitionEnabled",
            )
        },
    );

    t.run(
        "IsATADisabled returns false when neither setting is configured",
        || {
            let prefs = new_default_user_preferences();
            if prefs.is_ata_disabled() {
                return Err("assertion failed: !prefs.IsATADisabled()".to_string());
            }
            Ok(())
        },
    );

    t.finish();
}

// Go: ls/lsutil/userpreferences_test.go:619 TestParseUserPreferencesEditorFormatting
// PORT: Go writes the numbers as Go `int`; a JSON number is `LspAny::Number`.
#[test]
fn test_parse_user_preferences_editor_formatting() {
    let prefs = parse_user_preferences(&items(&[(
        "editor",
        obj(&[
            ("tabSize", LspAny::Number(2.0)),
            ("insertSpaces", LspAny::Bool(false)),
        ]),
    )]));

    let settings = &prefs.format_code_settings.editor_settings;
    assert_equal(settings.tab_size, 2, "FormatCodeSettings.TabSize").unwrap();
    assert_equal(settings.indent_size, 2, "FormatCodeSettings.IndentSize").unwrap();
    assert_equal(
        settings.convert_tabs_to_spaces,
        Tristate::False,
        "FormatCodeSettings.ConvertTabsToSpaces",
    )
    .unwrap();
}
