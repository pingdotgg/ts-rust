//! Port of internal/lsp/lsproto/lsp_json_test.go.
//!
//! PORT: Go `t.Parallel()` is dropped (cargo runs tests in parallel). A Go
//! table test is one `#[test]` with a loop that names the case in every
//! assert message. A Go `t.Run` subtest with its own body is one `#[test]`
//! named `<test>_<subtest>`. Go `json.Unmarshal` / `json.Marshal` are
//! `json_unmarshal` / `json_marshal` over the generated arshalers.

use crate::lsp::lsproto::prelude::*;

use std::any::Any;

// gotest.tools assert.NilError
#[track_caller]
fn assert_nil_error<T>(name: &str, err: &Result<T, JsonError>) {
    if let Err(err) = err {
        panic!("{name}: expected no error, got {err}");
    }
}

// gotest.tools assert.ErrorContains
#[track_caller]
fn assert_error_contains(name: &str, err: &Result<(), JsonError>, substring: &str) {
    match err {
        Ok(()) => panic!("{name}: expected an error containing {substring:?}, got nil"),
        Err(err) => {
            let text = err.to_string();
            assert!(
                text.contains(substring),
                "{name}: expected error to contain {substring:?}, got {text:?}"
            );
        }
    }
}

// PORT: Go `target any` holds a pointer that `check` type-asserts. The
// trait object keeps both the unmarshaler and the type assertion.
trait Target: UnmarshalerFrom + Any {
    fn as_any(&self) -> &dyn Any;
}

impl<T: UnmarshalerFrom + Any> Target for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Go: lsp_json_test.go:12 TestUnmarshalRejectsNullForOptionalNonNullableFields
#[test]
fn test_unmarshal_rejects_null_for_optional_non_nullable_fields() {
    struct Test {
        name: &'static str,
        input: &'static str,
        target: Box<dyn UnmarshalerFrom>,
        err_text: &'static str,
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "InlayHint kind null",
            input: r#"{"position": {"line": 0, "character": 0}, "label": "foo", "kind": null}"#,
            target: Box::new(InlayHint::default()),
            err_text: r#"null value is not allowed for field "kind""#,
        },
        Test {
            name: "InlayHint textEdits null",
            input: r#"{"position": {"line": 0, "character": 0}, "label": "foo", "textEdits": null}"#,
            target: Box::new(InlayHint::default()),
            err_text: r#"null value is not allowed for field "textEdits""#,
        },
        Test {
            name: "InlayHint paddingLeft null",
            input: r#"{"position": {"line": 0, "character": 0}, "label": "foo", "paddingLeft": null}"#,
            target: Box::new(InlayHint::default()),
            err_text: r#"null value is not allowed for field "paddingLeft""#,
        },
        Test {
            name: "FoldingRange kind null",
            input: r#"{"startLine": 0, "endLine": 10, "kind": null}"#,
            target: Box::new(FoldingRange::default()),
            err_text: r#"null value is not allowed for field "kind""#,
        },
        Test {
            name: "FoldingRange startCharacter null",
            input: r#"{"startLine": 0, "endLine": 10, "startCharacter": null}"#,
            target: Box::new(FoldingRange::default()),
            err_text: r#"null value is not allowed for field "startCharacter""#,
        },
        Test {
            name: "CompletionItem insertTextFormat null",
            input: r#"{"label": "test", "insertTextFormat": null}"#,
            target: Box::new(CompletionItem::default()),
            err_text: r#"null value is not allowed for field "insertTextFormat""#,
        },
        Test {
            name: "Hover range null",
            input: r#"{"contents": {"kind": "plaintext", "value": "hi"}, "range": null}"#,
            target: Box::new(Hover::default()),
            err_text: r#"null value is not allowed for field "range""#,
        },
        Test {
            name: "WorkDoneProgressOptions workDoneProgress null",
            input: r#"{"workDoneProgress": null}"#,
            target: Box::new(WorkDoneProgressOptions::default()),
            err_text: r#"null value is not allowed for field "workDoneProgress""#,
        },
        Test {
            name: "CallHierarchyIncomingCallsParams item null",
            input: r#"{"item": null}"#,
            target: Box::new(CallHierarchyIncomingCallsParams::default()),
            err_text: r#"null value is not allowed for field "item""#,
        },
        Test {
            name: "CallHierarchyIncomingCall from null",
            input: r#"{"from": null, "fromRanges": []}"#,
            target: Box::new(CallHierarchyIncomingCall::default()),
            err_text: r#"null value is not allowed for field "from""#,
        },
        Test {
            name: "InitializeParams capabilities null",
            input: r#"{"processId": null, "rootUri": null, "capabilities": null}"#,
            target: Box::new(InitializeParams::default()),
            err_text: r#"null value is not allowed for field "capabilities""#,
        },
        Test {
            name: "InitializeResult capabilities null",
            input: r#"{"capabilities": null}"#,
            target: Box::new(InitializeResult::default()),
            err_text: r#"null value is not allowed for field "capabilities""#,
        },
        Test {
            name: "SemanticTokens data null (required slice)",
            input: r#"{"data": null}"#,
            target: Box::new(SemanticTokens::default()),
            err_text: r#"null value is not allowed for field "data""#,
        },
        Test {
            name: "TextDocumentEdit edits null (required slice)",
            input: r#"{"textDocument": {"uri": "file:///a.ts", "version": 1}, "edits": null}"#,
            target: Box::new(TextDocumentEdit::default()),
            err_text: r#"null value is not allowed for field "edits""#,
        },
    ];

    for mut tt in tests {
        let err = json_unmarshal(tt.input.as_bytes(), &mut *tt.target, &[]);
        assert_error_contains(tt.name, &err, tt.err_text);
    }
}

