//! Port of internal/api/proto_test.go, and tests of the API copy of Go
//! `tspath.ToRootedPath` (`api::to_rooted_path`, ts#64159).

use super::Subtests;
use ts_goport::api::{
    DiagnosticPositionResponse, DiagnosticSourceLineResponse, DocumentIdentifier, EnsurePrograms,
    new_diagnostic_response, to_rooted_path,
};
use ts_goport::ast::{TextRange, new_diagnostic, source_file_get_position_map};
use ts_goport::diag;
use ts_goport::flags::ScriptKind;
use ts_goport::frontend::json::json_unmarshal;
use ts_goport::frontend::parser::{SourceFileParseOptions, parse_source_file};
use ts_goport::frontend::tspath;
use ts_goport::project;

// Go: api/proto_test.go:30 TestDocumentIdentifierUnmarshalJSON (ts#64159 cases)
#[test]
fn test_document_identifier_unmarshal_json() {
    struct Test {
        name: &'static str,
        input: &'static str,
        file_name: &'static str,
        uri: &'static str,
        err: &'static str,
    }
    let tests = [
        Test {
            name: "plain string",
            input: r#""foo.ts""#,
            file_name: "foo.ts",
            uri: "",
            err: "",
        },
        Test {
            name: "uri object",
            input: r#"{"uri":"file:///foo.ts"}"#,
            file_name: "",
            uri: "file:///foo.ts",
            err: "",
        },
        Test {
            name: "uri object with unknown fields",
            input: r#"{"uri":"file:///foo.ts","extra":true}"#,
            file_name: "",
            uri: "file:///foo.ts",
            err: "",
        },
        // ts#64159
        Test {
            name: "uri object with nested unknown field",
            input: r#"{"extra":{"nested":true},"uri":"file:///foo.ts"}"#,
            file_name: "",
            uri: "file:///foo.ts",
            err: "",
        },
        // ts#64159: was no error.
        Test {
            name: "empty object",
            input: "{}",
            file_name: "",
            uri: "",
            err: "object must contain uri",
        },
        // ts#64159
        Test {
            name: "empty file name",
            input: r#""""#,
            file_name: "",
            uri: "",
            err: "file name must not be empty",
        },
        Test {
            name: "empty uri",
            input: r#"{"uri":""}"#,
            file_name: "",
            uri: "",
            err: "uri must be a non-empty string",
        },
        Test {
            name: "non-string uri",
            input: r#"{"uri":42}"#,
            file_name: "",
            uri: "",
            err: "uri must be a non-empty string",
        },
        Test {
            name: "duplicate uri",
            input: r#"{"uri":"file:///foo.ts","uri":"file:///bar.ts"}"#,
            file_name: "",
            uri: "",
            err: r#"duplicate object member name "uri""#,
        },
        Test {
            name: "invalid type",
            input: "42",
            file_name: "",
            uri: "",
            err: "expected string or object, got number",
        },
    ];

    let mut t = Subtests::new("TestDocumentIdentifierUnmarshalJSON");
    for tt in &tests {
        t.run(tt.name, || {
            let mut d = DocumentIdentifier::default();
            let err = json_unmarshal(tt.input.as_bytes(), &mut d, &[]);
            if !tt.err.is_empty() {
                // Go: assert.ErrorContains(t, err, tt.err)
                match err {
                    Ok(()) => {
                        return Err(format!(
                            "expected an error containing {:?}, got nil",
                            tt.err
                        ));
                    }
                    Err(err) if !err.message.contains(tt.err) => {
                        return Err(format!(
                            "expected error {:?} to contain {:?}",
                            err.message, tt.err
                        ));
                    }
                    Err(_) => return Ok(()),
                }
            }
            if let Err(err) = err {
                return Err(format!("assertion failed: error is not nil: {err}"));
            }
            assert_eq!(d.file_name, tt.file_name);
            assert_eq!(d.uri.0, tt.uri);
            Ok(())
        });
    }
    t.finish();
}

