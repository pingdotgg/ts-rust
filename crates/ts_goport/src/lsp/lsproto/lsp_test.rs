//! Port of internal/lsp/lsproto/lsp_test.go.
//!
//! PORT: Go `t.Parallel()` is dropped (cargo runs tests in parallel).

use crate::lsp::lsproto::prelude::*;

// Go: lsp_test.go:11 TestUnmarshalCompletionItem
#[test]
fn test_unmarshal_completion_item() {
    const MESSAGE: &str = r#"{
    "label": "pageXOffset",
    "insertTextFormat": 1,
    "textEdit": {
        "newText": "pageXOffset",
        "insert": {
            "start": {
                "line": 4,
                "character": 0
            },
            "end": {
                "line": 4,
                "character": 4
            }
        },
        "replace": {
            "start": {
                "line": 4,
                "character": 0
            },
            "end": {
                "line": 4,
                "character": 4
            }
        }
    },
    "kind": 6,
    "sortText": "15",
    "commitCharacters": [
        ".",
        ",",
        ";"
    ]
}"#;

    let mut result = CompletionItem::default();
    let err = json_unmarshal(MESSAGE.as_bytes(), &mut result, &[]);
    if let Err(err) = &err {
        panic!("expected no error, got {err}");
    }

    assert_eq!(
        result,
        CompletionItem {
            label: "pageXOffset".to_string(),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            text_edit: Some(TextEditOrInsertReplaceEdit {
                insert_replace_edit: Some(InsertReplaceEdit {
                    new_text: "pageXOffset".to_string(),
                    insert: Range {
                        start: Position {
                            line: 4,
                            character: 0,
                        },
                        end: Position {
                            line: 4,
                            character: 4,
                        },
                    },
                    replace: Range {
                        start: Position {
                            line: 4,
                            character: 0,
                        },
                        end: Position {
                            line: 4,
                            character: 4,
                        },
                    },
                }),
                ..Default::default()
            }),
            kind: Some(CompletionItemKind::VARIABLE),
            sort_text: Some("15".to_string()),
            commit_characters: Some(vec![".".to_string(), ",".to_string(), ";".to_string()]),
            ..Default::default()
        }
    );
}

// Go: lsp_test.go:87 TestTryDynamicFileNameToDocumentUri (ts#64159)
#[test]
fn test_try_dynamic_file_name_to_document_uri() {
    let uri = DocumentUri("untitled://wsl+ubuntu/home/user/file.ts?version=1".to_string());
    let path = uri.file_name();
    let round_trip = try_dynamic_file_name_to_document_uri(&path);
    assert_eq!(round_trip, Some(uri));

    for malformed in [
        "^/invalid",
        "^/~ts-uri~//authority/path",
        "^/~ts-uri~/scheme/~ts-uri-escape~zz~/path",
        "^/~ts-uri~/scheme/authority/~ts-uri-escape~zz~",
        "^/~ts-uri~/scheme/authority/~ts-uri-no-path~zz~",
    ] {
        assert!(
            try_dynamic_file_name_to_document_uri(malformed).is_none(),
            "expected {malformed:?} to be rejected"
        );
    }
}