// Go: lsp_json_test.go:116 TestUnmarshalAcceptsNullForNullableFields
#[test]
fn test_unmarshal_accepts_null_for_nullable_fields() {
    struct Test {
        name: &'static str,
        input: &'static str,
        target: Box<dyn UnmarshalerFrom>,
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "InitializeParams rootUri null",
            input: r#"{"processId": null, "rootUri": null, "capabilities": {}}"#,
            target: Box::new(InitializeParams::default()),
        },
        Test {
            name: "InitializeParams workspaceFolders null",
            input: r#"{"processId": null, "rootUri": null, "capabilities": {}, "workspaceFolders": null}"#,
            target: Box::new(InitializeParams::default()),
        },
        Test {
            name: "InitializeParams processId null",
            input: r#"{"processId": null, "rootUri": null, "capabilities": {}}"#,
            target: Box::new(InitializeParams::default()),
        },
        Test {
            name: "InitializationOptions userPreferences null",
            input: r#"{"userPreferences": null}"#,
            target: Box::new(InitializationOptions::default()),
        },
        Test {
            name: "InitializeParams initializationOptions null",
            input: r#"{"processId": null, "rootUri": null, "capabilities": {}, "initializationOptions": null}"#,
            target: Box::new(InitializeParams::default()),
        },
    ];

    for mut tt in tests {
        let err = json_unmarshal(tt.input.as_bytes(), &mut *tt.target, &[]);
        assert_nil_error(tt.name, &err);
    }
}

// Go: lsp_json_test.go:160 TestUnmarshalAcceptsOmittedOptionalFields
#[test]
fn test_unmarshal_accepts_omitted_optional_fields() {
    struct Test {
        name: &'static str,
        input: &'static str,
        target: Box<dyn Target>,
        check: fn(name: &str, target: &dyn Any),
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "InlayHint with only required fields",
            input: r#"{"position": {"line": 1, "character": 5}, "label": "test"}"#,
            target: Box::new(InlayHint::default()),
            check: |name, target| {
                let hint = target
                    .downcast_ref::<InlayHint>()
                    .expect("interface conversion: target is not *InlayHint");
                assert!(hint.kind.is_none(), "{name}");
                assert!(hint.text_edits.is_none(), "{name}");
                assert!(hint.tooltip.is_none(), "{name}");
                assert!(hint.padding_left.is_none(), "{name}");
                assert!(hint.padding_right.is_none(), "{name}");
                assert!(hint.data.is_none(), "{name}");
                assert_eq!(hint.position.line, 1u32, "{name}");
                assert_eq!(hint.position.character, 5u32, "{name}");
            },
        },
        Test {
            name: "FoldingRange with only required fields",
            input: r#"{"startLine": 5, "endLine": 10}"#,
            target: Box::new(FoldingRange::default()),
            check: |name, target| {
                let fr = target
                    .downcast_ref::<FoldingRange>()
                    .expect("interface conversion: target is not *FoldingRange");
                assert!(fr.kind.is_none(), "{name}");
                assert!(fr.start_character.is_none(), "{name}");
                assert!(fr.end_character.is_none(), "{name}");
                assert!(fr.collapsed_text.is_none(), "{name}");
                assert_eq!(fr.start_line, 5u32, "{name}");
                assert_eq!(fr.end_line, 10u32, "{name}");
            },
        },
    ];

    for mut tt in tests {
        let err = json_unmarshal(tt.input.as_bytes(), &mut *tt.target, &[]);
        assert_nil_error(tt.name, &err);
        (tt.check)(tt.name, (*tt.target).as_any());
    }
}

// Go: lsp_json_test.go:213 TestUnmarshalRejectsIncompleteObjects
#[test]
fn test_unmarshal_rejects_incomplete_objects() {
    struct Test {
        name: &'static str,
        input: &'static str,
        target: Box<dyn UnmarshalerFrom>,
        err_text: &'static str,
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "InlayHint missing position",
            input: r#"{"label": "test"}"#,
            target: Box::new(InlayHint::default()),
            err_text: "missing required properties: position",
        },
        Test {
            name: "InlayHint missing label",
            input: r#"{"position": {"line": 0, "character": 0}}"#,
            target: Box::new(InlayHint::default()),
            err_text: "missing required properties: label",
        },
        Test {
            name: "Location missing uri",
            input: r#"{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}}"#,
            target: Box::new(Location::default()),
            err_text: "missing required properties: uri",
        },
        Test {
            name: "Location empty object",
            input: r#"{}"#,
            target: Box::new(Location::default()),
            err_text: "missing required properties: uri, range",
        },
    ];

    for mut tt in tests {
        let err = json_unmarshal(tt.input.as_bytes(), &mut *tt.target, &[]);
        assert_error_contains(tt.name, &err, tt.err_text);
    }
}

// PORT: Go `value any` holds one of four pointer types, and the test
// switches on the dynamic type. This closed enum stands in for `any`. Its
// `MarshalerTo` forwards to the held value, as Go dispatches on the dynamic
// type.
#[derive(Debug)]
enum RoundTripValue {
    InlayHint(InlayHint),
    FoldingRange(FoldingRange),
    Location(Location),
    InitializeParams(InitializeParams),
}

impl MarshalerTo for RoundTripValue {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        match self {
            RoundTripValue::InlayHint(v) => v.marshal_json_to(enc),
            RoundTripValue::FoldingRange(v) => v.marshal_json_to(enc),
            RoundTripValue::Location(v) => v.marshal_json_to(enc),
            RoundTripValue::InitializeParams(v) => v.marshal_json_to(enc),
        }
    }
}