// Go: api/proto_test.go:69 TestEnsureProgramsUnmarshalJSON (ts#64204, ts#64319)
#[test]
fn test_ensure_programs_unmarshal_json() {
    let mut all = EnsurePrograms::default();
    json_unmarshal(b"true", &mut all, &[]).unwrap_or_else(|err| panic!("unmarshal: {err}"));
    assert!(all.all);

    let mut projects = EnsurePrograms::default();
    json_unmarshal(
        br#"["/tsconfig.json","/dev/null/synthetic/1"]"#,
        &mut projects,
        &[],
    )
    .unwrap_or_else(|err| panic!("unmarshal: {err}"));
    assert_eq!(
        projects.projects,
        vec![
            project::ConfiguredProjectID(tspath::Path("/tsconfig.json".to_string())).as_id(),
            project::new_synthetic_project_id(1).as_id(),
        ]
    );

    let mut invalid = EnsurePrograms::default();
    match json_unmarshal(b"false", &mut invalid, &[]) {
        Ok(()) => panic!("expected an error containing \"must be true or an array\", got nil"),
        Err(err) => assert!(
            err.message.contains("must be true or an array"),
            "expected error {:?} to contain \"must be true or an array\"",
            err.message
        ),
    }
}

// Go: api/proto_test.go:87 TestNewDiagnosticResponseIncludesFormattingContext (ts#63935; was
// TestNewDiagnosticResponseUsesUTF16Offsets)
#[test]
fn test_new_diagnostic_response_includes_formatting_context() {
    let text = "const 💩 = 1;";
    let file = parse_source_file(
        &SourceFileParseOptions {
            file_name: "/unicode.ts".to_string(),
            ..Default::default()
        },
        text,
        ScriptKind::TS,
    )
    .root;
    // Go `strings.Index` is -1 when the text has no "=".
    let pos = text.find('=').map_or(-1, |i| i as i32);
    assert!(pos > 0);
    let end = pos + "=".len() as i32;

    let diag = new_diagnostic(
        file,
        TextRange::new(pos, end),
        diag::Expression_expected,
        Vec::new(),
    );
    let resp = new_diagnostic_response(&diag);

    assert_eq!(resp.pos, 9);
    assert_eq!(resp.end, 10);
    assert_eq!(
        resp.start_position,
        Some(DiagnosticPositionResponse {
            line: 0,
            character: 9
        })
    );
    assert_eq!(
        resp.end_position,
        Some(DiagnosticPositionResponse {
            line: 0,
            character: 10
        })
    );
    assert_eq!(
        resp.source_lines,
        vec![DiagnosticSourceLineResponse {
            line: 0,
            text: text.to_string()
        }]
    );
    assert_eq!(
        resp.pos,
        source_file_get_position_map(file).utf8_to_utf16(pos)
    );
    assert_eq!(
        resp.end,
        source_file_get_position_map(file).utf8_to_utf16(end)
    );
}

// Go: api/proto_test.go:108 TestNewDiagnosticResponseTruncatesLongFormattingContext (ts#63935)
#[test]
fn test_new_diagnostic_response_truncates_long_formatting_context() {
    let text = "one\ntwo\nthree\nfour\nfive\nsix\nseven";
    let file = parse_source_file(
        &SourceFileParseOptions {
            file_name: "/multiline.ts".to_string(),
            ..Default::default()
        },
        text,
        ScriptKind::TS,
    )
    .root;
    let diag = new_diagnostic(
        file,
        TextRange::new(0, text.len() as i32),
        diag::Expression_expected,
        Vec::new(),
    );
    let resp = new_diagnostic_response(&diag);

    assert_eq!(
        resp.start_position,
        Some(DiagnosticPositionResponse {
            line: 0,
            character: 0
        })
    );
    assert_eq!(
        resp.end_position,
        Some(DiagnosticPositionResponse {
            line: 6,
            character: 5
        })
    );
    let line = |line: i32, text: &str| DiagnosticSourceLineResponse {
        line,
        text: text.to_string(),
    };
    assert_eq!(
        resp.source_lines,
        vec![
            line(0, "one\n"),
            line(1, "two\n"),
            line(5, "six\n"),
            line(6, "seven"),
        ]
    );
}

