//! Rust port of `internal/tsoptions/tsconfigparsing_test.go`.
//!
//! Baselines: `config/tsconfigParsing` (`baseline.Run`).
//!
//! PORT: Go runs the subtests in parallel (`t.Parallel`); here they run in
//! order. `BenchmarkParseSrcCompiler` is a benchmark, not a test, and is not
//! ported.
//!
//! The test data tables are generated from the Go source, so the text of
//! each config (tabs included) is the same bytes.

use std::collections::BTreeMap;

use ts_goport::contentmapper::OptionPathSegment;
use ts_goport::frontend::json::{JsonError, MarshalerTo};
use ts_goport::frontend::prelude::*;
use ts_goport::frontend::tsoptions;
use ts_goport::scanner_util::get_ecma_line_and_utf16_character_of_position;

use super::tsoptionstest::{
    AnyJson, CompilerOptionsJson, Subtests, TypeAcquisitionJson, VfsParseConfigHost, file_map,
    format_diagnostics_with_color_and_context, formatting_options, get_parsed_command_line,
    marshal_indent_write, new_vfs_parse_config_host,
};
use crate::support::baseline;

// Go: tsconfigparsing_test.go:31 testConfig
#[derive(Clone)]
struct TestConfig {
    json_text: &'static str,
    config_file_name: &'static str,
    base_path: &'static str,
    all_file_list: BTreeMap<String, String>,
    // tsgo#4712
    existing_options: Option<CompilerOptions>,
}

// Go: tsconfigparsing_test.go:39 parseConfigFileTextToJsonTests (element type)
struct ParseConfigFileTextToJsonTest {
    title: &'static str,
    input: &'static [&'static str],
}