// Go: lsp_json_test.go:257 TestMarshalUnmarshalRoundTrip
#[test]
fn test_marshal_unmarshal_round_trip() {
    struct Test {
        name: &'static str,
        value: RoundTripValue,
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "InlayHint with kind",
            value: RoundTripValue::InlayHint(InlayHint {
                position: Position {
                    line: 1,
                    character: 5,
                },
                label: StringOrInlayHintLabelParts {
                    string: Some("param".to_string()),
                    ..Default::default()
                },
                kind: Some(InlayHintKind::PARAMETER),
                ..Default::default()
            }),
        },
        Test {
            name: "InlayHint minimal",
            value: RoundTripValue::InlayHint(InlayHint {
                position: Position {
                    line: 0,
                    character: 0,
                },
                label: StringOrInlayHintLabelParts {
                    string: Some("x".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        Test {
            name: "FoldingRange with all fields",
            value: RoundTripValue::FoldingRange(FoldingRange {
                start_line: 1,
                start_character: Some(0u32),
                end_line: 10,
                end_character: Some(5u32),
                kind: Some(FoldingRangeKind::REGION),
                collapsed_text: Some("...".to_string()),
            }),
        },
        Test {
            name: "Location",
            value: RoundTripValue::Location(Location {
                uri: DocumentUri("file:///test.ts".to_string()),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 2,
                    },
                    end: Position {
                        line: 3,
                        character: 4,
                    },
                },
            }),
        },
        Test {
            name: "InitializeParams with null processId",
            value: RoundTripValue::InitializeParams(InitializeParams {
                process_id: IntegerOrNull::default(),
                root_uri: DocumentUriOrNull {
                    document_uri: Some(DocumentUri("file:///workspace".to_string())),
                },
                capabilities: Some(ClientCapabilities::default()),
                ..Default::default()
            }),
        },
    ];

    for tt in tests {
        let data = json_marshal(&tt.value, &[]);
        assert_nil_error(tt.name, &data);
        let data = data.unwrap();

        // Unmarshal into a new value of the same type
        match &tt.value {
            RoundTripValue::InlayHint(v) => {
                let mut result = InlayHint::default();
                let err = json_unmarshal(data.as_bytes(), &mut result, &[]);
                assert_nil_error(tt.name, &err);
                assert_eq!(*v, result, "{}", tt.name);
            }
            RoundTripValue::FoldingRange(v) => {
                let mut result = FoldingRange::default();
                let err = json_unmarshal(data.as_bytes(), &mut result, &[]);
                assert_nil_error(tt.name, &err);
                assert_eq!(*v, result, "{}", tt.name);
            }
            RoundTripValue::Location(v) => {
                let mut result = Location::default();
                let err = json_unmarshal(data.as_bytes(), &mut result, &[]);
                assert_nil_error(tt.name, &err);
                assert_eq!(*v, result, "{}", tt.name);
            }
            RoundTripValue::InitializeParams(v) => {
                let mut result = InitializeParams::default();
                let err = json_unmarshal(data.as_bytes(), &mut result, &[]);
                assert_nil_error(tt.name, &err);
                assert_eq!(*v, result, "{}", tt.name);
            }
        }
        // PORT: Go `default: t.Fatalf("unhandled type %T", tt.value)` cannot
        // happen; the enum is closed.
    }
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "IntegerOrString with integer"
#[test]
fn test_unmarshal_union_types_integer_or_string_with_integer() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(b"42", &mut v, &[]);
    assert_nil_error("IntegerOrString with integer", &err);
    assert!(v.integer.is_some());
    assert_eq!(v.integer.unwrap(), 42i32);
    assert!(v.string.is_none());
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "IntegerOrString with string"
#[test]
fn test_unmarshal_union_types_integer_or_string_with_string() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(br#""hello""#, &mut v, &[]);
    assert_nil_error("IntegerOrString with string", &err);
    assert!(v.string.is_some());
    assert_eq!(v.string.as_deref().unwrap(), "hello");
    assert!(v.integer.is_none());
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "IntegerOrNull with integer"
#[test]
fn test_unmarshal_union_types_integer_or_null_with_integer() {
    let mut v = IntegerOrNull::default();
    let err = json_unmarshal(b"42", &mut v, &[]);
    assert_nil_error("IntegerOrNull with integer", &err);
    assert!(v.integer.is_some());
    assert_eq!(v.integer.unwrap(), 42i32);
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "IntegerOrNull with null"
#[test]
fn test_unmarshal_union_types_integer_or_null_with_null() {
    let mut v = IntegerOrNull::default();
    let err = json_unmarshal(b"null", &mut v, &[]);
    assert_nil_error("IntegerOrNull with null", &err);
    assert!(v.integer.is_none());
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "DocumentUriOrNull with string"
#[test]
fn test_unmarshal_union_types_document_uri_or_null_with_string() {
    let mut v = DocumentUriOrNull::default();
    let err = json_unmarshal(br#""file:///test.ts""#, &mut v, &[]);
    assert_nil_error("DocumentUriOrNull with string", &err);
    assert!(v.document_uri.is_some());
    assert_eq!(
        *v.document_uri.as_ref().unwrap(),
        DocumentUri("file:///test.ts".to_string())
    );
}

// Go: lsp_json_test.go:345 TestUnmarshalUnionTypes, "DocumentUriOrNull with null"
#[test]
fn test_unmarshal_union_types_document_uri_or_null_with_null() {
    let mut v = DocumentUriOrNull::default();
    let err = json_unmarshal(b"null", &mut v, &[]);
    assert_nil_error("DocumentUriOrNull with null", &err);
    assert!(v.document_uri.is_none());
}

// Go: lsp_json_test.go:403 TestMarshalUnionTypes, "IntegerOrNull with value"
#[test]
fn test_marshal_union_types_integer_or_null_with_value() {
    let v = IntegerOrNull {
        integer: Some(42i32),
    };
    let data = json_marshal(&v, &[]);
    assert_nil_error("IntegerOrNull with value", &data);
    assert_eq!(data.unwrap(), "42");
}

// Go: lsp_json_test.go:403 TestMarshalUnionTypes, "IntegerOrNull with null"
#[test]
fn test_marshal_union_types_integer_or_null_with_null() {
    let v = IntegerOrNull::default();
    let data = json_marshal(&v, &[]);
    assert_nil_error("IntegerOrNull with null", &data);
    assert_eq!(data.unwrap(), "null");
}

// Go: lsp_json_test.go:403 TestMarshalUnionTypes, "IntegerOrString with integer"
#[test]
fn test_marshal_union_types_integer_or_string_with_integer() {
    let v = IntegerOrString {
        integer: Some(7i32),
        ..Default::default()
    };
    let data = json_marshal(&v, &[]);
    assert_nil_error("IntegerOrString with integer", &data);
    assert_eq!(data.unwrap(), "7");
}

// Go: lsp_json_test.go:403 TestMarshalUnionTypes, "IntegerOrString with string"
#[test]
fn test_marshal_union_types_integer_or_string_with_string() {
    let v = IntegerOrString {
        string: Some("tok".to_string()),
        ..Default::default()
    };
    let data = json_marshal(&v, &[]);
    assert_nil_error("IntegerOrString with string", &data);
    assert_eq!(data.unwrap(), r#""tok""#);
}

// Go: lsp_json_test.go:439 TestUnmarshalIgnoresUnknownFields, "Location with extra fields"
#[test]
fn test_unmarshal_ignores_unknown_fields_location_with_extra_fields() {
    let mut loc = Location::default();
    let err = json_unmarshal(
        br#"{
			"uri": "file:///test.ts",
			"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
			"someUnknownField": 42,
			"anotherUnknown": {"nested": true}
		}"#,
        &mut loc,
        &[],
    );
    assert_nil_error("Location with extra fields", &err);
    assert_eq!(loc.uri, DocumentUri("file:///test.ts".to_string()));
}

// Go: lsp_json_test.go:439 TestUnmarshalIgnoresUnknownFields, "InlayHint with extra fields"
#[test]
fn test_unmarshal_ignores_unknown_fields_inlay_hint_with_extra_fields() {
    let mut hint = InlayHint::default();
    let err = json_unmarshal(
        br#"{
			"position": {"line": 0, "character": 0},
			"label": "x",
			"futureField": [1, 2, 3]
		}"#,
        &mut hint,
        &[],
    );
    assert_nil_error("InlayHint with extra fields", &err);
}

// Go: lsp_json_test.go:467 TestUnmarshalRejectsWrongTypes
#[test]
fn test_unmarshal_rejects_wrong_types() {
    struct Test {
        name: &'static str,
        input: &'static str,
        target: Box<dyn UnmarshalerFrom>,
    }

    let tests: Vec<Test> = vec![
        Test {
            name: "Location receives array",
            input: r#"[]"#,
            target: Box::new(Location::default()),
        },
        Test {
            name: "Location receives string",
            input: r#""not an object""#,
            target: Box::new(Location::default()),
        },
        Test {
            name: "Location receives number",
            input: r#"42"#,
            target: Box::new(Location::default()),
        },
        Test {
            name: "Location receives null",
            input: r#"null"#,
            target: Box::new(Location::default()),
        },
        Test {
            name: "FoldingRange receives boolean",
            input: r#"true"#,
            target: Box::new(FoldingRange::default()),
        },
    ];

    for mut tt in tests {
        let err = json_unmarshal(tt.input.as_bytes(), &mut *tt.target, &[]);
        assert!(
            err.is_err(),
            "{}: expected error for input {}",
            tt.name,
            tt.input
        );
    }
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "IntegerOrString rejects boolean"
#[test]
fn test_unmarshal_union_type_wrong_kind_integer_or_string_rejects_boolean() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(b"true", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "IntegerOrString rejects null"
#[test]
fn test_unmarshal_union_type_wrong_kind_integer_or_string_rejects_null() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(b"null", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "IntegerOrString rejects object"
#[test]
fn test_unmarshal_union_type_wrong_kind_integer_or_string_rejects_object() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "IntegerOrString rejects array"
#[test]
fn test_unmarshal_union_type_wrong_kind_integer_or_string_rejects_array() {
    let mut v = IntegerOrString::default();
    let err = json_unmarshal(b"[]", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "StringOrInlayHintLabelParts rejects number"
#[test]
fn test_unmarshal_union_type_wrong_kind_string_or_inlay_hint_label_parts_rejects_number() {
    let mut v = StringOrInlayHintLabelParts::default();
    let err = json_unmarshal(b"42", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:511 TestUnmarshalUnionTypeWrongKind, "StringOrInlayHintLabelParts rejects boolean"
#[test]
fn test_unmarshal_union_type_wrong_kind_string_or_inlay_hint_label_parts_rejects_boolean() {
    let mut v = StringOrInlayHintLabelParts::default();
    let err = json_unmarshal(b"true", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:557 TestUnmarshalBooleanUnionTypes, "BooleanOrHoverOptions with true"
#[test]
fn test_unmarshal_boolean_union_types_boolean_or_hover_options_with_true() {
    let mut v = BooleanOrHoverOptions::default();
    let err = json_unmarshal(b"true", &mut v, &[]);
    assert_nil_error("BooleanOrHoverOptions with true", &err);
    assert!(v.boolean.is_some());
    assert!(v.boolean.unwrap());
    assert!(v.hover_options.is_none());
}

// Go: lsp_json_test.go:557 TestUnmarshalBooleanUnionTypes, "BooleanOrHoverOptions with false"
#[test]
fn test_unmarshal_boolean_union_types_boolean_or_hover_options_with_false() {
    let mut v = BooleanOrHoverOptions::default();
    let err = json_unmarshal(b"false", &mut v, &[]);
    assert_nil_error("BooleanOrHoverOptions with false", &err);
    assert!(v.boolean.is_some());
    assert!(!v.boolean.unwrap());
    assert!(v.hover_options.is_none());
}

// Go: lsp_json_test.go:557 TestUnmarshalBooleanUnionTypes, "BooleanOrHoverOptions with object"
#[test]
fn test_unmarshal_boolean_union_types_boolean_or_hover_options_with_object() {
    let mut v = BooleanOrHoverOptions::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert_nil_error("BooleanOrHoverOptions with object", &err);
    assert!(v.boolean.is_none());
    assert!(v.hover_options.is_some());
}

// Go: lsp_json_test.go:557 TestUnmarshalBooleanUnionTypes, "BooleanOrHoverOptions rejects string"
#[test]
fn test_unmarshal_boolean_union_types_boolean_or_hover_options_rejects_string() {
    let mut v = BooleanOrHoverOptions::default();
    let err = json_unmarshal(br#""nope""#, &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "WorkDoneProgressBegin"
#[test]
fn test_unmarshal_discriminator_union_work_done_progress_begin() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"kind": "begin", "title": "Indexing"}"#, &mut v, &[]);
    assert_nil_error("WorkDoneProgressBegin", &err);
    assert!(v.begin.is_some());
    assert!(v.report.is_none());
    assert!(v.end.is_none());
    assert_eq!(v.begin.as_ref().unwrap().title, "Indexing");
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "WorkDoneProgressReport"
#[test]
fn test_unmarshal_discriminator_union_work_done_progress_report() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"kind": "report", "message": "50%"}"#, &mut v, &[]);
    assert_nil_error("WorkDoneProgressReport", &err);
    assert!(v.begin.is_none());
    assert!(v.report.is_some());
    assert!(v.end.is_none());
    assert!(v.report.as_ref().unwrap().message.is_some());
    assert_eq!(
        v.report.as_ref().unwrap().message.as_deref().unwrap(),
        "50%"
    );
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "WorkDoneProgressEnd"
#[test]
fn test_unmarshal_discriminator_union_work_done_progress_end() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"kind": "end"}"#, &mut v, &[]);
    assert_nil_error("WorkDoneProgressEnd", &err);
    assert!(v.begin.is_none());
    assert!(v.report.is_none());
    assert!(v.end.is_some());
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "invalid discriminator"
#[test]
fn test_unmarshal_discriminator_union_invalid_discriminator() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"kind": "invalid"}"#, &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "discriminator after variant fields"
#[test]
fn test_unmarshal_discriminator_union_discriminator_after_variant_fields() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(
        br#"{"title": "Indexing", "percentage": 25, "kind": "begin"}"#,
        &mut v,
        &[],
    );
    assert_nil_error("discriminator after variant fields", &err);
    let begin = v.begin.as_ref().expect("begin");
    assert_eq!(begin.title, "Indexing");
    assert_eq!(begin.percentage, Some(25));
}

// PORT: Go lsp_json_test.go:649 TestUnmarshalDiscriminatorUnion, "optional
// discriminator is preserved", decodes a test-only struct by reflection
// (`optionalDiscriminatorArm`). Rust arms decode with generated code only, so
// it is not ported; the generated arms keep the discriminator (the tests
// above read the arm that it picks).

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "non-string discriminator"
#[test]
fn test_unmarshal_discriminator_union_non_string_discriminator() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"kind": null}"#, &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:602 TestUnmarshalDiscriminatorUnion, "missing discriminator"
#[test]
fn test_unmarshal_discriminator_union_missing_discriminator() {
    let mut v = WorkDoneProgressBeginOrReportOrEnd::default();
    let err = json_unmarshal(br#"{"message": "missing kind"}"#, &mut v, &[]);
    assert_error_contains(
        "missing discriminator",
        &err,
        r#"missing discriminator "kind""#,
    );
}

// Go: lsp_json_test.go:683 TestUnmarshalPresenceDiscriminatorUnion, "TextEdit via range field"
#[test]
fn test_unmarshal_presence_discriminator_union_text_edit_via_range_field() {
    let mut v = TextEditOrInsertReplaceEdit::default();
    let err = json_unmarshal(
        br#"{
			"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
			"newText": "x"
		}"#,
        &mut v,
        &[],
    );
    assert_nil_error("TextEdit via range field", &err);
    assert!(v.text_edit.is_some());
    assert!(v.insert_replace_edit.is_none());
    assert_eq!(v.text_edit.as_ref().unwrap().new_text, "x");
}

// Go: lsp_json_test.go:683 TestUnmarshalPresenceDiscriminatorUnion, "InsertReplaceEdit via insert field"
#[test]
fn test_unmarshal_presence_discriminator_union_insert_replace_edit_via_insert_field() {
    let mut v = TextEditOrInsertReplaceEdit::default();
    let err = json_unmarshal(
        br#"{
			"insert": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
			"replace": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}},
			"newText": "y"
		}"#,
        &mut v,
        &[],
    );
    assert_nil_error("InsertReplaceEdit via insert field", &err);
    assert!(v.text_edit.is_none());
    assert!(v.insert_replace_edit.is_some());
    assert_eq!(v.insert_replace_edit.as_ref().unwrap().new_text, "y");
}

// Go: lsp_json_test.go:714 TestUnmarshalStringOrArrayUnion, "StringOrInlayHintLabelParts with string"
#[test]
fn test_unmarshal_string_or_array_union_string_or_inlay_hint_label_parts_with_string() {
    let mut v = StringOrInlayHintLabelParts::default();
    let err = json_unmarshal(br#""hello""#, &mut v, &[]);
    assert_nil_error("StringOrInlayHintLabelParts with string", &err);
    assert!(v.string.is_some());
    assert_eq!(v.string.as_deref().unwrap(), "hello");
    assert!(v.inlay_hint_label_parts.is_none());
}

// Go: lsp_json_test.go:714 TestUnmarshalStringOrArrayUnion, "StringOrInlayHintLabelParts with array"
#[test]
fn test_unmarshal_string_or_array_union_string_or_inlay_hint_label_parts_with_array() {
    let mut v = StringOrInlayHintLabelParts::default();
    let err = json_unmarshal(
        br#"[{"value": "param"}, {"value": ": "}, {"value": "string"}]"#,
        &mut v,
        &[],
    );
    assert_nil_error("StringOrInlayHintLabelParts with array", &err);
    assert!(v.string.is_none());
    assert!(v.inlay_hint_label_parts.is_some());
    assert_eq!(v.inlay_hint_label_parts.as_ref().unwrap().len(), 3);
    // Go `(*v.InlayHintLabelParts)[0].Value`: the element is a `*InlayHintLabelPart`.
    let first = v.inlay_hint_label_parts.as_ref().unwrap()[0]
        .as_ref()
        .unwrap();
    assert_eq!(first.value, "param");
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "TextDocumentEdit without kind"
#[test]
fn test_unmarshal_document_edit_union_text_document_edit_without_kind() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{
			"textDocument": {"uri": "file:///a.ts", "version": 1},
			"edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}]
		}"#,
        &mut v,
        &[],
    );
    assert_nil_error("TextDocumentEdit without kind", &err);
    assert!(v.text_document_edit.is_some());
    assert!(v.create_file.is_none());
    assert!(v.rename_file.is_none());
    assert!(v.delete_file.is_none());
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "TextDocumentEdit with non-string kind"
#[test]
fn test_unmarshal_document_edit_union_text_document_edit_with_non_string_kind() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{
			"kind": null,
			"textDocument": {"uri": "file:///a.ts", "version": 1},
			"edits": []
		}"#,
        &mut v,
        &[],
    );
    assert_nil_error("TextDocumentEdit with non-string kind", &err);
    assert!(v.text_document_edit.is_some());
    assert!(v.create_file.is_none());
    assert!(v.rename_file.is_none());
    assert!(v.delete_file.is_none());
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "CreateFile with kind create"
#[test]
fn test_unmarshal_document_edit_union_create_file_with_kind_create() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{"kind": "create", "uri": "file:///new.ts"}"#,
        &mut v,
        &[],
    );
    assert_nil_error("CreateFile with kind create", &err);
    assert!(v.text_document_edit.is_none());
    assert!(v.create_file.is_some());
    assert_eq!(
        v.create_file.as_ref().unwrap().uri,
        DocumentUri("file:///new.ts".to_string())
    );
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "CreateFile with kind after fields"
#[test]
fn test_unmarshal_document_edit_union_create_file_with_kind_after_fields() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{"uri": "file:///new.ts", "kind": "create"}"#,
        &mut v,
        &[],
    );
    assert_nil_error("CreateFile with kind after fields", &err);
    assert!(v.create_file.is_some());
    assert_eq!(
        v.create_file.as_ref().unwrap().uri,
        DocumentUri("file:///new.ts".to_string())
    );
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "RenameFile with kind rename"
#[test]
fn test_unmarshal_document_edit_union_rename_file_with_kind_rename() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{"kind": "rename", "oldUri": "file:///old.ts", "newUri": "file:///new.ts"}"#,
        &mut v,
        &[],
    );
    assert_nil_error("RenameFile with kind rename", &err);
    assert!(v.rename_file.is_some());
    assert_eq!(
        v.rename_file.as_ref().unwrap().old_uri,
        DocumentUri("file:///old.ts".to_string())
    );
}

// Go: lsp_json_test.go:739 TestUnmarshalDocumentEditUnion, "DeleteFile with kind delete"
#[test]
fn test_unmarshal_document_edit_union_delete_file_with_kind_delete() {
    let mut v = TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile::default();
    let err = json_unmarshal(
        br#"{"kind": "delete", "uri": "file:///gone.ts"}"#,
        &mut v,
        &[],
    );
    assert_nil_error("DeleteFile with kind delete", &err);
    assert!(v.delete_file.is_some());
    assert_eq!(
        v.delete_file.as_ref().unwrap().uri,
        DocumentUri("file:///gone.ts".to_string())
    );
}

// Go: lsp_json_test.go:809 TestUnmarshalFieldOrdering, "Location with reversed field order"
#[test]
fn test_unmarshal_field_ordering_location_with_reversed_field_order() {
    let mut loc = Location::default();
    let err = json_unmarshal(
        br#"{
			"range": {"start": {"line": 1, "character": 2}, "end": {"line": 3, "character": 4}},
			"uri": "file:///test.ts"
		}"#,
        &mut loc,
        &[],
    );
    assert_nil_error("Location with reversed field order", &err);
    assert_eq!(loc.uri, DocumentUri("file:///test.ts".to_string()));
    assert_eq!(loc.range.start.line, 1u32);
}

// Go: lsp_json_test.go:809 TestUnmarshalFieldOrdering, "InlayHint with kind before label"
#[test]
fn test_unmarshal_field_ordering_inlay_hint_with_kind_before_label() {
    let mut hint = InlayHint::default();
    let err = json_unmarshal(
        br#"{
			"kind": 1,
			"label": "x",
			"position": {"line": 0, "character": 0}
		}"#,
        &mut hint,
        &[],
    );
    assert_nil_error("InlayHint with kind before label", &err);
    assert!(hint.kind.is_some());
    assert_eq!(hint.kind.unwrap(), InlayHintKind::TYPE);
}

// Go: lsp_json_test.go:838 TestUnmarshalCompletionItemDataFileName (ts#64159)
#[test]
fn test_unmarshal_completion_item_data_file_name() {
    let mut data = CompletionItemData::default();
    let err = json_unmarshal(
        br#"{"fileName":"/src/index.ts","position":1,"name":"value"}"#,
        &mut data,
        &[],
    );
    assert_nil_error("CompletionItemData", &err);
    assert_eq!(data.file_name, "/src/index.ts");
}

// Go: lsp_json_test.go:847 TestUnmarshalEmptyObject, "WorkDoneProgressOptions empty"
#[test]
fn test_unmarshal_empty_object_work_done_progress_options_empty() {
    let mut v = WorkDoneProgressOptions::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert_nil_error("WorkDoneProgressOptions empty", &err);
    assert!(v.work_done_progress.is_none());
}

// Go: lsp_json_test.go:838 TestUnmarshalEmptyObject, "InitializationOptions empty"
#[test]
fn test_unmarshal_empty_object_initialization_options_empty() {
    let mut v = InitializationOptions::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert_nil_error("InitializationOptions empty", &err);
}

// Go: lsp_json_test.go:838 TestUnmarshalEmptyObject, "ClientCapabilities empty"
#[test]
fn test_unmarshal_empty_object_client_capabilities_empty() {
    let mut v = ClientCapabilities::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert_nil_error("ClientCapabilities empty", &err);
}

// Go: lsp_json_test.go:838 TestUnmarshalEmptyObject, "ServerCapabilities empty"
#[test]
fn test_unmarshal_empty_object_server_capabilities_empty() {
    let mut v = ServerCapabilities::default();
    let err = json_unmarshal(b"{}", &mut v, &[]);
    assert_nil_error("ServerCapabilities empty", &err);
}

// Go: lsp_json_test.go:871 TestMarshalOmitsZeroOptionalFields, "InlayHint omits nil fields"
#[test]
fn test_marshal_omits_zero_optional_fields_inlay_hint_omits_nil_fields() {
    let hint = InlayHint {
        position: Position {
            line: 0,
            character: 0,
        },
        label: StringOrInlayHintLabelParts {
            string: Some("x".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let data = json_marshal(&hint, &[]);
    assert_nil_error("InlayHint omits nil fields", &data);
    let s = data.unwrap();
    assert!(!s.contains("kind"), "should not contain 'kind', got: {s}");
    assert!(
        !s.contains("textEdits"),
        "should not contain 'textEdits', got: {s}"
    );
    assert!(
        !s.contains("paddingLeft"),
        "should not contain 'paddingLeft', got: {s}"
    );
    assert!(
        s.contains("position"),
        "should contain 'position', got: {s}"
    );
    assert!(s.contains("label"), "should contain 'label', got: {s}");
}

// Go: lsp_json_test.go:871 TestMarshalOmitsZeroOptionalFields, "FoldingRange omits nil optional fields"
#[test]
fn test_marshal_omits_zero_optional_fields_folding_range_omits_nil_optional_fields() {
    let fr = FoldingRange {
        start_line: 1,
        end_line: 10,
        ..Default::default()
    };
    let data = json_marshal(&fr, &[]);
    assert_nil_error("FoldingRange omits nil optional fields", &data);
    let s = data.unwrap();
    assert!(!s.contains("kind"), "should not contain 'kind', got: {s}");
    assert!(
        !s.contains("startCharacter"),
        "should not contain 'startCharacter', got: {s}"
    );
    assert!(
        s.contains("startLine"),
        "should contain 'startLine', got: {s}"
    );
    assert!(s.contains("endLine"), "should contain 'endLine', got: {s}");
}

// Go: lsp_json_test.go:903 TestLiteralTypes, "StringLiteralCreate marshal"
#[test]
fn test_literal_types_string_literal_create_marshal() {
    let v = StringLiteralCreate;
    let data = json_marshal(&v, &[]);
    assert_nil_error("StringLiteralCreate marshal", &data);
    assert_eq!(data.unwrap(), r#""create""#);
}

// Go: lsp_json_test.go:903 TestLiteralTypes, "StringLiteralCreate unmarshal"
#[test]
fn test_literal_types_string_literal_create_unmarshal() {
    let mut v = StringLiteralCreate;
    let err = json_unmarshal(br#""create""#, &mut v, &[]);
    assert_nil_error("StringLiteralCreate unmarshal", &err);
}

// Go: lsp_json_test.go:903 TestLiteralTypes, "StringLiteralCreate rejects wrong value"
#[test]
fn test_literal_types_string_literal_create_rejects_wrong_value() {
    let mut v = StringLiteralCreate;
    let err = json_unmarshal(br#""delete""#, &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:903 TestLiteralTypes, "StringLiteralCreate rejects wrong type"
#[test]
fn test_literal_types_string_literal_create_rejects_wrong_type() {
    let mut v = StringLiteralCreate;
    let err = json_unmarshal(b"42", &mut v, &[]);
    assert!(err.is_err());
}

// Go: lsp_json_test.go:936 TestEnumStringValues, "InlayHintKind values"
#[test]
fn test_enum_string_values_inlay_hint_kind_values() {
    assert_eq!(InlayHintKind::TYPE.string(), "Type");
    assert_eq!(InlayHintKind::PARAMETER.string(), "Parameter");
}

// Go: lsp_json_test.go:936 TestEnumStringValues, "SymbolKind values"
#[test]
fn test_enum_string_values_symbol_kind_values() {
    assert_eq!(SymbolKind::FILE.string(), "File");
    assert_eq!(SymbolKind::FUNCTION.string(), "Function");
    assert_eq!(SymbolKind::VARIABLE.string(), "Variable");
}

// Go: lsp_json_test.go:936 TestEnumStringValues, "unknown enum value"
#[test]
fn test_enum_string_values_unknown_enum_value() {
    let v = InlayHintKind(999);
    let s = v.string();
    assert!(
        s.contains("999"),
        "should contain the numeric value, got: {s}"
    );
}

/// One `TestRoundTrip` case: marshal, unmarshal into a new value, marshal
/// again; both texts must be equal.
#[track_caller]
fn check_round_trip<T: MarshalerTo + UnmarshalerFrom + Default>(name: &str, value: &T) {
    let data = json_marshal(value, &[]);
    assert_nil_error(name, &data);
    let data = data.expect("checked");
    let mut got = T::default();
    let err = json_unmarshal(data.as_bytes(), &mut got, &[]);
    assert_nil_error(name, &err);
    let again = json_marshal(&got, &[]);
    assert_nil_error(name, &again);
    assert_eq!(data, again.expect("checked"), "{name}: re-marshal differs");
}

// Go: lsp_json_test.go:966 TestRoundTrip
// TestRoundTrip locks the generated codecs: every value must survive
// marshal -> unmarshal unchanged. This guards fidelity so codec changes
// (e.g. pruning or table-driving them) cannot silently corrupt the wire
// format. Cover a representative spread of shapes: required fields,
// nullable/non-nullable optionals, enums, slices, nested objects, and
// unions.
// PORT: Go keeps the cases in one `[]any` table; the Rust cases have
// different types, so each is one call.
#[test]
fn test_round_trip() {
    check_round_trip(
        "Range",
        &Range {
            start: Position {
                line: 1,
                character: 2,
            },
            end: Position {
                line: 3,
                character: 4,
            },
        },
    );
    check_round_trip(
        "TextEdit",
        &TextEdit {
            range: Range {
                start: Position {
                    line: 1,
                    character: 2,
                },
                end: Position {
                    line: 3,
                    character: 4,
                },
            },
            new_text: "hello".to_string(),
        },
    );
    check_round_trip(
        "MarkupContent",
        &MarkupContent {
            kind: MarkupKind::MARKDOWN,
            value: "**x**".to_string(),
        },
    );
    let mut inner = IndexMap::new();
    inner.insert("x".to_string(), LspAny::Number(1.0));
    let mut settings = IndexMap::new();
    settings.insert("js/ts".to_string(), LspAny::Object(inner));
    check_round_trip(
        "DidChangeConfigurationParams object",
        &DidChangeConfigurationParams {
            settings: LspAny::Object(settings),
        },
    );
    check_round_trip(
        "DidChangeConfigurationParams null",
        &DidChangeConfigurationParams {
            settings: LspAny::Null,
        },
    );
    check_round_trip(
        "CompletionItem",
        &CompletionItem {
            label: "pageXOffset".to_string(),
            kind: Some(CompletionItemKind::FIELD),
            sort_text: Some("15".to_string()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        },
    );
    // StringOrTuple union (string arm and tuple arm).
    check_round_trip(
        "ParameterInformation string label",
        &ParameterInformation {
            label: StringOrTuple {
                string: Some("p: number".to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    check_round_trip(
        "ParameterInformation tuple label",
        &ParameterInformation {
            label: StringOrTuple {
                tuple: Some([0, 4]),
                ..Default::default()
            },
            ..Default::default()
        },
    );
}

// Go: lsp_json_test.go:1017 TestStrictnessMissingRequired
// TestStrictnessMissingRequired confirms required fields are still enforced;
// default reflective decoding would silently accept these.
#[test]
fn test_strictness_missing_required() {
    assert_error_contains(
        "TextEdit missing newText",
        &json_unmarshal(
            br#"{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}"#,
            &mut TextEdit::default(),
            &[],
        ),
        "missing required properties",
    );
    assert_error_contains(
        "Range missing end",
        &json_unmarshal(
            br#"{"start":{"line":0,"character":0}}"#,
            &mut Range::default(),
            &[],
        ),
        "missing required properties",
    );
    assert_error_contains(
        "Position missing character",
        &json_unmarshal(br#"{"line":0}"#, &mut Position::default(), &[]),
        "missing required properties",
    );
}

// Go: lsp_json_test.go:1039 TestStrictnessNotObject
// TestStrictnessNotObject confirms a non-object where an object is required
// is rejected rather than coerced.
#[test]
fn test_strictness_not_object() {
    let err = json_unmarshal(br#""oops""#, &mut TextEdit::default(), &[]);
    let Err(err) = err else {
        panic!("expected an error");
    };
    let text = err.to_string();
    assert!(
        text.contains("object") || text.contains("cannot unmarshal"),
        "got {text:?}"
    );
}

/// A request whose raw params are `params` (`None` is absent).
fn request_with_params(params: Option<&str>) -> RequestMessage {
    RequestMessage {
        params: params.map(|p| Box::new(JsonValue(p.as_bytes().to_vec())) as Box<dyn AnyValue>),
        ..RequestMessage::default()
    }
}

/// Go `assert.ErrorIs(t, err, ErrorCodeInvalidParams)`.
fn is_invalid_params(err: &GoError) -> bool {
    gostd::errors::is(err, &gostd::errors::from_value(ErrorCode::INVALID_PARAMS))
}

// Go: lsp_json_test.go:1049 TestUnmarshalParamsRequiresParams
// TestUnmarshalParamsRequiresParams verifies that a NoParams method must be
// given no params while every other method must be given params, and that a
// mismatch (including a null value either way) is an InvalidParams error.
#[test]
fn test_unmarshal_params_requires_params() {
    // NoParams: only truly-absent/empty params are accepted; null and any
    // present value are rejected.
    for (name, params, want_err) in [
        ("absent", None, false),
        ("empty", Some(""), false),
        ("null", Some("null"), true),
        ("object", Some("{}"), true),
    ] {
        let result = unmarshal_params::<NoParams>(&request_with_params(params));
        match result {
            Err(err) => assert!(
                want_err && is_invalid_params(&err),
                "NoParams/{name}: unexpected error {}",
                err.error()
            ),
            Ok(_) => assert!(!want_err, "NoParams/{name}: expected an error"),
        }
    }

    // Required-params method: only an object or array is accepted; absent,
    // empty, null, and other scalars are rejected.
    for (name, params, want_err) in [
        ("absent", None, true),
        ("empty", Some(""), true),
        ("null", Some("null"), true),
        ("number", Some("5"), true),
        ("string", Some(r#""x""#), true),
        ("object", Some(r#"{"settings":{"x":1}}"#), false),
    ] {
        let result = unmarshal_params::<DidChangeConfigurationParams>(&request_with_params(params));
        match result {
            Err(err) => assert!(
                want_err && is_invalid_params(&err),
                "typed/{name}: unexpected error {}",
                err.error()
            ),
            Ok(got) => {
                assert!(!want_err, "typed/{name}: expected an error");
                assert!(
                    got.settings != LspAny::Null,
                    "typed/{name}: settings is nil"
                );
            }
        }
    }
}

// PORT: no Go counterpart. Go `WorkspaceEdit.Changes` is
// `*map[DocumentUri][]*TextEdit`: a null element decodes as a nil edit, a nil
// edit encodes as null, and a value that is not an array names the Go type.
#[test]
fn workspace_edit_changes_hold_nil_edits() {
    let input = r#"{"changes":{"file:///a.ts":[null,{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"newText":"x"}]}}"#;
    let mut edit = WorkspaceEdit::default();
    let err = json_unmarshal(input.as_bytes(), &mut edit, &[]);
    assert_nil_error("decode", &err);
    let edits = &edit.changes.as_ref().unwrap()[&DocumentUri("file:///a.ts".to_string())];
    assert!(edits[0].is_none(), "a null element is a nil edit");
    assert_eq!(edits[1].as_ref().unwrap().new_text, "x");
    assert_eq!(json_marshal(&edit, &[]).unwrap(), input);

    let mut edit = WorkspaceEdit::default();
    let err = json_unmarshal(br#"{"changes":{"file:///a.ts":1}}"#, &mut edit, &[]);
    assert_error_contains("non-array", &err, "[]*lsproto.TextEdit");
}