/// The message of the Go panic (`go_panic`) that `f` raises.
fn go_panic_text<R>(f: impl FnOnce() -> R) -> String {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .err()
        .expect("no panic");
    match payload.downcast::<ts_goport::core::GoPanic>() {
        Ok(panic) => panic.message,
        Err(_) => panic!("not a Go panic"),
    }
}

// Go: tspath/typed_paths_test.go:10 TestToRootedFilePath and :275
// TestToRootedFilePathRequiresRoot (ts#64159): the `ToRootedPath` asserts,
// on the API copy (`api::to_rooted_path`, session_p2.rs).
// PORT: Go `ToRootedFilePath` and `ToRootedDirectoryPath` are
// `ToRootedPath` with a typed result, so one call checks the three.
#[test]
fn to_rooted_path_matches_go_to_rooted_path() {
    assert_eq!(
        to_rooted_path("./src/../src/a.ts", "/project"),
        "/project/src/a.ts"
    );
    assert_eq!(to_rooted_path("/project/src/", "/ignored"), "/project/src");
    assert_eq!(to_rooted_path("/", "/ignored"), "/");
    assert_eq!(
        to_rooted_path("file:///project/src/a.ts", "/ignored"),
        "file:///project/src/a.ts"
    );
    assert_eq!(
        to_rooted_path("^/untitled/ts-nul-authority/Untitled-1", "/ignored"),
        "^/untitled/ts-nul-authority/Untitled-1"
    );
    for (input, expected) in [
        ("c:", "c:/"),
        ("//server", "//server/"),
        ("file://server", "file://server/"),
        (
            "^/~ts-uri~/custom/ts-nul-authority",
            "^/~ts-uri~/custom/ts-nul-authority/",
        ),
        (
            "^/~ts-uri~/custom/authority?query",
            "^/~ts-uri~/custom/authority?query/",
        ),
    ] {
        assert_eq!(to_rooted_path(input, "/ignored"), expected, "{input}");
    }
    assert_eq!(to_rooted_path("/a://b?x/../y", "/ignored"), "/a:/y");
    for input in [
        "http://server?query#fragment",
        "http://server?x/../y",
        "file:///c:?query/path",
    ] {
        assert_eq!(
            go_panic_text(|| to_rooted_path(input, "/ignored")),
            "path must not contain a URL query or fragment",
            "{input}"
        );
    }
    assert_eq!(
        go_panic_text(|| to_rooted_path("file.ts?query/..", "http://server/base")),
        "relative URL path must not contain a query or fragment"
    );
    assert_eq!(
        go_panic_text(|| to_rooted_path("", "/project")),
        "path must not be empty"
    );
    assert_eq!(
        go_panic_text(|| to_rooted_path("src/a.ts", "")),
        "path must be rooted"
    );
}

// Go: api/proto.go:353 DocumentIdentifier.ToFileName (ts#64159). Port-only
// test (api skeptic, bump D wave 2b): a file name is rooted with Go
// `tspath.ToRootedFilePath`, so the zero identifier (a request without the
// field) and a URL name with a query or fragment panic as in Go N'. A URI
// gives its file name.
#[test]
fn document_identifier_to_file_name_roots_like_go() {
    let name = |file_name: &str| DocumentIdentifier {
        file_name: file_name.to_string(),
        ..Default::default()
    };
    assert_eq!(name("src/../a.ts").to_file_name("/p"), "/p/a.ts");
    assert_eq!(
        DocumentIdentifier {
            uri: ts_goport::lsp::lsproto::DocumentUri("file:///p/a.ts".to_string()),
            ..Default::default()
        }
        .to_file_name("/q"),
        "/p/a.ts"
    );
    assert_eq!(
        go_panic_text(|| DocumentIdentifier::default().to_file_name("/p")),
        "path must not be empty"
    );
    for file_name in ["file:///a.ts?x", "http://h/a.ts#f"] {
        assert_eq!(
            go_panic_text(|| name(file_name).to_file_name("/p")),
            "path must not contain a URL query or fragment",
            "{file_name}"
        );
    }
}
