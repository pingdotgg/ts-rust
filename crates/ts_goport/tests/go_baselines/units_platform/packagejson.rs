//! Go: `internal/packagejson/{expected,exportsorimports,jsonvalue,packagejson}_test.go`.
//!
//! PORT: Go unmarshals into local structs by reflection. Here each local
//! struct implements `UnmarshalerFrom` with `decode_struct`, the JSON v2
//! struct rules the tests need (names match exactly, unknown members are
//! skipped). Go `OrderedMap.GetOrZero` is `get(..).cloned().unwrap_or_default()`.

use ts_goport::frontend::json::{
    JsonDecoder, JsonError, JsonToken, UnmarshalerFrom, json_unmarshal, json_unmarshal_decode,
};
use ts_goport::frontend::packagejson::{
    self, ContentMapperFields, Expected, ExportsOrImports, JSONValue, JSONValueType, JsonAny,
};

/// Decodes one JSON object; `field` decodes the member `name` and returns
/// `None` for a member the struct does not have.
fn decode_struct(
    dec: &mut JsonDecoder<'_>,
    mut field: impl FnMut(&str, &mut JsonDecoder<'_>) -> Option<Result<(), JsonError>>,
) -> Result<(), JsonError> {
    match dec.read_token()? {
        JsonToken::BeginObject => {}
        JsonToken::Null => return Ok(()),
        other => panic!("expected an object, got {other:?}"),
    }
    while dec.peek_kind() != b'}' {
        let JsonToken::String(name) = dec.read_token()? else {
            unreachable!("object member names are strings")
        };
        match field(&name, dec) {
            Some(result) => result?,
            None => dec.skip_value()?,
        }
    }
    dec.read_token()?;
    Ok(())
}

// Go: expected_test.go:14 packageJson
// PORT: Go `Expected[any]` for "exports" is `Expected<String>`: the Rust
// `Expected` needs a concrete JSON kind. The test reads only its null flags,
// which do not depend on `T`.
#[derive(Default)]
struct ExpectedPackageJson {
    name: Expected<String>,
    version: Expected<String>,
    exports: Expected<String>,
    main: Expected<String>,
}

impl UnmarshalerFrom for ExpectedPackageJson {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        decode_struct(dec, |name, dec| match name {
            "name" => Some(json_unmarshal_decode(dec, &mut self.name)),
            "version" => Some(json_unmarshal_decode(dec, &mut self.version)),
            "exports" => Some(json_unmarshal_decode(dec, &mut self.exports)),
            "main" => Some(json_unmarshal_decode(dec, &mut self.main)),
            _ => None,
        })
    }
}

// Go: expected_test.go:11 TestExpected
#[test]
fn test_expected() {
    let json_string = r#"{
		"name": "test",
		"version": 2,
		"exports": null
	}"#;
    let mut p = ExpectedPackageJson::default();
    json_unmarshal(json_string.as_bytes(), &mut p, &[]).expect("unmarshal");

    assert!(p.name.valid);
    assert_eq!(p.name.value, "test");

    assert!(!p.version.valid);
    assert_eq!(p.version.value, "");

    assert!(p.exports.null);
    assert!(!p.exports.valid);

    assert!(!p.main.valid);
    assert!(!p.main.null);
    assert_eq!(p.main.value, "");
}

// Go: exportsorimports_test.go:19 Exports
#[derive(Default)]
struct Exports {
    imports: ExportsOrImports,
    exports: ExportsOrImports,
}

impl UnmarshalerFrom for Exports {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        decode_struct(dec, |name, dec| match name {
            "imports" => Some(json_unmarshal_decode(dec, &mut self.imports)),
            "exports" => Some(json_unmarshal_decode(dec, &mut self.exports)),
            _ => None,
        })
    }
}

fn get_or_zero<V: Clone + Default>(map: &indexmap::IndexMap<String, V>, key: &str) -> V {
    map.get(key).cloned().unwrap_or_default()
}