// Go: tsconfigparsing_test.go:129 TestParseConfigFileTextToJson
#[test]
fn parse_config_file_text_to_json() {
    let mut t = Subtests::new("TestParseConfigFileTextToJson");
    for rec in parse_config_file_text_to_json_tests() {
        t.run(rec.title, || {
            let mut baseline_content = String::new();
            for (i, json_text) in rec.input.iter().enumerate() {
                baseline_content.push_str("Input::\n");
                baseline_content.push_str(json_text);
                baseline_content.push('\n');
                let (parsed, errors) = tsoptions::parse_config_file_text_to_json(
                    "/apath/tsconfig.json",
                    Path("/apath".to_string()),
                    json_text,
                );
                baseline_content.push_str("Config::\n");
                if let Err(err) = write_json_readable_text(&mut baseline_content, &AnyJson(&parsed))
                {
                    panic!("Failed to write JSON text: {err}");
                }
                baseline_content.push('\n');
                baseline_content.push_str("Errors::\n");
                format_diagnostics_with_color_and_context(
                    &mut baseline_content,
                    &errors,
                    &formatting_options("\n", "/", true),
                );
                baseline_content.push('\n');
                if i != rec.input.len() - 1 {
                    baseline_content.push('\n');
                }
            }
            baseline::run(
                &format!("{} jsonParse.js", rec.title),
                &baseline_content,
                &baseline::Options {
                    subfolder: "config/tsconfigParsing".into(),
                    ..Default::default()
                },
            )
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:159 parseJsonConfigTestCase
struct ParseJsonConfigTestCase {
    title: &'static str,
    include_compiler_options: bool,
    input: Vec<TestConfig>,
}

/// Go `func(config testConfig, host tsoptions.ParseConfigHost, basePath string) *tsoptions.ParsedCommandLine`.
type GetParsed = fn(&TestConfig, &dyn ParseConfigHost, &str) -> ParsedCommandLine;

// Go: tsconfigparsing_test.go:871 TestParseJsonConfigFileContent
#[test]
fn parse_json_config_file_content() {
    let mut t = Subtests::new("TestParseJsonConfigFileContent");
    for rec in parse_json_config_file_tests() {
        let name = format!("{} with json api", rec.title);
        t.run(&name, || {
            baseline_parse_config_with(
                &format!("{} with json api.js", rec.title),
                rec.include_compiler_options,
                &rec.input,
                get_parsed_with_json_api,
            )
        });
    }
    t.finish();
}

/// A Go `*collections.OrderedMap[string, any]` literal.
fn json_object(entries: Vec<(&str, CompilerOptionsValue)>) -> CompilerOptionsValue {
    CompilerOptionsValue::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

/// Go `tsoptions.ParseJsonConfigFileContent(json, host, "/project", nil,
/// "/project/tsconfig.json", nil, nil, nil)`, as the tests below call it.
fn parse_project_json(json: &CompilerOptionsValue, host: &VfsParseConfigHost) -> ParsedCommandLine {
    tsoptions::parse_json_config_file_content(
        json,
        host,
        "/project",
        None,
        "/project/tsconfig.json",
        /*resolutionStack*/ &[],
        /*extendedConfigCache*/ None,
    )
}

// Go: tsconfigparsing_test.go:881 TestParseJsonConfigFileContentAcceptsJsonRepresentations
// PORT: `CompilerOptionsValue` has no form for a Go `map[string]any`, so the
// "plain map" and "typed slices" cases are ordered maps in the sorted key
// order that Go `normalizeJsonValue` gives them, and a Go `[]string` is
// `StringList`. Go runs the cases in map order; here they run in source
// order.
#[test]
fn parse_json_config_file_content_accepts_json_representations() {
    let host = new_vfs_parse_config_host(
        &file_map(&[("/project/index.ts", "export {};")]),
        "/project",
        true, /*useCaseSensitiveFileNames*/
    );

    let (ordered_map, parse_errors) = tsoptions::parse_config_file_text_to_json(
        "/project/tsconfig.json",
        Path("/project/tsconfig.json".to_string()),
        r#"{"compilerOptions":{"strict":true},"files":["index.ts"]}"#,
    );
    assert_eq!(parse_errors.len(), 0);

    let strict = || json_object(vec![("strict", CompilerOptionsValue::Bool(true))]);
    let ordered_map_with_typed_slices = json_object(vec![
        ("compilerOptions", strict()),
        (
            "files",
            CompilerOptionsValue::StringList(vec!["index.ts".to_string()]),
        ),
    ]);

    let tests: Vec<(&str, CompilerOptionsValue)> = vec![
        ("ordered map", ordered_map),
        (
            "ordered map with typed slices",
            ordered_map_with_typed_slices,
        ),
        (
            "plain map",
            json_object(vec![
                ("compilerOptions", strict()),
                (
                    "files",
                    CompilerOptionsValue::List(vec![CompilerOptionsValue::String(
                        "index.ts".to_string(),
                    )]),
                ),
            ]),
        ),
        (
            "typed slices",
            json_object(vec![
                ("compilerOptions", strict()),
                (
                    "files",
                    CompilerOptionsValue::StringList(vec!["index.ts".to_string()]),
                ),
            ]),
        ),
    ];
    let mut t = Subtests::new("TestParseJsonConfigFileContentAcceptsJsonRepresentations");
    for (name, json) in &tests {
        t.run(name, || {
            let parsed = parse_project_json(json, &host);
            assert_eq!(parsed.file_names(), ["/project/index.ts".to_string()]);
            assert!(parsed.compiler_options().strict.is_true());
            assert_eq!(parsed.errors.len(), 0);
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:930 TestParseJsonConfigFileContentPreservesRaw
// PORT: the Go `map[string]any` input is an ordered map in its sorted key
// order (see above).
#[test]
fn parse_json_config_file_content_preserves_raw() {
    let host = new_vfs_parse_config_host(
        &file_map(&[("/project/index.ts", "export {};")]),
        "/project",
        true, /*useCaseSensitiveFileNames*/
    );

    let parsed = parse_project_json(
        &json_object(vec![
            ("compileOnSave", CompilerOptionsValue::Bool(true)),
            (
                "customSetting",
                json_object(vec![("enabled", CompilerOptionsValue::Bool(true))]),
            ),
            (
                "files",
                CompilerOptionsValue::List(vec![CompilerOptionsValue::String(
                    "index.ts".to_string(),
                )]),
            ),
        ]),
        &host,
    );

    assert_eq!(parsed.errors.len(), 0);
    assert!(parsed.compile_on_save == Some(true));

    let CompilerOptionsValue::Map(raw) = &parsed.raw else {
        panic!("raw should be an ordered map");
    };
    assert_eq!(
        raw.keys().map(String::as_str).collect::<Vec<_>>(),
        ["compileOnSave", "customSetting", "files"]
    );
    assert!(raw.contains_key("customSetting"));
}

// Go: tsconfigparsing_test.go:959 TestParseJsonConfigFileContentHandlesNullArrayElements
#[test]
fn parse_json_config_file_content_handles_null_array_elements() {
    let host = new_vfs_parse_config_host(
        &file_map(&[("/project/index.ts", "export {};")]),
        "/project",
        true, /*useCaseSensitiveFileNames*/
    );
    let mut t = Subtests::new("TestParseJsonConfigFileContentHandlesNullArrayElements");
    for property in ["files", "include", "exclude"] {
        t.run(property, || {
            let parsed = parse_project_json(
                &json_object(vec![(
                    property,
                    CompilerOptionsValue::List(vec![CompilerOptionsValue::Nil]),
                )]),
                &host,
            );
            assert!(!parsed.errors.is_empty());
            assert_eq!(
                parsed.errors[0].code,
                diag::Compiler_option_0_requires_a_value_of_type_1.code() as i32
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:983 TestParseJsonConfigFileContentDefaultsCompileOnSaveToFalse
#[test]
fn parse_json_config_file_content_defaults_compile_on_save_to_false() {
    let host = new_vfs_parse_config_host(
        &file_map(&[("/project/index.ts", "export {};")]),
        "/project",
        true, /*useCaseSensitiveFileNames*/
    );
    let parsed = parse_project_json(
        &json_object(vec![(
            "files",
            CompilerOptionsValue::List(vec![CompilerOptionsValue::String("index.ts".to_string())]),
        )]),
        &host,
    );
    assert!(parsed.compile_on_save.is_some());
    assert_eq!(parsed.compile_on_save, Some(false));
}

// PORT: no Go test. The project reference network check (ts#64832) resolves
// the path against the config file's directory, not the base path: Go calls
// getProjectReferences(basePathForFileNames) (tsconfigparsing.go:1478, check
// at :1449). The expected values are the Go N' (3e9236361fa9) output of the
// same calls (a `go test -overlay` probe, bump E L1 round 2):
//   base /apath, config /apath/sub/tsconfig.json: no error, reference
//     /apath/??/UNC/server/share/project
//   base /apath/sub, config /apath/tsconfig.json: TS5116 (JSON API at -1..-1,
//     source file API at 50..82), no reference
#[test]
fn project_reference_network_check_uses_config_directory() {
    const TEXT: &str = "{\n  \"files\": [\"a.ts\"],\n  \"references\": [{ \"path\": \"../??/UNC/server/share/project\" }]\n}";
    const REFERENCE: &str = "../??/UNC/server/share/project";
    // (base path, config file, file names, (JSON API, source file API) error ranges, reference)
    let cases = [
        (
            "/apath",
            "/apath/sub/tsconfig.json",
            "/apath/sub/a.ts",
            None,
            Some("/apath/??/UNC/server/share/project"),
        ),
        (
            "/apath/sub",
            "/apath/tsconfig.json",
            "/apath/a.ts",
            Some([(-1, -1), (50, 82)]),
            None,
        ),
    ];
    for (base_path, config_file_name, file_name, error_ranges, reference) in cases {
        let files = file_map(&[
            ("/apath/a.ts", ""),
            ("/apath/sub/a.ts", ""),
            (config_file_name, TEXT),
        ]);
        let host =
            new_vfs_parse_config_host(&files, base_path, true /*useCaseSensitiveFileNames*/);
        let path = to_path(config_file_name, base_path, true);
        let (json, _) =
            tsoptions::parse_config_file_text_to_json(config_file_name, path.clone(), TEXT);
        let via_json = tsoptions::parse_json_config_file_content(
            &json,
            &host,
            base_path,
            None,
            config_file_name,
            &[],
            None,
        );
        let via_source_file = tsoptions::parse_json_source_file_config_file_content(
            new_tsconfig_source_file_from_file_path(config_file_name, path, TEXT),
            &host,
            base_path,
            None,
            None,
            config_file_name,
            &[],
            None,
        );
        for (api, parsed) in [via_json, via_source_file].iter().enumerate() {
            let context = format!("base {base_path}, config {config_file_name}, api {api}");
            assert_eq!(parsed.parsed_config.file_names, [file_name], "{context}");
            let errors: Vec<_> = parsed
                .errors
                .iter()
                .map(|d| (d.code, d.pos, d.end, d.message_args.clone()))
                .collect();
            let expected_errors: Vec<_> = error_ranges
                .map(|ranges| {
                    (
                        5116,
                        ranges[api].0,
                        ranges[api].1,
                        vec![REFERENCE.to_string()],
                    )
                })
                .into_iter()
                .collect();
            assert_eq!(errors, expected_errors, "{context}");
            let references: Vec<_> = parsed
                .parsed_config
                .project_references
                .iter()
                .flatten()
                .map(|r| (r.path.as_str(), r.original_path.as_str()))
                .collect();
            let expected_references: Vec<_> = reference
                .map(|path| (path, REFERENCE))
                .into_iter()
                .collect();
            assert_eq!(references, expected_references, "{context}");
        }
    }
}

// Go: tsconfigparsing_test.go:1002 getParsedWithJsonApi
fn get_parsed_with_json_api(
    config: &TestConfig,
    host: &dyn ParseConfigHost,
    base_path: &str,
) -> ParsedCommandLine {
    let config_file_name = get_normalized_absolute_path(config.config_file_name, base_path);
    let path = to_path(
        config.config_file_name,
        base_path,
        host.fs().use_case_sensitive_file_names(),
    );
    let (parsed, _) =
        tsoptions::parse_config_file_text_to_json(&config_file_name, path, config.json_text);
    tsoptions::parse_json_config_file_content(
        &parsed,
        host,
        base_path,
        config.existing_options.as_ref(),
        &config_file_name,
        /*resolutionStack*/ &[],
        /*extendedConfigCache*/ None,
    )
}

// Go: tsconfigparsing_test.go:1017 TestParseJsonSourceFileConfigFileContent
#[test]
fn parse_json_source_file_config_file_content() {
    let mut t = Subtests::new("TestParseJsonSourceFileConfigFileContent");
    for rec in parse_json_config_file_tests() {
        let name = format!("{} with jsonSourceFile api", rec.title);
        t.run(&name, || {
            baseline_parse_config_with(
                &format!("{} with jsonSourceFile api.js", rec.title),
                rec.include_compiler_options,
                &rec.input,
                get_parsed_with_json_source_file_api,
            )
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1027 TestParseJsonSourceFileConfigFileContentReportsInvalidExtendedConfig
#[test]
fn parse_json_source_file_config_file_content_reports_invalid_extended_config() {
    let files = file_map(&[
        (
            "/project/tsconfig.json",
            "{\n  \"extends\": \"./bad.json\"\n}",
        ),
        // The parser recovers from this as object-like JSON, producing expected-token errors for ':', ',', ',', and '}'.
        ("/project/bad.json", "{ this is not json"),
        ("/project/main.ts", "export const x = 1;"),
    ]);
    let host =
        new_vfs_parse_config_host(&files, "/project", true /*useCaseSensitiveFileNames*/);
    let config_file_name = "/project/tsconfig.json";
    let config_file = new_tsconfig_source_file_from_file_path(
        config_file_name,
        to_path(
            config_file_name,
            &host.get_current_directory(),
            host.fs().use_case_sensitive_file_names(),
        ),
        &files[config_file_name],
    );

    let parsed = tsoptions::parse_json_source_file_config_file_content(
        config_file,
        &host,
        &host.get_current_directory(),
        None,
        None,
        config_file_name,
        &[],
        None,
    );

    let parse_errors: Vec<&Diagnostic> = parsed
        .errors
        .iter()
        .filter(|diagnostic| diagnostic.code == diag::X_0_expected.code() as i32)
        .collect();
    let expected_parse_error_messages = [":", ",", ",", "}"];
    let expected_parse_error_positions = [7, 10, 14, 18];
    assert_eq!(expected_parse_error_messages.len(), parse_errors.len());
    assert_eq!(
        parse_errors
            .iter()
            .map(|diagnostic| diagnostic.message_args[0].as_str())
            .collect::<Vec<_>>(),
        expected_parse_error_messages
    );
    assert_eq!(
        parse_errors
            .iter()
            .map(|diagnostic| diagnostic.pos)
            .collect::<Vec<_>>(),
        expected_parse_error_positions
    );
    for diagnostic in &parse_errors {
        assert_eq!(source_file_file_name(diagnostic.file), "/project/bad.json");
    }
}

// Extending an empty config file used to panic on nil Statements (#4265).
// Go: tsconfigparsing_test.go:1071 TestParseJsonSourceFileConfigFileContentWithEmptyExtendedConfig
#[test]
fn parse_json_source_file_config_file_content_with_empty_extended_config() {
    let files = file_map(&[
        (
            "/project/tsconfig.json",
            "{\n  \"extends\": \"./base.json\"\n}",
        ),
        ("/project/base.json", ""),
        ("/project/main.ts", "export const x = 1;"),
    ]);
    let host =
        new_vfs_parse_config_host(&files, "/project", true /*useCaseSensitiveFileNames*/);
    let config_file_name = "/project/tsconfig.json";
    let config_file = new_tsconfig_source_file_from_file_path(
        config_file_name,
        to_path(
            config_file_name,
            &host.get_current_directory(),
            host.fs().use_case_sensitive_file_names(),
        ),
        &files[config_file_name],
    );

    let parsed = tsoptions::parse_json_source_file_config_file_content(
        config_file,
        &host,
        &host.get_current_directory(),
        None,
        None,
        config_file_name,
        &[],
        None,
    );

    // PORT: Go asserts `parsed != nil`; the Rust result is a value.
    assert_eq!(
        parsed.file_names().to_vec(),
        vec!["/project/main.ts".to_string()]
    );
}

// PORT: no Go test. Go tests ts#64755 through the API
// (TestOpenProjectsWithCircularExtendsReportCircularity, ported in
// `project_lsp/api_session_apistate_test.rs`): without the fix Go deadlocks
// there, and the port has no such lock, so that test passes without it. This
// test checks the parse result on the same files: a config in an `extends`
// cycle is not in its own extended source files (tsconfigparsing.go:1106,
// with the file system's case sensitivity). The expected values are the Go N'
// (3e9236361fa9) output of the same calls (a `go test -overlay` probe, bump E
// L1 round 2). Go N' without the fix gives p2 its own path first.
#[test]
fn circular_extends_leave_config_out_of_its_extended_source_files() {
    const P1: &str = "/home/projects/p1/tsconfig.json";
    const P2: &str = "/home/projects/p2/tsconfig.json";
    const BASE: &str = "/home/projects/shared/base.json";
    // (useCaseSensitiveFileNames, base.json `extends`, p2 as p1 sees it)
    let cases = [
        (true, "../p2/tsconfig.json", P2),
        (
            false,
            "../P2/TSCONFIG.json",
            "/home/projects/P2/TSCONFIG.json",
        ),
    ];
    for (case_sensitive, base_extends, p2_extended) in cases {
        let base_text = format!("{{ \"extends\": \"{base_extends}\" }}");
        let files = file_map(&[
            (P1, r#"{ "extends": "../shared/base.json" }"#),
            (P2, r#"{ "extends": "../shared/base.json" }"#),
            (BASE, base_text.as_str()),
            ("/home/projects/p1/a.ts", "export const a = 1;"),
            ("/home/projects/p2/b.ts", "export const b = 1;"),
        ]);
        let host = new_vfs_parse_config_host(&files, "/", case_sensitive);
        for (config_file_name, extended, file_name) in [
            (P1, vec![p2_extended, BASE], "/home/projects/p1/a.ts"),
            (P2, vec![BASE], "/home/projects/p2/b.ts"),
        ] {
            let config_file = new_tsconfig_source_file_from_file_path(
                config_file_name,
                to_path(config_file_name, "", case_sensitive),
                &files[config_file_name],
            );
            let parsed = tsoptions::parse_json_source_file_config_file_content(
                config_file,
                &host,
                &get_directory_path(config_file_name),
                None,
                None,
                config_file_name,
                &[],
                None,
            );
            let context = format!("{config_file_name}, case sensitive {case_sensitive}");
            assert_eq!(parsed.extended_source_files(), extended, "{context}");
            let codes: Vec<_> = parsed.errors.iter().map(|d| d.code).collect();
            assert_eq!(codes, [18000], "{context}");
            assert_eq!(parsed.parsed_config.file_names, [file_name], "{context}");
        }
    }
}

// Go: tsconfigparsing_test.go:1102 TestParseJsonSourceFileConfigFileContentDoesNotDuplicateUnquotedKeyDiagnostics
#[test]
fn parse_json_source_file_config_file_content_does_not_duplicate_unquoted_key_diagnostics() {
    let parsed = get_parsed_command_line(
        "{\n  compilerOptions: {\n    strict: true\n  }\n}",
        &file_map(&[("/main.ts", "export const x = 1;")]),
        "/",
        true, /*useCaseSensitiveFileNames*/
    );

    let diags = parsed.get_config_file_parsing_diagnostics();
    assert_eq!(diags.len(), 2);
    let expected_locations: [(i32, i32); 2] = [(1, 2), (2, 4)];
    for (index, diagnostic) in diags.iter().enumerate() {
        assert_eq!(
            diagnostic.code,
            diag::String_literal_with_double_quotes_expected.code() as i32
        );
        let (line, character) =
            get_ecma_line_and_utf16_character_of_position(diagnostic.file, diagnostic.pos);
        assert_eq!(line, expected_locations[index].0);
        assert_eq!(character, expected_locations[index].1);
    }
}

// Go: tsconfigparsing_test.go:1127 TestParseJsonSourceFileConfigFileContentReportsQuestionTokenDiagnostics
#[test]
fn parse_json_source_file_config_file_content_reports_question_token_diagnostics() {
    let parsed = get_parsed_command_line(
        "{\n  compilerOptions?: {\n    strict?: true\n  }\n}",
        &file_map(&[("/main.ts", "export const x = 1;")]),
        "/",
        true, /*useCaseSensitiveFileNames*/
    );

    let mut question_token_diagnostics: Vec<Diagnostic> = Vec::new();
    for diagnostic in parsed.get_config_file_parsing_diagnostics() {
        if diagnostic.code
            == diag::The_0_modifier_can_only_be_used_in_TypeScript_files.code() as i32
        {
            question_token_diagnostics.push(diagnostic);
        }
    }
    assert_eq!(question_token_diagnostics.len(), 2);
    let expected_locations: [(i32, i32); 2] = [(1, 17), (2, 10)];
    for (index, diagnostic) in question_token_diagnostics.iter().enumerate() {
        let (line, character) =
            get_ecma_line_and_utf16_character_of_position(diagnostic.file, diagnostic.pos);
        assert_eq!(line, expected_locations[index].0);
        assert_eq!(character, expected_locations[index].1);
    }
}

// Go: tsconfigparsing_test.go:1156 TestParseNullEnumCompilerOptions
#[test]
fn parse_null_enum_compiler_options() {
    let config = TestConfig {
        json_text: "{\n\t\t\t\"compilerOptions\": {\n\t\t\t\t\"target\": null,\n\t\t\t\t\"module\": null\n\t\t\t}\n\t\t}",
        config_file_name: "tsconfig.json",
        base_path: "/",
        all_file_list: file_map(&[("/app.ts", "")]),
        existing_options: None,
    };
    // PORT: Go ranges over a map, so the subtest order is random.
    let get_parsed_functions: [(&str, GetParsed); 2] = [
        ("json api", get_parsed_with_json_api),
        ("jsonSourceFile api", get_parsed_with_json_source_file_api),
    ];
    let mut t = Subtests::new("TestParseNullEnumCompilerOptions");
    for (name, get_parsed) in get_parsed_functions {
        t.run(name, || {
            let mut all_file_lists = config.all_file_list.clone();
            all_file_lists.insert("/tsconfig.json".to_string(), config.json_text.to_string());
            let host = new_vfs_parse_config_host(
                &all_file_lists,
                config.base_path,
                true, /*useCaseSensitiveFileNames*/
            );
            let parsed_config_file_content = get_parsed(&config, &host, config.base_path);
            assert_eq!(parsed_config_file_content.errors.len(), 0);
            Ok(())
        });
    }
    t.finish();
}

// The two config APIs that the content mapper tests run, in the Go map
// order the tests list them.
// PORT: Go ranges over a map, so the subtest order is random.
const CONFIG_APIS: [(&str, GetParsed); 2] = [
    ("json api", get_parsed_with_json_api),
    ("jsonSourceFile api", get_parsed_with_json_source_file_api),
];

/// Go `RunExternalCode: core.TSTrue` as the existing options.
fn run_external_code_options() -> Option<CompilerOptions> {
    Some(CompilerOptions {
        run_external_code: Tristate::True,
        ..Default::default()
    })
}

// Go: tsconfigparsing_test.go:1187 TestContentMapperOutputExtensionsReset (ts#64581)
#[test]
fn content_mapper_output_extensions_reset() {
    struct ResetTest {
        name: &'static str,
        override_: &'static str,
        rewrite: bool,
        declaration: &'static str,
        output_extensions: Option<&'static [(&'static str, &'static str)]>,
    }
    let tests = [
        ResetTest {
            name: "omitted",
            override_: "",
            rewrite: true,
            declaration: "/dist/Card.d.ts",
            output_extensions: Some(&[(".astro", ".js")]),
        },
        ResetTest {
            name: "null",
            override_: r#", "outputExtensions": null"#,
            rewrite: false,
            declaration: "/dist/Card.d.astro.ts",
            output_extensions: None,
        },
        ResetTest {
            name: "empty object",
            override_: r#", "outputExtensions": {}"#,
            rewrite: false,
            declaration: "/dist/Card.d.astro.ts",
            output_extensions: Some(&[]),
        },
        ResetTest {
            name: "replacement",
            override_: r#", "outputExtensions": {".astro": ".mjs"}"#,
            rewrite: true,
            declaration: "/dist/Card.d.mts",
            output_extensions: Some(&[(".astro", ".mjs")]),
        },
    ];
    let mut t = Subtests::new("TestContentMapperOutputExtensionsReset");
    for test in &tests {
        for (name, get_parsed) in CONFIG_APIS {
            t.run(&format!("{}/{name}", test.name), || {
                // PORT: the parser keeps `&'static str` text, so the config text is leaked.
                let json_text: &'static str = Box::leak(
                    format!(
                        "{{\n\t\t\t\t\t\t\"compilerOptions\": {{\n\t\t\t\t\t\t\t\"declaration\": true, \"rootDir\": \"/src\", \"outDir\": \"/dist\",\n\t\t\t\t\t\t\t\"rewriteRelativeImportExtensions\": {}\n\t\t\t\t\t\t}},\n\t\t\t\t\t\t\"contentMappers\": [{{ \"package\": \"mapper\", \"extensions\": [\".astro\"]{} }}],\n\t\t\t\t\t\t\"files\": [\"/src/Card.astro\"]\n\t\t\t\t\t}}",
                        test.rewrite, test.override_
                    )
                    .into_boxed_str(),
                );
                let files = file_map(&[
                    ("/tsconfig.json", json_text),
                    ("/src/Card.astro", "export {};"),
                    (
                        "/node_modules/mapper/package.json",
                        "{\n\t\t\t\t\t\t\"name\": \"mapper\",\n\t\t\t\t\t\t\"typescript\": { \"contentMapper\": {\n\t\t\t\t\t\t\t\"exec\": [\"mapper\"], \"outputExtensions\": {\".astro\": \".js\"}\n\t\t\t\t\t\t} }\n\t\t\t\t\t}",
                    ),
                ]);
                let config = TestConfig {
                    json_text,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: files.clone(),
                    existing_options: run_external_code_options(),
                };
                let host = new_vfs_parse_config_host(
                    &files,
                    config.base_path,
                    true, /*useCaseSensitiveFileNames*/
                );
                let parsed = get_parsed(&config, &host, config.base_path);
                if !parsed.errors.is_empty() {
                    return Err(format!("unexpected errors: {:?}", parsed.errors));
                }
                assert_eq!(parsed.content_mappers().len(), 1);
                let mapper = &parsed.content_mappers()[0];
                let want: Option<BTreeMap<String, String>> = test.output_extensions.map(|entries| {
                    entries
                        .iter()
                        .map(|(source, target)| ((*source).to_string(), (*target).to_string()))
                        .collect()
                });
                assert_eq!(mapper.definition.output_extensions, want);
                assert_eq!(
                    !parsed.content_mapper_extension_rewrites().is_empty(),
                    test.rewrite
                );
                assert_eq!(
                    parsed.get_output_file_names().collect::<Vec<_>>(),
                    vec![test.declaration.to_string()]
                );
                // PORT: Go then writes into the manifest map and checks that
                // the definition map did not change (it is a clone). A Rust
                // clone is always a separate map, and the mapper is in an
                // `Rc`, so that step has no port.
                Ok(())
            });
        }
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1245 TestContentMappers (tsgo#4712)
#[test]
fn content_mappers() {
    let config = TestConfig {
        json_text: "{\n\t\t\t\"contentMappers\": [\n\t\t\t\t{ \"package\": \"vue-mapper\", \"extensions\": [\".vue\"], \"options\": { \"strictTemplates\": true } }\n\t\t\t],\n\t\t\t\"include\": [\"src\"]\n\t\t}",
        config_file_name: "tsconfig.json",
        base_path: "/",
        all_file_list: file_map(&[
            ("/src/app.ts", "export {}"),
            ("/src/Component.vue", "<template></template>"),
            (
                "/node_modules/vue-mapper/package.json",
                r#"{ "name": "vue-mapper", "version": "1.2.3", "typescript": { "contentMapper": { "exec": ["node", "./mapper.js"], "dynamicConfig": true } } }"#,
            ),
        ]),
        existing_options: run_external_code_options(),
    };
    let mut t = Subtests::new("TestContentMappers");
    for (name, get_parsed) in CONFIG_APIS {
        t.run(name, || {
            let mut all_file_lists = config.all_file_list.clone();
            all_file_lists.insert("/tsconfig.json".to_string(), config.json_text.to_string());
            let host = new_vfs_parse_config_host(
                &all_file_lists,
                config.base_path,
                true, /*useCaseSensitiveFileNames*/
            );
            let parsed = get_parsed(&config, &host, config.base_path);

            assert_eq!(parsed.errors.len(), 0);

            let mappers = parsed.content_mappers();
            assert_eq!(mappers.len(), 1);
            assert_eq!(mappers[0].definition.package, "vue-mapper");
            assert_eq!(mappers[0].definition.extensions, vec![".vue".to_string()]);
            assert_eq!(
                String::from_utf8_lossy(&mappers[0].definition.options.0),
                r#"{"strictTemplates":true}"#
            );
            assert_eq!(parsed.content_mapper_extensions(), vec![".vue".to_string()]);

            // The package.json is resolved during parsing, populating name, version, and exec.
            assert_eq!(mappers[0].manifest.name, "vue-mapper");
            assert_eq!(mappers[0].manifest.version, "1.2.3");
            assert_eq!(
                mappers[0].manifest.exec,
                vec!["node".to_string(), "./mapper.js".to_string()]
            );
            assert!(mappers[0].manifest.dynamic_config);
            assert_eq!(mappers[0].package_directory, "/node_modules/vue-mapper");

            // The .vue file is picked up by the include glob because its extension is registered.
            assert!(
                parsed
                    .file_names()
                    .contains(&"/src/Component.vue".to_string()),
                "expected /src/Component.vue in {:?}",
                parsed.file_names()
            );
            assert!(
                parsed.file_names().contains(&"/src/app.ts".to_string()),
                "expected /src/app.ts in {:?}",
                parsed.file_names()
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1300 TestContentMapperOptionDiagnosticLocation (tsgo#4712)
#[test]
fn content_mapper_option_diagnostic_location() {
    let config = TestConfig {
        json_text: "{\n\t\t\t\"contentMappers\": [{\n\t\t\t\t\"package\": \"mapper\",\n\t\t\t\t\"extensions\": [\".vue\"],\n\t\t\t\t\"options\": { \"plugins\": [{ \"name\": 1 }] }\n\t\t\t}]\n\t\t}",
        config_file_name: "tsconfig.json",
        base_path: "/",
        all_file_list: file_map(&[
            ("/index.ts", "export {};"),
            (
                "/node_modules/mapper/package.json",
                r#"{ "name": "mapper", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["mapper"] } } }"#,
            ),
        ]),
        existing_options: run_external_code_options(),
    };
    let host = new_vfs_parse_config_host(
        &config.all_file_list,
        config.base_path,
        true, /*useCaseSensitiveFileNames*/
    );
    let parsed = get_parsed_with_json_source_file_api(&config, &host, config.base_path);
    let (file, loc) = tsoptions::get_content_mapper_option_diagnostic_location(
        Some(&parsed),
        &parsed.content_mappers()[0],
        &[
            OptionPathSegment {
                property: "plugins".to_string(),
                ..Default::default()
            },
            OptionPathSegment {
                index: 0,
                is_index: true,
                ..Default::default()
            },
            OptionPathSegment {
                property: "name".to_string(),
                ..Default::default()
            },
        ],
    );
    assert_eq!(
        &source_file_text(file)[loc.pos() as usize..loc.end() as usize],
        "1"
    );
}

// Go: tsconfigparsing_test.go:1328 TestContentMappersAreInheritedFromExtendedConfig (tsgo#4712)
#[test]
fn content_mappers_are_inherited_from_extended_config() {
    let config = TestConfig {
        json_text: r#"{ "extends": "./base.json" }"#,
        config_file_name: "tsconfig.json",
        base_path: "/project",
        all_file_list: file_map(&[
            (
                "/project/base.json",
                r#"{ "contentMappers": [{ "package": "vue-mapper", "extensions": [".vue"] }], "include": ["src"] }"#,
            ),
            ("/project/src/index.ts", "export {};"),
            ("/project/src/component.vue", "<template></template>"),
            (
                "/project/node_modules/vue-mapper/package.json",
                r#"{ "name": "vue-mapper", "version": "1.2.3", "typescript": { "contentMapper": { "exec": ["node", "./mapper.js"] } } }"#,
            ),
        ]),
        existing_options: run_external_code_options(),
    };
    let mut t = Subtests::new("TestContentMappersAreInheritedFromExtendedConfig");
    for (name, get_parsed) in CONFIG_APIS {
        t.run(name, || {
            let host = new_vfs_parse_config_host(
                &config.all_file_list,
                config.base_path,
                true, /*useCaseSensitiveFileNames*/
            );
            let parsed = get_parsed(&config, &host, config.base_path);
            assert_eq!(parsed.errors.len(), 0);
            assert_eq!(parsed.content_mappers().len(), 1);
            assert_eq!(parsed.content_mappers()[0].definition.package, "vue-mapper");
            assert_eq!(parsed.content_mapper_extensions(), vec![".vue".to_string()]);
            assert!(
                parsed
                    .file_names()
                    .contains(&"/project/src/component.vue".to_string())
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1359 TestContentMappersRequireFlag (tsgo#4712)
#[test]
fn content_mappers_require_flag() {
    let config = TestConfig {
        json_text: r#"{ "contentMappers": [{ "package": "vue-mapper", "extensions": [".vue"] }] }"#,
        config_file_name: "tsconfig.json",
        base_path: "/",
        all_file_list: file_map(&[("/app.ts", "export {}")]),
        // existingOptions omitted: --runExternalCode is not set.
        existing_options: None,
    };
    let expected_code =
        diag::Content_mappers_require_the_runExternalCode_command_line_flag_to_be_enabled.code()
            as i32;
    let mut t = Subtests::new("TestContentMappersRequireFlag");
    for (name, get_parsed) in CONFIG_APIS {
        t.run(name, || {
            let mut all_file_lists = file_map(&[("/tsconfig.json", config.json_text)]);
            all_file_lists.extend(config.all_file_list.clone());
            let host = new_vfs_parse_config_host(
                &all_file_lists,
                config.base_path,
                true, /*useCaseSensitiveFileNames*/
            );
            let parsed = get_parsed(&config, &host, config.base_path);
            let found = parsed.errors.iter().any(|d| d.code == expected_code);
            assert!(
                found,
                "expected diagnostic {expected_code}, got errors: {:?}",
                parsed.errors
            );
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1388 TestUnresolvedContentMapperDoesNotRegisterExtensions (tsgo#4712)
#[test]
fn unresolved_content_mapper_does_not_register_extensions() {
    let config = TestConfig {
        json_text: r#"{ "contentMappers": [{ "package": "missing-mapper", "extensions": [".vue"] }], "include": ["src"] }"#,
        config_file_name: "tsconfig.json",
        base_path: "/",
        all_file_list: file_map(&[
            ("/src/app.ts", "export {}"),
            ("/src/Component.vue", "<template />"),
        ]),
        existing_options: run_external_code_options(),
    };
    let mut t = Subtests::new("TestUnresolvedContentMapperDoesNotRegisterExtensions");
    for (name, get_parsed) in CONFIG_APIS {
        t.run(name, || {
            let host = new_vfs_parse_config_host(&config.all_file_list, config.base_path, true);
            let parsed = get_parsed(&config, &host, config.base_path);

            assert_eq!(parsed.content_mappers().len(), 0);
            assert_eq!(parsed.content_mapper_extensions().len(), 0);
            assert!(
                !parsed
                    .file_names()
                    .contains(&"/src/Component.vue".to_string())
            );
            assert!(parsed.file_names().contains(&"/src/app.ts".to_string()));
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1415 TestContentMappersValidation (tsgo#4712, ts#64581)
#[test]
fn content_mappers_validation() {
    struct ValidationTest {
        name: &'static str,
        content_mappers: &'static str,
        expected_code: i32,
    }
    let requires_type = diag::Compiler_option_0_requires_a_value_of_type_1.code() as i32;
    let separators =
        diag::Content_mapper_extension_0_must_not_contain_path_separators.code() as i32;
    let tests = [
        ValidationTest {
            name: "extension without leading dot",
            content_mappers: r#"[{ "package": "vue-mapper", "extensions": ["vue"] }]"#,
            expected_code: diag::Content_mapper_file_extension_0_must_begin_with_a.code() as i32,
        },
        // ts#64581
        ValidationTest {
            name: "source extension contains slash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue/foo"] }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "source extension contains backslash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue\\foo"] }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "output extension key contains slash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue"], "outputExtensions": {".vue/foo": ".js"} }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "output extension key contains backslash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue"], "outputExtensions": {".vue\\foo": ".js"} }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "output extension value contains slash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue"], "outputExtensions": {".vue": ".js/foo"} }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "output extension value contains backslash",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue"], "outputExtensions": {".vue": ".js\\foo"} }]"#,
            expected_code: separators,
        },
        ValidationTest {
            name: "built-in extension",
            content_mappers: r#"[{ "package": "x", "extensions": [".ts"] }]"#,
            expected_code: diag::Content_mapper_file_extension_0_is_a_built_in_extension_and_cannot_be_registered_by_a_content_mapper.code() as i32,
        },
        ValidationTest {
            name: "missing extensions",
            content_mappers: r#"[{ "package": "x" }]"#,
            expected_code: requires_type,
        },
        ValidationTest {
            name: "duplicate extension across mappers",
            content_mappers: r#"[{ "package": "a", "extensions": [".vue"] }, { "package": "b", "extensions": [".vue"] }]"#,
            expected_code: diag::Content_mapper_file_extension_0_is_registered_by_more_than_one_content_mapper.code() as i32,
        },
        ValidationTest {
            name: "extensions is not an array",
            content_mappers: r#"[{ "package": "x", "extensions": ".vue" }]"#,
            expected_code: requires_type,
        },
        ValidationTest {
            name: "extensions contains a non-string",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue", 1] }]"#,
            expected_code: requires_type,
        },
        ValidationTest {
            name: "package is not a string",
            content_mappers: r#"[{ "package": ["x"], "extensions": [".vue"] }]"#,
            expected_code: requires_type,
        },
        ValidationTest {
            name: "missing package",
            content_mappers: r#"[{ "extensions": [".vue"] }]"#,
            expected_code: requires_type,
        },
        ValidationTest {
            name: "options is not an object",
            content_mappers: r#"[{ "package": "x", "extensions": [".vue"], "options": ["strict"] }]"#,
            expected_code: requires_type,
        },
    ];

    let mut t = Subtests::new("TestContentMappersValidation");
    for test in &tests {
        // PORT: the parser keeps `&'static str` text, so the config text is leaked.
        let json_text: &'static str = Box::leak(
            format!(r#"{{ "contentMappers": {} }}"#, test.content_mappers).into_boxed_str(),
        );
        let mut config = TestConfig {
            json_text,
            config_file_name: "tsconfig.json",
            base_path: "/",
            all_file_list: file_map(&[("/app.ts", "export {}")]),
            existing_options: run_external_code_options(),
        };
        if test.name == "duplicate extension across mappers" {
            config.all_file_list.insert(
                "/node_modules/a/package.json".to_string(),
                r#"{ "name": "a", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["a"] } } }"#.to_string(),
            );
            config.all_file_list.insert(
                "/node_modules/b/package.json".to_string(),
                r#"{ "name": "b", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["b"] } } }"#.to_string(),
            );
        }
        for (api_name, get_parsed) in CONFIG_APIS {
            t.run(&format!("{}/{api_name}", test.name), || {
                let mut all_file_lists = file_map(&[("/tsconfig.json", config.json_text)]);
                all_file_lists.extend(config.all_file_list.clone());
                let host = new_vfs_parse_config_host(
                    &all_file_lists,
                    config.base_path,
                    true, /*useCaseSensitiveFileNames*/
                );
                let parsed = get_parsed(&config, &host, config.base_path);
                let diagnostic = parsed
                    .errors
                    .iter()
                    .find(|d| d.code == test.expected_code)
                    .unwrap_or_else(|| {
                        panic!(
                            "expected diagnostic {}, got errors: {:?}",
                            test.expected_code, parsed.errors
                        )
                    });
                match test.name {
                    "built-in extension" => {
                        assert_eq!(parsed.content_mappers().len(), 0);
                        assert_eq!(parsed.content_mapper_extensions().len(), 0);
                    }
                    "duplicate extension across mappers" => {
                        assert_eq!(parsed.content_mappers().len(), 2);
                        assert_eq!(
                            parsed.content_mappers()[0].definition.extensions,
                            vec![".vue".to_string()]
                        );
                        assert_eq!(parsed.content_mappers()[1].definition.extensions.len(), 0);
                        assert_eq!(parsed.content_mapper_extensions(), vec![".vue".to_string()]);
                    }
                    "missing extensions"
                    | "extensions is not an array"
                    | "extensions contains a non-string"
                    | "package is not a string"
                    | "missing package"
                    | "options is not an object" => {
                        assert_eq!(parsed.content_mappers().len(), 0);
                    }
                    _ => {}
                }

                // With the jsonSourceFile API the diagnostic is located at the offending tsconfig syntax.
                if api_name == "jsonSourceFile api" {
                    assert!(
                        diagnostic.file.is_some(),
                        "expected diagnostic {} to have a source file",
                        test.expected_code
                    );
                    assert!(
                        diagnostic.end - diagnostic.pos > 0,
                        "expected diagnostic {} to have a non-empty location",
                        test.expected_code
                    );
                }
                Ok(())
            });
        }
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1552 TestContentMapperExtensionValidationUsesHostCaseSensitivity (ts#63936)
#[test]
fn content_mapper_extension_validation_uses_host_case_sensitivity() {
    struct CaseTest {
        name: &'static str,
        use_case_sensitive_file_names: bool,
        content_mappers: &'static str,
        expected_code: i32,
    }
    let built_in = diag::Content_mapper_file_extension_0_is_a_built_in_extension_and_cannot_be_registered_by_a_content_mapper.code() as i32;
    let tests = [
        CaseTest {
            name: "built-in extension on case-insensitive host",
            use_case_sensitive_file_names: false,
            content_mappers: r#"[{ "package": "mapper", "extensions": [".TS"] }]"#,
            expected_code: built_in,
        },
        CaseTest {
            name: "duplicate extension on case-insensitive host",
            use_case_sensitive_file_names: false,
            content_mappers: r#"[{ "package": "a", "extensions": [".vue"] }, { "package": "b", "extensions": [".VUE"] }]"#,
            expected_code:
                diag::Content_mapper_file_extension_0_is_registered_by_more_than_one_content_mapper
                    .code() as i32,
        },
        CaseTest {
            name: "built-in extension on case-sensitive host",
            use_case_sensitive_file_names: true,
            content_mappers: r#"[{ "package": "mapper", "extensions": [".TS"] }]"#,
            expected_code: built_in,
        },
        CaseTest {
            name: "mapper extension casing is distinct on case-sensitive host",
            use_case_sensitive_file_names: true,
            content_mappers: r#"[{ "package": "a", "extensions": [".vue"] }, { "package": "b", "extensions": [".VUE"] }]"#,
            expected_code: 0,
        },
    ];

    let mut t = Subtests::new("TestContentMapperExtensionValidationUsesHostCaseSensitivity");
    for test in &tests {
        t.run(test.name, || {
            // PORT: the parser keeps `&'static str` text, so the config text is leaked.
            let json_text: &'static str = Box::leak(
                format!(r#"{{ "contentMappers": {} }}"#, test.content_mappers).into_boxed_str(),
            );
            let files = file_map(&[
                ("/tsconfig.json", json_text),
                ("/app.ts", "export {};"),
                (
                    "/node_modules/mapper/package.json",
                    r#"{ "name": "mapper", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["mapper"] } } }"#,
                ),
                (
                    "/node_modules/a/package.json",
                    r#"{ "name": "a", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["a"] } } }"#,
                ),
                (
                    "/node_modules/b/package.json",
                    r#"{ "name": "b", "version": "1.0.0", "typescript": { "contentMapper": { "exec": ["b"] } } }"#,
                ),
            ]);
            let host = new_vfs_parse_config_host(&files, "/", test.use_case_sensitive_file_names);
            let config = TestConfig {
                json_text,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: files.clone(),
                existing_options: run_external_code_options(),
            };
            let parsed = get_parsed_with_json_source_file_api(&config, &host, config.base_path);
            if test.expected_code == 0 {
                if !parsed.errors.is_empty() {
                    return Err(format!("unexpected errors: {:?}", parsed.errors));
                }
            } else if !parsed.errors.iter().any(|d| d.code == test.expected_code) {
                return Err(format!(
                    "expected diagnostic {}, got errors: {:?}",
                    test.expected_code, parsed.errors
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1617 TestContentMapperCompoundExtensionValidation (ts#64581)
#[test]
fn content_mapper_compound_extension_validation() {
    let built_in = diag::Content_mapper_file_extension_0_is_a_built_in_extension_and_cannot_be_registered_by_a_content_mapper.code() as i32;
    let mut t = Subtests::new("TestContentMapperCompoundExtensionValidation");
    for use_case_sensitive_file_names in [false, true] {
        let mut extensions: Vec<String> =
            vec![".y.z".to_string(), ".component.tsx.vue".to_string()];
        for extension in ALL_SUPPORTED_EXTENSIONS_WITH_JSON
            .iter()
            .flat_map(|group| group.iter())
        {
            extensions.push(format!(".component{extension}"));
            extensions.push(format!(".component{}", extension.to_uppercase()));
        }
        for extension in &extensions {
            t.run(
                &format!("{extension}/caseSensitive={use_case_sensitive_file_names}"),
                || {
                    // PORT: the parser keeps `&'static str` text, so the config text is leaked.
                    let json_text: &'static str = Box::leak(
                        format!(
                            r#"{{ "contentMappers": [{{ "package": "mapper", "extensions": ["{extension}"] }}] }}"#
                        )
                        .into_boxed_str(),
                    );
                    let files = file_map(&[
                        ("/tsconfig.json", json_text),
                        ("/app.ts", "export {};"),
                        (
                            "/node_modules/mapper/package.json",
                            "{\n\t\t\t\t\t\t\"name\": \"mapper\",\n\t\t\t\t\t\t\"typescript\": { \"contentMapper\": { \"exec\": [\"mapper\"] } }\n\t\t\t\t\t}",
                        ),
                    ]);
                    let host = new_vfs_parse_config_host(&files, "/", use_case_sensitive_file_names);
                    let config = TestConfig {
                        json_text,
                        config_file_name: "tsconfig.json",
                        base_path: "/",
                        all_file_list: files.clone(),
                        existing_options: run_external_code_options(),
                    };
                    let parsed =
                        get_parsed_with_json_source_file_api(&config, &host, config.base_path);
                    if extension == ".y.z" || extension == ".component.tsx.vue" {
                        assert_eq!(parsed.errors.len(), 0, "{:?}", parsed.errors);
                        assert_eq!(parsed.content_mapper_extensions(), vec![extension.clone()]);
                    } else {
                        assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
                        assert_eq!(parsed.errors[0].code, built_in);
                        assert_eq!(parsed.content_mapper_extensions().len(), 0);
                    }
                    Ok(())
                },
            );
        }
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1654 getParsedWithJsonSourceFileApi
fn get_parsed_with_json_source_file_api(
    config: &TestConfig,
    host: &dyn ParseConfigHost,
    base_path: &str,
) -> ParsedCommandLine {
    let config_file_name = get_normalized_absolute_path(config.config_file_name, base_path);
    let path = to_path(
        config.config_file_name,
        base_path,
        host.fs().use_case_sensitive_file_names(),
    );
    let parsed = parse_source_file(
        &SourceFileParseOptions {
            file_name: config_file_name.clone(),
            path,
            ..Default::default()
        },
        config.json_text,
        ScriptKind::JSON,
    );
    let ts_config_source_file = ts_config_source_file(&parsed);
    tsoptions::parse_json_source_file_config_file_content(
        ts_config_source_file,
        host,
        &host.get_current_directory(),
        config.existing_options.as_ref(),
        None,
        &config_file_name,
        /*resolutionStack*/ &[],
        /*extendedConfigCache*/ None,
    )
}

/// Go `&tsoptions.TsConfigSourceFile{SourceFile: parsed}`.
// PORT: the Rust value keeps the root node, path and file name of the
// embedded Go `*ast.SourceFile`.
fn ts_config_source_file(parsed: &ParsedSourceFile) -> TsConfigSourceFile {
    TsConfigSourceFile {
        source_file: parsed.root,
        path: parsed.path().clone(),
        file_name: parsed.file_name().to_string(),
        ..Default::default()
    }
}

// Go: tsconfigparsing_test.go:1675 baselineParseConfigWith
// PORT: Go `t.Fatal` and the fatal `assert.NilError` are panics; the
// `baseline.Run` result (Go `t.Errorf`) is the returned `Err`.
fn baseline_parse_config_with(
    baseline_file_name: &str,
    include_compiler_options: bool,
    input: &[TestConfig],
    get_parsed: GetParsed,
) -> Result<(), String> {
    let mut baseline_content = String::new();
    for (i, config) in input.iter().enumerate() {
        let mut base_path = config.base_path.to_string();
        if base_path.is_empty() {
            base_path =
                get_normalized_absolute_path(&get_directory_path(config.config_file_name), "");
        }
        let config_file_name = combine_paths(&base_path, &[config.config_file_name]);
        let mut all_file_lists = config.all_file_list.clone();
        all_file_lists.insert(config_file_name.clone(), config.json_text.to_string());
        let host = new_vfs_parse_config_host(
            &all_file_lists,
            config.base_path,
            true, /*useCaseSensitiveFileNames*/
        );
        let parsed_config_file_content = get_parsed(config, &host, &base_path);

        baseline_content.push_str("Fs::\n");
        if let Err(err) = print_fs(&mut baseline_content, &*host.fs(), "/") {
            panic!("{err:?}");
        }
        baseline_content.push('\n');
        baseline_content.push_str("configFileName:: ");
        baseline_content.push_str(config.config_file_name);
        baseline_content.push('\n');
        if include_compiler_options {
            baseline_content.push_str("CompilerOptions::\n");
            if let Err(err) = marshal_indent_write(
                &mut baseline_content,
                &CompilerOptionsJson(&parsed_config_file_content.parsed_config.compiler_options),
                "",
                "  ",
            ) {
                panic!("{err}");
            }
            baseline_content.push('\n');
            baseline_content.push('\n');

            if let Some(type_acquisition) =
                &parsed_config_file_content.parsed_config.type_acquisition
            {
                baseline_content.push_str("TypeAcquisition::\n");
                if let Err(err) = marshal_indent_write(
                    &mut baseline_content,
                    &TypeAcquisitionJson(type_acquisition),
                    "",
                    "  ",
                ) {
                    panic!("{err}");
                }
                baseline_content.push('\n');
                baseline_content.push('\n');
            }
        }
        baseline_content.push_str("FileNames::\n");
        baseline_content.push_str(
            &parsed_config_file_content
                .parsed_config
                .file_names
                .join(","),
        );
        baseline_content.push('\n');
        baseline_content.push_str("Errors::\n");
        format_diagnostics_with_color_and_context(
            &mut baseline_content,
            &parsed_config_file_content.errors,
            &formatting_options("\r\n", &base_path, true),
        );
        baseline_content.push('\n');
        if i != input.len() - 1 {
            baseline_content.push('\n');
        }
    }
    baseline::run(
        baseline_file_name,
        &baseline_content,
        &baseline::Options {
            subfolder: "config/tsconfigParsing".into(),
            ..Default::default()
        },
    )
}

// Go: tsconfigparsing_test.go:1729 writeJsonReadableText
fn write_json_readable_text<T: MarshalerTo + ?Sized>(
    output: &mut String,
    input: &T,
) -> Result<(), JsonError> {
    marshal_indent_write(output, input, "", "  ")
}

// Go: tsconfigparsing_test.go:1735 TestParseTypeAcquisition (element type of `cases`)
struct TypeAcquisitionCase {
    title: &'static str,
    config_name: &'static str,
    config: &'static str,
}

// Go: tsconfigparsing_test.go:1733 TestParseTypeAcquisition
#[test]
fn parse_type_acquisition() {
    let mut t = Subtests::new("TestParseTypeAcquisition");
    for test in type_acquisition_cases() {
        let with_json_api_name = format!("{} with json api", test.title);
        let input = vec![TestConfig {
            json_text: test.config,
            config_file_name: test.config_name,
            base_path: "/apath",
            all_file_list: file_map(&[("/apath/a.ts", ""), ("/apath/b.ts", "")]),
            existing_options: None,
        }];
        t.run(&with_json_api_name, || {
            baseline_parse_config_with(
                &format!("{with_json_api_name}.js"),
                true,
                &input,
                get_parsed_with_json_api,
            )
        });
        let with_json_source_file_api_name = format!("{} with jsonSourceFile api", test.title);
        t.run(&with_json_source_file_api_name, || {
            baseline_parse_config_with(
                &format!("{with_json_source_file_api_name}.js"),
                true,
                &input,
                get_parsed_with_json_source_file_api,
            )
        });
    }
    t.finish();
}

// Go: tsconfigparsing_test.go:1829 printFS
fn print_fs(output: &mut String, files: &dyn Fs, root: &str) -> Result<(), FsError> {
    let mut walk_fn = |path: &str, d: Option<&DirEntry>, err: Option<FsError>| {
        if let Some(err) = err {
            return Err(err);
        }
        let d = d.expect("WalkDir passes an entry when there is no error");
        if d.type_().is_regular() {
            let (content, ok) = files.read_file(path);
            if !ok {
                return Err(FsError::Other(format!("failed to read file {path}")));
            }
            output.push_str(&format!("//// [{path}]\r\n{content}\r\n\r\n"));
        }
        Ok(())
    };
    // ts#64277: Go `vfs.WalkDir(files, root, ..)`.
    ts_goport::frontend::vfs::walk_dir(files, root, &mut walk_fn)
}

// Go: tsconfigparsing_test.go:1871 TestParseSrcCompiler (ts#64022)
// PORT: Go `parseSrcCompiler` (tsconfigparsing_test.go:1845) is inlined: its
// only other user is `BenchmarkParseSrcCompiler`, which is not ported.
#[test]
fn parse_src_compiler() {
    let compiler_dir = normalize_slashes(
        &baseline::test_data_path()
            .join("fixtures")
            .join("compiler")
            .to_string_lossy(),
    );
    let tsconfig_file_name = combine_paths(&compiler_dir, &["tsconfig.json"]);
    let fs = osvfs_fs();
    let host = VfsParseConfigHost {
        vfs: Rc::clone(&fs),
        current_directory: compiler_dir.clone(),
    };
    let (json_text, ok) = fs.read_file(&tsconfig_file_name);
    assert!(ok);
    let config_file = tsoptions::new_tsconfig_source_file_from_file_path(
        &tsconfig_file_name,
        to_path(
            &tsconfig_file_name,
            &compiler_dir,
            fs.use_case_sensitive_file_names(),
        ),
        &json_text,
    );
    let parsed = tsoptions::parse_json_source_file_config_file_content(
        config_file,
        &host,
        &host.get_current_directory(),
        None,
        None,
        &tsconfig_file_name,
        &[],
        None,
    );
    assert_eq!(
        parsed.errors.len(),
        0,
        "Expected no errors in parsed command line"
    );

    let opts = parsed.compiler_options();
    assert_eq!(opts.types, Some(Vec::<String>::new()));
    assert_eq!(opts.module, ModuleKind::NODE_NEXT);
    assert_eq!(opts.module_resolution, ModuleResolutionKind::NODE_NEXT);
    assert_eq!(opts.target, ScriptTarget::ES2020);
    assert_eq!(parsed.file_names().len(), 79);
    for file in [
        "checker.ts",
        "diagnosticInformationMap.generated.ts",
        "node.d.ts",
        "program.ts",
    ] {
        let want = combine_paths(&get_directory_path(&opts.config_file_path), &[file]);
        assert!(
            parsed.file_names().contains(&want),
            "parsed.FileNames() has no {want}"
        );
    }
}

// memoCache is a minimal memoizing ExtendedConfigCache used by tests to simulate
// cache hits across multiple parses of configs that extend a common base.
// Go: tsconfigparsing_test.go:1915 memoCache
// PORT: the trait takes `&self`, so the map is in a `RefCell`.
#[derive(Default)]
struct MemoCache {
    m: RefCell<FxHashMap<Path, Rc<ExtendedConfigCacheEntry>>>,
}

impl ExtendedConfigCache for MemoCache {
    // Go: tsconfigparsing_test.go:1919 (*memoCache).GetExtendedConfig
    fn get_extended_config(
        &self,
        file_name: &str,
        path: &Path,
        resolution_stack: &ResolutionStack,
        host: &dyn ParseConfigHost,
    ) -> Rc<ExtendedConfigCacheEntry> {
        let cached = self.m.borrow().get(path).cloned();
        if let Some(e) = cached {
            return e;
        }
        let e = Rc::new(parse_extended_config(
            file_name,
            path.clone(),
            resolution_stack,
            host,
            Some(self),
        ));
        self.m.borrow_mut().insert(path.clone(), Rc::clone(&e));
        e
    }
}

// TestExtendedConfigErrorsAppearOnCacheHit verifies that diagnostics produced while parsing an
// extended config are still reported when the extended config comes from the cache.
// Go: tsconfigparsing_test.go:1935 TestExtendedConfigErrorsAppearOnCacheHit
#[test]
fn extended_config_errors_appear_on_cache_hit() {
    let mut t = Subtests::new("TestExtendedConfigErrorsAppearOnCacheHit");

    t.run("single config parsed twice", || {
        let files = file_map(&[
            ("/tsconfig.json", "{\n  \"extends\": \"./base.json\"\n}"),
            // 'excludes' instead of 'exclude' triggers diagnostic
            ("/base.json", "{\n  \"excludes\": [\"**/*.ts\"]\n}"),
            ("/app.ts", "export {}"),
        ]);

        let host = new_vfs_parse_config_host(&files, "/", true /*useCaseSensitiveFileNames*/);

        let cache = MemoCache::default();
        let first = parse_config_with_cache(&host, "/tsconfig.json", &cache);
        assert!(
            !first.errors.is_empty(),
            "expected diagnostics on first parse, got 0"
        );
        let second = parse_config_with_cache(&host, "/tsconfig.json", &cache);
        assert!(
            !second.errors.is_empty(),
            "expected diagnostics on second parse (cache hit), got 0"
        );
        Ok(())
    });

    t.run("two configs share same base", || {
        let files = file_map(&[
            ("/base.json", "{\n  \"excludes\": [\"**/*.ts\"]\n}"),
            (
                "/projA/tsconfig.json",
                "{\n  \"extends\": \"../base.json\"\n}",
            ),
            (
                "/projB/tsconfig.json",
                "{\n  \"extends\": \"../base.json\"\n}",
            ),
            ("/projA/app.ts", "export {}"),
            ("/projB/app.ts", "export {}"),
        ]);

        let host = new_vfs_parse_config_host(&files, "/", true /*useCaseSensitiveFileNames*/);

        let cache = MemoCache::default();
        let first = parse_config_with_cache(&host, "/projA/tsconfig.json", &cache);
        assert!(
            !first.errors.is_empty(),
            "expected diagnostics for projA parse, got 0"
        );
        let second = parse_config_with_cache(&host, "/projB/tsconfig.json", &cache);
        assert!(
            !second.errors.is_empty(),
            "expected diagnostics for projB parse (cache hit on base), got 0"
        );
        Ok(())
    });
    t.finish();
}

// Go: tsconfigparsing_test.go:1953 and :1996 parseConfig (the closure in
// both subtests of TestExtendedConfigErrorsAppearOnCacheHit)
fn parse_config_with_cache(
    host: &VfsParseConfigHost,
    config_file_name: &str,
    cache: &dyn ExtendedConfigCache,
) -> ParsedCommandLine {
    let cfg_path = to_path(
        config_file_name,
        &host.get_current_directory(),
        host.fs().use_case_sensitive_file_names(),
    );
    let (json_text, ok) = host.fs().read_file(config_file_name);
    assert!(ok, "missing {config_file_name} in test fs");
    let parsed = parse_source_file(
        &SourceFileParseOptions {
            file_name: config_file_name.to_string(),
            path: cfg_path,
            ..Default::default()
        },
        &*Box::leak(json_text.into_boxed_str()),
        ScriptKind::JSON,
    );
    let ts_config_source_file = ts_config_source_file(&parsed);
    tsoptions::parse_json_source_file_config_file_content(
        ts_config_source_file,
        host,
        &host.get_current_directory(),
        None,
        None,
        config_file_name,
        &[],
        Some(cache),
    )
}

// Go: tsconfigparsing_test.go:2022 TestExtendedConfigConfigDirPathsAreNotCached
#[test]
fn extended_config_config_dir_paths_are_not_cached() {
    let files = file_map(&[
        (
            "/tsconfig.base.json",
            "{\n  \"compilerOptions\": {\n    \"paths\": {\n      \"@pkg/*\": [\"${configDir}/src/*\"]\n    }\n  }\n}",
        ),
        (
            "/packages/a/tsconfig.json",
            "{\n  \"extends\": \"../../tsconfig.base.json\"\n}",
        ),
        (
            "/packages/b/tsconfig.json",
            "{\n  \"extends\": \"../../tsconfig.base.json\"\n}",
        ),
        ("/packages/a/index.ts", "export {}"),
        ("/packages/b/index.ts", "export {}"),
    ]);

    let host = new_vfs_parse_config_host(&files, "/", true /*useCaseSensitiveFileNames*/);
    let cache = MemoCache::default();

    let parse_config = |config_file_name: &str| -> ParsedCommandLine {
        let (parsed, errors) = tsoptions::get_parsed_command_line_of_config_file(
            config_file_name,
            None,
            None,
            &host,
            Some(&cache),
        );
        assert!(
            errors.is_empty(),
            "unexpected errors parsing {config_file_name}: {} errors",
            errors.len()
        );
        parsed.expect("parsed command line")
    };

    parse_config("/packages/a/tsconfig.json");
    let parsed = parse_config("/packages/b/tsconfig.json");
    let paths = parsed
        .compiler_options()
        .paths
        .as_ref()
        .and_then(|paths| paths.get("@pkg/*").cloned())
        .flatten();
    assert_eq!(paths, Some(vec!["/packages/b/src/*".to_string()]));
}

// ---------------------------------------------------------------------------
// Test data, generated from the Go source (tsconfigparsing_test.go).
// ---------------------------------------------------------------------------

// Go: tsconfigparsing_test.go:39 parseConfigFileTextToJsonTests
fn parse_config_file_text_to_json_tests() -> Vec<ParseConfigFileTextToJsonTest> {
    vec![
        ParseConfigFileTextToJsonTest {
            title: "returns empty config for file with only whitespaces",
            input: &["", " "],
        },
        ParseConfigFileTextToJsonTest {
            title: "returns empty config for file with comments only",
            input: &["// Comment", "/* Comment*/"],
        },
        ParseConfigFileTextToJsonTest {
            title: "returns empty config when config is empty object",
            input: &[r"{}"],
        },
        ParseConfigFileTextToJsonTest {
            title: "returns config object without comments",
            input: &[
                r#"{ // Excluded files
            "exclude": [
                // Exclude d.ts
                "file.d.ts"
            ]
        }"#,
                r#"{
            /* Excluded
                    Files
            */
            "exclude": [
                /* multiline comments can be in the middle of a line */"file.d.ts"
            ]
        }"#,
            ],
        },
        ParseConfigFileTextToJsonTest {
            title: "keeps string content untouched",
            input: &[
                r#"{
            "exclude": [
                "xx//file.d.ts"
            ]
        }"#,
                r#"{
            "exclude": [
                "xx/*file.d.ts*/"
            ]
        }"#,
            ],
        },
        ParseConfigFileTextToJsonTest {
            title: "handles escaped characters in strings correctly",
            input: &[
                r#"{
            "exclude": [
                "xx\"//files"
            ]
        }"#,
                r#"{
            "exclude": [
                "xx\\" // end of line comment
            ]
        }"#,
            ],
        },
        ParseConfigFileTextToJsonTest {
            title: "returns object when users correctly specify library",
            input: &[
                r#"{
            "compilerOptions": {
                "lib": ["es5"]
            }
        }"#,
                r#"{
            "compilerOptions": {
                "lib": ["es5", "es6"]
            }
        }"#,
            ],
        },
    ]
}

// Go: tsconfigparsing_test.go:165 parseJsonConfigFileTests
fn parse_json_config_file_tests() -> Vec<ParseJsonConfigTestCase> {
    vec![
        ParseJsonConfigTestCase {
            title: "ignore dotted files and folders",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r"{}",
                config_file_name: "tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[
                    ("/apath/test.ts", ""),
                    ("/apath/.git/a.ts", ""),
                    ("/apath/.b.ts", ""),
                    ("/apath/..c.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "allow dotted files and folders when explicitly requested",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                    "files": ["/apath/.git/a.ts", "/apath/.b.ts", "/apath/..c.ts"]
                }"#,
                config_file_name: "tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[
                    ("/apath/test.ts", ""),
                    ("/apath/.git/a.ts", ""),
                    ("/apath/.b.ts", ""),
                    ("/apath/..c.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "implicitly exclude common package folders",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r"{}",
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    ("/node_modules/a.ts", ""),
                    ("/bower_components/b.ts", ""),
                    ("/jspm_packages/c.ts", ""),
                    ("/d.ts", ""),
                    ("/folder/e.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for empty files list",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "files": []
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        // ts#64159
        ParseJsonConfigTestCase {
            title: "handles empty file name in files list",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "files": [""]
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for empty files list when no references are provided",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "files": [],
                "references": []
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for directory with no .ts files",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r"{
            }",
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.js", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for empty include",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "include": []
            }"#,
                config_file_name: "/apath/tsconfig.json",
                // ts#64159: a rooted base path.
                base_path: "/tests/cases/unittests",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for include with parent directory after recursive wildcard",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "include": ["**/../*.ts"]
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/main.ts", "")]),
                existing_options: None,
            }],
        },
        // Go: tsconfigparsing_test.go:261 (ts#64832)
        ParseJsonConfigTestCase {
            title: "reports errors for network paths",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
                "extends": ["//server/share/base.json", "\\\\?\\UNC\\server\\share\\base.json"],
                "compilerOptions": {
                    "outDir": "\\\\server\\share\\out",
                    "rootDir": "./src",
                    "typeRoots": ["./types", "\\??\\UNC\\server\\share\\types"],
                    "mapRoot": "//server/share/maps",
                    "declarationDir": "/x/../??/UNC/server/share/decl",
                    "baseUrl": "/??/../apath"
                },
                "files": ["main.ts", "//server/share/a.ts", "../??/UNC/server/share/b.ts"],
                "include": ["src", "\\\\.\\GLOBALROOT\\Device\\Mup\\server\\share"],
                "exclude": ["//server/share/excluded", "../??/*.ts"],
                "references": [{ "path": "//server/share/project" }]
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/main.ts", ""), ("/apath/src/index.ts", "")]),
                existing_options: None,
            }],
        },
        // Go: tsconfigparsing_test.go:285 (ts#64832)
        ParseJsonConfigTestCase {
            title: "reports errors for network path in string extends",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{ "extends": "\\\\server\\share\\base.json" }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/main.ts", "")]),
                existing_options: None,
            }],
        },
        // Go: tsconfigparsing_test.go:294 (ts#64832)
        ParseJsonConfigTestCase {
            title: "reports errors for normalized network path in string extends",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{ "extends": "/x/../??/UNC/server/share/base.json" }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/main.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "parses tsconfig with compilerOptions, files, include, and exclude",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "compilerOptions": {
    "outDir": "./dist",
    "strict": true,
    "noImplicitAny": true,
    "target": "ES2017",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "moduleDetection": "auto",
    "jsx": "react",
	"maxNodeModuleJsDepth": 1,
	"paths": {
      "jquery": ["./vendor/jquery/dist/jquery"]
    }
  },
  "files": ["/apath/src/index.ts", "/apath/src/app.ts"],
  "include": ["/apath/src/**/*"],
  "exclude": ["/apath/node_modules", "/apath/dist"]
}"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[
                    ("/apath/src/index.ts", ""),
                    ("/apath/src/app.ts", ""),
                    ("/apath/node_modules/module.ts", ""),
                    ("/apath/dist/output.js", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors when commandline option is in tsconfig",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
  "compilerOptions": {
    "help": true
  }
}"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "does not generate errors for empty files list when one or more references are provided",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "files": [],
                "references": [{ "path": "/apath" }]
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "exclude outDir unless overridden",
            include_compiler_options: false,
            input: vec![
                TestConfig {
                    json_text: r#"{
                "compilerOptions": {
                    "outDir": "bin"
                }
            }"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[("/bin/a.ts", ""), ("/b.ts", "")]),
                    existing_options: None,
                },
                TestConfig {
                    json_text: r#"{
                "compilerOptions": {
                    "outDir": "bin"
                },
                "exclude": [ "obj" ]
            }"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[("/bin/a.ts", ""), ("/b.ts", "")]),
                    existing_options: None,
                },
            ],
        },
        ParseJsonConfigTestCase {
            title: "exclude declarationDir unless overridden",
            include_compiler_options: false,
            input: vec![
                TestConfig {
                    json_text: r#"{
                "compilerOptions": {
                    "declarationDir": "declarations"
                }
            }"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[("/declarations/a.d.ts", ""), ("/a.ts", "")]),
                    existing_options: None,
                },
                TestConfig {
                    json_text: r#"{
                "compilerOptions": {
                    "declarationDir": "declarations"
                },
                "exclude": [ "types" ]
            }"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[("/declarations/a.d.ts", ""), ("/a.ts", "")]),
                    existing_options: None,
                },
            ],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for empty directory",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "compilerOptions": {
                    "allowJs": true
                }
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors for includes with outDir",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                "compilerOptions": {
                    "outDir": "./"
                },
                "include": ["**/*"]
            }"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors when include is not string",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
  "include": [
    [
      "./**/*.ts"
    ]
  ]
}"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "generates errors when files is not string",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
  "files": [
    {
      "compilerOptions": {
        "experimentalDecorators": true,
        "allowJs": true
      }
    }
  ]
}"#,
                config_file_name: "/apath/tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/a.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "with outDir from base tsconfig",
            include_compiler_options: false,
            input: vec![
                TestConfig {
                    json_text: r#"{
  "extends": "./tsconfigWithoutConfigDir.json"
}"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[
                        (
                            "/tsconfigWithoutConfigDir.json",
                            TSCONFIG_WITHOUT_CONFIG_DIR,
                        ),
                        ("/bin/a.ts", ""),
                        ("/b.ts", ""),
                    ]),
                    existing_options: None,
                },
                TestConfig {
                    json_text: r#"{
  "extends": "./tsconfigWithConfigDir.json"
}"#,
                    config_file_name: "tsconfig.json",
                    base_path: "/",
                    all_file_list: file_map(&[
                        ("/tsconfigWithConfigDir.json", TSCONFIG_WITH_CONFIG_DIR),
                        ("/bin/a.ts", ""),
                        ("/b.ts", ""),
                    ]),
                    existing_options: None,
                },
            ],
        },
        ParseJsonConfigTestCase {
            title: "returns error when tsconfig have excludes",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
                    "compilerOptions": {
                        "lib": ["es5"]
                    },
                    "excludes": [
                        "foge.ts"
                    ]
                }"#,
                config_file_name: "tsconfig.json",
                base_path: "/apath",
                all_file_list: file_map(&[("/apath/test.ts", ""), ("/apath/foge.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "parses tsconfig with extends, files, include and other options",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
				"extends": "./tsconfigWithExtends.json",
				"compilerOptions": {
				    "outDir": "./dist",
    				"strict": true,
    				"noImplicitAny": true,
					"baseUrl": "",
				},
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    ("/tsconfigWithExtends.json", TSCONFIG_WITH_EXTENDS),
                    ("/src/index.ts", ""),
                    ("/src/app.ts", ""),
                    ("/node_modules/module.ts", ""),
                    ("/dist/output.js", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "parses tsconfig with extends and configDir",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
				"extends": "./tsconfig.base.json"
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    ("/tsconfig.base.json", TSCONFIG_WITH_EXTENDS_AND_CONFIG_DIR),
                    ("/src/index.ts", ""),
                    ("/src/app.ts", ""),
                    ("/node_modules/module.ts", ""),
                    ("/dist/output.js", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "reports error for an unknown option",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
			    "compilerOptions": {
				"unknown": true
			    }
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[("/app.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "reports spelling suggestion for an unknown option",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
			    "compilerOptions": {
				"targt": 1
			    }
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[("/app.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "reports errors for wrong type option and invalid enum value",
            include_compiler_options: false,
            input: vec![TestConfig {
                json_text: r#"{
			    "compilerOptions": {
				"target": "invalid value",
				"removeComments": "should be a boolean",
				"moduleResolution": "invalid value"
			    }
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[("/app.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "reports errors for incorrectly cased option names",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
			    "compilerOptions": {
				"sourcemap": true,
				"declarationmap": true,
				"nouncheckedindexedaccess": true,
				"exactoptionalpropertytypes": true,
				"verbatimmodulesyntax": true,
				"isolatedmodules": true,
				"nouncheckedsideeffectimports": true,
				"moduledetection": "force",
				"skiplibcheck": true,
				"checkjs": true
			    }
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[("/app.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "handles empty types array",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
			    "compilerOptions": {
					"types": []
				}
			}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[("/app.ts", "")]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "issue 1267 scenario - extended files not picked up",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-base/backend.json",
  "compilerOptions": {
    "baseUrl": "./",
    "outDir": "dist",
    "rootDir": "src",
    "resolveJsonModule": true
  },
  "exclude": ["node_modules", "dist"],
  "include": ["src/**/*"]
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-base/backend.json",
                        r#"{
  "$schema": "https://json.schemastore.org/tsconfig",
  "display": "Backend",
  "compilerOptions": {
    "allowJs": true,
    "module": "nodenext",
    "removeComments": true,
    "emitDecoratorMetadata": true,
    "experimentalDecorators": true,
    "allowSyntheticDefaultImports": true,
    "target": "esnext",
    "lib": ["ESNext"],
    "incremental": false,
    "esModuleInterop": true,
    "noImplicitAny": true,
    "moduleResolution": "nodenext",
    "types": ["node", "vitest/globals"],
    "sourceMap": true,
    "strictPropertyInitialization": false
  },
  "files": [
    "types/ical2json.d.ts",
    "types/express.d.ts",
    "types/multer.d.ts",
    "types/reset.d.ts",
    "types/stripe-custom-typings.d.ts",
    "types/nestjs-modules.d.ts",
    "types/luxon.d.ts",
    "types/nestjs-pino.d.ts"
  ],
  "ts-node": {
    "files": true
  }
}"#,
                    ),
                    ("/tsconfig-base/types/ical2json.d.ts", "export {}"),
                    ("/tsconfig-base/types/express.d.ts", "export {}"),
                    ("/tsconfig-base/types/multer.d.ts", "export {}"),
                    ("/tsconfig-base/types/reset.d.ts", "export {}"),
                    (
                        "/tsconfig-base/types/stripe-custom-typings.d.ts",
                        "export {}",
                    ),
                    ("/tsconfig-base/types/nestjs-modules.d.ts", "export {}"),
                    (
                        "/tsconfig-base/types/luxon.d.ts",
                        r"declare module 'luxon' {
  interface TSSettings {
    throwOnInvalid: true
  }
}
export {}",
                    ),
                    ("/tsconfig-base/types/nestjs-pino.d.ts", "export {}"),
                    ("/src/main.ts", "export {}"),
                    ("/src/utils.ts", "export {}"),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "null overrides in extended tsconfig - array fields",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-base.json",
  "compilerOptions": {
    "types": null,
    "lib": null,
    "typeRoots": null
  }
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-base.json",
                        r#"{
  "compilerOptions": {
    "types": ["node", "@types/jest"],
    "lib": ["es2020", "dom"],
    "typeRoots": ["./types", "./node_modules/@types"]
  }
}"#,
                    ),
                    ("/app.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "null overrides in extended tsconfig - string fields",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-base.json",
  "compilerOptions": {
    "outDir": null,
    "baseUrl": null,
    "rootDir": null
  }
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-base.json",
                        r#"{
  "compilerOptions": {
    "outDir": "./dist",
    "baseUrl": "./src",
    "rootDir": "./src"
  }
}"#,
                    ),
                    ("/app.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "null overrides in extended tsconfig - mixed field types",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-base.json",
  "compilerOptions": {
    "types": null,
    "outDir": null,
    "strict": false,
    "lib": ["es2022"],
    "allowJs": null
  }
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-base.json",
                        r#"{
  "compilerOptions": {
    "types": ["node"],
    "lib": ["es2020", "dom"],
    "outDir": "./dist",
    "strict": true,
    "allowJs": true,
    "target": "es2020"
  }
}"#,
                    ),
                    ("/app.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "null overrides with multiple extends levels",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-middle.json",
  "compilerOptions": {
    "types": null,
    "lib": null
  }
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-middle.json",
                        r#"{
  "extends": "./tsconfig-base.json",
  "compilerOptions": {
    "types": ["jest"],
    "outDir": "./build"
  }
}"#,
                    ),
                    (
                        "/tsconfig-base.json",
                        r#"{
  "compilerOptions": {
    "types": ["node"],
    "lib": ["es2020"],
    "outDir": "./dist",
    "strict": true
  }
}"#,
                    ),
                    ("/app.ts", ""),
                ]),
                existing_options: None,
            }],
        },
        ParseJsonConfigTestCase {
            title: "null overrides in middle level of extends chain",
            include_compiler_options: true,
            input: vec![TestConfig {
                json_text: r#"{
  "extends": "./tsconfig-middle.json",
  "compilerOptions": {
    "outDir": "./final"
  }
}"#,
                config_file_name: "tsconfig.json",
                base_path: "/",
                all_file_list: file_map(&[
                    (
                        "/tsconfig-middle.json",
                        r#"{
  "extends": "./tsconfig-base.json",
  "compilerOptions": {
    "types": null,
    "lib": null,
    "outDir": "./middle"
  }
}"#,
                    ),
                    (
                        "/tsconfig-base.json",
                        r#"{
  "compilerOptions": {
    "types": ["node"],
    "lib": ["es2020"],
    "outDir": "./base",
    "strict": true
  }
}"#,
                    ),
                    ("/app.ts", ""),
                ]),
                existing_options: None,
            }],
        },
    ]
}

// Go: tsconfigparsing_test.go:836 tsconfigWithExtends
const TSCONFIG_WITH_EXTENDS: &str = r#"{
  "files": ["/src/index.ts", "/src/app.ts"],
  "include": ["/src/**/*"],
  "exclude": [],
  "ts-node": {
    "compilerOptions": {
      "module": "commonjs"
    },
    "transpileOnly": true
  }
}"#;

// Go: tsconfigparsing_test.go:848 tsconfigWithoutConfigDir
const TSCONFIG_WITHOUT_CONFIG_DIR: &str = r#"{
  "compilerOptions": {
    "outDir": "bin"
  }
}"#;

// Go: tsconfigparsing_test.go:854 tsconfigWithConfigDir
const TSCONFIG_WITH_CONFIG_DIR: &str = r#"{
  "compilerOptions": {
    "outDir": "${configDir}/bin"
  }
}"#;

// Go: tsconfigparsing_test.go:860 tsconfigWithExtendsAndConfigDir
const TSCONFIG_WITH_EXTENDS_AND_CONFIG_DIR: &str = r#"{
  "compilerOptions": {
    "outFile": "${configDir}/outFile",
    "outDir": "${configDir}/outDir",
    "rootDir": "${configDir}/rootDir",
    "tsBuildInfoFile": "${configDir}/tsBuildInfoFile",
    "baseUrl": "${configDir}/baseUrl",
    "declarationDir": "${configDir}/declarationDir",
  }
}"#;

// Go: tsconfigparsing_test.go:1735 TestParseTypeAcquisition (cases)
fn type_acquisition_cases() -> Vec<TypeAcquisitionCase> {
    vec![
        TypeAcquisitionCase {
            title: "Convert correctly format tsconfig.json to typeAcquisition ",
            config_name: "tsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enable": true,
		"include": ["0.d.ts", "1.d.ts"],
		"exclude": ["0.js", "1.js"],
	},
}"#,
        },
        TypeAcquisitionCase {
            title: "Convert incorrect format tsconfig.json to typeAcquisition ",
            config_name: "tsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enableAutoDiscovy": true,
	}
}"#,
        },
        TypeAcquisitionCase {
            title: "Convert default tsconfig.json to typeAcquisition ",
            config_name: "tsconfig.json",
            config: r"{}",
        },
        TypeAcquisitionCase {
            title: "Convert tsconfig.json with only enable property to typeAcquisition ",
            config_name: "tsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enable": true,
	},
}"#,
        },
        TypeAcquisitionCase {
            title: "Convert jsconfig.json to typeAcquisition ",
            config_name: "jsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enable": false,
		"include": ["0.d.ts"],
		"exclude": ["0.js"],
	},
}"#,
        },
        TypeAcquisitionCase {
            title: "Convert default jsconfig.json to typeAcquisition ",
            config_name: "jsconfig.json",
            config: r"{}",
        },
        TypeAcquisitionCase {
            title: "Convert incorrect format jsconfig.json to typeAcquisition ",
            config_name: "jsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enableAutoDiscovy": true,
	},
}"#,
        },
        TypeAcquisitionCase {
            title: "Convert jsconfig.json with only enable property to typeAcquisition ",
            config_name: "jsconfig.json",
            config: r#"{
	"typeAcquisition": {
		"enable": false,
	},
}"#,
        },
    ]
}