// Go: exportsorimports_test.go:11 TestExports (subtest UnmarshalJSONV2)
#[test]
fn test_exports() {
    let json_string = r##"{
		"imports": {
			"#foo": {
				"import": "./foo.ts"
			}
		},
		"exports": {
			".": {
				"import": "./test.ts",
				"default": "./test.ts"
			},
			"./test": [
				"./test1.ts",
				"./test2.ts",
				null
			],
			"./null": null
		}
	}"##;
    let mut e = Exports::default();
    json_unmarshal(json_string.as_bytes(), &mut e, &[]).expect("unmarshal");

    assert!(e.exports.is_subpaths());
    assert_eq!(e.exports.as_object().len(), 3);
    let dot = get_or_zero(e.exports.as_object(), ".");
    assert!(dot.is_conditions());
    assert!(get_or_zero(dot.as_object(), "import").json_value.type_ == JSONValueType::STRING);
    assert_eq!(
        get_or_zero(e.exports.as_object(), "./test").as_array()[2]
            .json_value
            .type_,
        JSONValueType::NULL
    );
    assert!(
        get_or_zero(e.exports.as_object(), "./null")
            .json_value
            .type_
            == JSONValueType::NULL
    );

    assert!(e.imports.is_imports());
    assert_eq!(e.imports.as_object().len(), 1);
    let foo = get_or_zero(e.imports.as_object(), "#foo");
    assert!(foo.is_conditions());
    assert!(get_or_zero(foo.as_object(), "import").json_value.type_ == JSONValueType::STRING);
}

// Go: jsonvalue_test.go:19 packageJson
#[derive(Default)]
struct JsonValuePackageJson {
    private: JSONValue,
    false_: JSONValue,
    name: JSONValue,
    version: JSONValue,
    exports: JSONValue,
    imports: JSONValue,
    not_present: JSONValue,
}

impl UnmarshalerFrom for JsonValuePackageJson {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        decode_struct(dec, |name, dec| match name {
            "private" => Some(json_unmarshal_decode(dec, &mut self.private)),
            "false" => Some(json_unmarshal_decode(dec, &mut self.false_)),
            "name" => Some(json_unmarshal_decode(dec, &mut self.name)),
            "version" => Some(json_unmarshal_decode(dec, &mut self.version)),
            "exports" => Some(json_unmarshal_decode(dec, &mut self.exports)),
            "imports" => Some(json_unmarshal_decode(dec, &mut self.imports)),
            "notPresent" => Some(json_unmarshal_decode(dec, &mut self.not_present)),
            _ => None,
        })
    }
}

fn value_str(v: &JSONValue) -> Option<&str> {
    match &v.value {
        JsonAny::String(s) => Some(s),
        _ => None,
    }
}

// Go: jsonvalue_test.go:11 TestJSONValue (subtest UnmarshalJSONV2)
#[test]
fn test_json_value() {
    let json_string = r#"{
		"private": true,
		"false": false,
		"name": "test",
		"version": 2,
		"exports": {
			".": {
				"import": "./test.ts",
				"default": "./test.ts"
			},
			"./test": [
				"./test1.ts",
				"./test2.ts",
				null
			],
			"./null": null
		},
		"imports": null
	}"#;
    let mut p = JsonValuePackageJson::default();
    json_unmarshal(json_string.as_bytes(), &mut p, &[]).expect("unmarshal");

    assert_eq!(p.private.type_, JSONValueType::BOOLEAN);
    assert!(matches!(p.private.value, JsonAny::Bool(true)));

    assert_eq!(p.name.type_, JSONValueType::STRING);
    assert_eq!(value_str(&p.name), Some("test"));

    assert_eq!(p.version.type_, JSONValueType::NUMBER);
    assert!(matches!(p.version.value, JsonAny::Number(n) if n == 2.0));

    assert_eq!(p.exports.type_, JSONValueType::OBJECT);
    assert_eq!(p.exports.as_object().len(), 3);
    let dot = get_or_zero(p.exports.as_object(), ".");
    assert_eq!(dot.type_, JSONValueType::OBJECT);
    assert_eq!(
        value_str(&get_or_zero(dot.as_object(), "import")),
        Some("./test.ts")
    );

    let test = get_or_zero(p.exports.as_object(), "./test");
    assert_eq!(test.type_, JSONValueType::ARRAY);
    assert_eq!(test.as_array().len(), 3);
    assert_eq!(value_str(&test.as_array()[0]), Some("./test1.ts"));
    assert_eq!(value_str(&test.as_array()[1]), Some("./test2.ts"));
    assert_eq!(test.as_array()[2].type_, JSONValueType::NULL);

    assert_eq!(
        get_or_zero(p.exports.as_object(), "./null").type_,
        JSONValueType::NULL
    );

    assert_eq!(p.imports.type_, JSONValueType::NULL);
    assert!(matches!(p.imports.value, JsonAny::Nil));

    assert_eq!(p.not_present.type_, JSONValueType::NOT_PRESENT);
    assert!(matches!(p.not_present.value, JsonAny::Nil));
    let _ = &p.false_;
}

/// The exported fields of a Go `Expected[T]` (Go `cmpopts.IgnoreUnexported`).
fn exported<T: Clone>(e: &Expected<T>) -> (bool, bool, T) {
    (e.null, e.valid, e.value.clone())
}

/// The exported fields of a Go `Expected[ContentMapperFields]`, with the
/// exported fields of each nested `Expected` (Go `cmpopts.IgnoreUnexported`).
type ContentMapperExported = (
    bool,
    bool,
    (bool, bool, Vec<String>),
    (bool, bool, Vec<String>),
    (bool, bool, bool),
);

fn exported_content_mapper(e: &Expected<ContentMapperFields>) -> ContentMapperExported {
    (
        e.null,
        e.valid,
        exported(&e.value.exec),
        exported(&e.value.compiler_options),
        exported(&e.value.dynamic_config),
    )
}

/// Go `assert.DeepEqual(got, want, cmpopts.IgnoreUnexported(...))` for a
/// want value that sets only the name, the version and the content mapper.
fn assert_parse_result(
    got: &packagejson::Fields,
    name: &Expected<String>,
    version: &Expected<String>,
    content_mapper: &Expected<ContentMapperFields>,
) {
    assert_eq!(exported(&got.header_fields.name), exported(name));
    assert_eq!(exported(&got.header_fields.version), exported(version));
    // Every other exported field is the zero value.
    let zero: Expected<String> = Expected::default();
    assert_eq!(exported(&got.header_fields.type_), exported(&zero));
    let paths = &got.path_fields;
    for e in [&paths.ts_config, &paths.main, &paths.types, &paths.typings] {
        assert_eq!(exported(e), exported(&zero));
    }
    assert_eq!(paths.types_versions.type_, JSONValueType::NOT_PRESENT);
    assert_eq!(paths.imports.json_value.type_, JSONValueType::NOT_PRESENT);
    assert_eq!(paths.exports.json_value.type_, JSONValueType::NOT_PRESENT);
    let deps = &got.dependency_fields;
    for e in [
        &deps.dependencies,
        &deps.dev_dependencies,
        &deps.peer_dependencies,
        &deps.optional_dependencies,
    ] {
        assert!(!e.null && !e.valid && e.value.is_empty());
    }
    assert_eq!(
        exported_content_mapper(&got.content_mapper),
        exported_content_mapper(content_mapper)
    );
}

// Go: packagejson_test.go:63 TestParse
#[test]
fn test_parse() {
    let zero: Expected<String> = Expected::default();
    let no_content_mapper: Expected<ContentMapperFields> = Expected::default();

    // duplicate names
    let content = r#"{
				"name": "test-package",
				"name": "test-package",
				"version": "1.0.0"
			}"#;
    let got = packagejson::parse(content.as_bytes()).expect("Parse");
    assert_parse_result(
        &got,
        &packagejson::expected_of("test-package".to_string()),
        &packagejson::expected_of("1.0.0".to_string()),
        &no_content_mapper,
    );

    // content mapper (tsgo#4712)
    let content = r#"{
				"name": "test-package",
				"typescript": {
					"contentMapper": { "exec": ["mapper"], "dynamicConfig": true }
				}
			}"#;
    let got = packagejson::parse(content.as_bytes()).expect("Parse");
    assert_parse_result(
        &got,
        &packagejson::expected_of("test-package".to_string()),
        &zero,
        &packagejson::expected_of(ContentMapperFields {
            exec: packagejson::expected_of(vec!["mapper".to_string()]),
            dynamic_config: packagejson::expected_of(true),
            ..Default::default()
        }),
    );

    // invalid typescript field is ignored (tsgo#4712)
    let content = r#"{ "name": "test-package", "typescript": "invalid" }"#;
    let got = packagejson::parse(content.as_bytes()).expect("Parse");
    assert_parse_result(
        &got,
        &packagejson::expected_of("test-package".to_string()),
        &zero,
        &no_content_mapper,
    );
}

// Go: packagejson_test.go:58 TestForEachAncestorDirectoryStoppingAtGlobalCache (ts#64159)
// PORT: Go walks `packagejson.PackageDirectory` values (a name and its
// path key). The port walks directory names with
// `tspath::for_each_ancestor_directory_stopping_at_global_cache`; the key
// of a name is its `tspath::to_path`.
#[test]
fn test_for_each_ancestor_directory_stopping_at_global_cache() {
    use ts_goport::frontend::tspath;
    let mut names = Vec::new();
    let mut keys = Vec::new();
    tspath::for_each_ancestor_directory_stopping_at_global_cache(
        "/Repo",
        "/Repo/Project/src",
        |directory| {
            names.push(directory.to_string());
            keys.push(tspath::to_path(directory, "", false).to_string());
            ((), false)
        },
    );
    assert_eq!(names, ["/Repo/Project/src", "/Repo/Project", "/Repo"]);
    assert_eq!(keys, ["/repo/project/src", "/repo/project", "/repo"]);
}
