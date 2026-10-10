//! Rust port of `internal/tsoptions/decls_test.go`,
//! `internal/tsoptions/parsinghelpers_test.go` and
//! `internal/tsoptions/wildcarddirectories_test.go`.
//!
//! PORT: the Go tests read the fields and `json` tags of
//! `core.CompilerOptions` by reflection. The port reads the field list from
//! `compiler_options_field_values` (the production stand-in for that
//! reflection: exported fields in Go order) and the tags from
//! `COMPILER_OPTIONS_JSON_FIELDS` (copied from the pinned Go struct).

use std::collections::{BTreeMap, HashMap};

use ts_goport::frontend::prelude::*;

use super::tsoptionstest::{COMPILER_OPTIONS_JSON_FIELDS, Subtests};

/// Go `field.Tag.Get("json")` for a `core.CompilerOptions` field. Go returns
/// "" for a field without the tag.
fn compiler_options_json_tag(field_name: &str) -> String {
    COMPILER_OPTIONS_JSON_FIELDS
        .iter()
        .find(|(name, _)| *name == field_name)
        .map(|(_, tag)| format!("{tag},omitzero"))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// decls_test.go
// ---------------------------------------------------------------------------

// Go: decls_test.go:12 TestCompilerOptionsDeclaration
// PORT: Go `t.Errorf` failures are collected and reported together at the
// end. Go ranges over a map for the leftover declarations; they are sorted
// here.
#[test]
fn compiler_options_declaration() {
    let mut errors: Vec<String> = Vec::new();

    let mut decls: BTreeMap<String, &'static CommandLineOption> = BTreeMap::new();

    for decl in OPTIONS_DECLARATIONS.iter() {
        decls.insert(decl.name.to_lowercase(), *decl);
    }

    let internal_options = [
        "allowNonTsExtensions",
        "build",
        "configFilePath",
        "noDtsResolution",
        "noEmitForJsFiles",
        "pathsBasePath",
        "suppressOutputPathCheck",
        "build",
    ];

    let mut internal_options_map: HashMap<String, &str> = HashMap::new();
    for opt in internal_options {
        internal_options_map.insert(opt.to_lowercase(), opt);
    }

    for (field_name, _) in compiler_options_field_values(&CompilerOptions::default()) {
        let lower_name = field_name.to_lowercase();

        let Some(decl) = decls.get(&lower_name).copied() else {
            if let Some(name) = internal_options_map.get(&lower_name) {
                check_compiler_option_json_tag_name(&mut errors, field_name, name);
                continue;
            }
            errors.push(format!(
                "CompilerOptions.{field_name} has no options declaration"
            ));
            continue;
        };
        decls.remove(&lower_name);

        check_compiler_option_json_tag_name(&mut errors, field_name, decl.name);
    }

    let skipped_options = ["plugins"];

    for opt in skipped_options {
        decls.remove(&opt.to_lowercase());
    }

    for decl in decls.values() {
        errors.push(format!(
            "Option declaration {} is not present in CompilerOptions",
            decl.name
        ));
    }

    assert!(
        errors.is_empty(),
        "TestCompilerOptionsDeclaration: {} error(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

// Go: decls_test.go:72 checkCompilerOptionJsonTagName
fn check_compiler_option_json_tag_name(errors: &mut Vec<String>, field_name: &str, name: &str) {
    let want = format!("{name},omitzero");
    let got = compiler_options_json_tag(field_name);
    if got != want {
        errors.push(format!(
            "Field {field_name} has json tag {got}, but the option declaration has name {name}"
        ));
    }
}

// ---------------------------------------------------------------------------
// parsinghelpers_test.go
// ---------------------------------------------------------------------------

// Go: parsinghelpers_test.go:11 TestParseCompilerOptionNoMissingFields
// PORT: Go calls the unexported `parseCompilerOptions(key, zero value, &co)`
// and reads its `foundKey` result. The Rust worker that returns `found`
// (`parse_compiler_options_worker`, src/frontend/tsoptions/parsing_helpers.rs:226)
// is private, and the public `parse_compiler_options` returns no such flag
// (and returns early for a nil value). So the port passes a non-zero value
// of the field's type to `parse_compiler_options` and counts the key as
// found when the options change. That checks the same thing: the switch
// has a case for every field key.
#[test]
fn parse_compiler_option_no_missing_fields() {
    let mut missing_keys: Vec<String> = Vec::new();
    for (field_name, zero_value) in compiler_options_field_values(&CompilerOptions::default()) {
        let mut key_name = field_name.to_string();
        // use the JSON key from the tag, if present
        // e.g. `json:"dog[,anythingelse]"` --> dog
        let json_tag = compiler_options_json_tag(field_name);
        if !json_tag.is_empty() {
            key_name = json_tag.split(',').next().unwrap_or_default().to_string();
        }
        let val = non_zero_value_like(&zero_value);
        let mut co = CompilerOptions::default();
        parse_compiler_options(&key_name, val, &mut co);
        let found = co != CompilerOptions::default();
        if !found {
            missing_keys.push(key_name);
        }
    }
    assert!(
        missing_keys.is_empty(),
        "The following keys are missing entries in the ParseCompilerOptions switch statement:\n[{}]",
        missing_keys.join(" ")
    );
}

/// A non-zero value of the same Go type as `zero`, in the form that the
/// tsoptions parsers read (JSON values: bool, float64, string, []any,
/// map[string]any).
fn non_zero_value_like(zero: &CompilerOptionsValue) -> CompilerOptionsValue {
    use CompilerOptionsValue as V;
    match zero {
        V::Tristate(_) => V::Bool(true),
        V::String(_) => V::String("x".to_string()),
        V::StringList(_) => V::List(vec![V::String("x".to_string())]),
        V::IntPtr(_) => V::Int(1),
        V::Paths(_) => {
            let mut paths: IndexMap<String, CompilerOptionsValue> = IndexMap::default();
            paths.insert("x".to_string(), V::List(vec![V::String("y".to_string())]));
            V::Map(paths)
        }
        // ts#64397: `Plugins []PluginImport`, read from `[{"name": "x"}]`.
        V::List(_) => V::List(vec![V::Map(IndexMap::from_iter([(
            "name".to_string(),
            V::String("x".to_string()),
        )]))]),
        V::ScriptTarget(_)
        | V::ModuleKind(_)
        | V::ModuleResolutionKind(_)
        | V::ModuleDetectionKind(_)
        | V::JsxEmit(_)
        | V::NewLineKind(_) => V::Number(1.0),
        other => panic!("unexpected CompilerOptions field value {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// wildcarddirectories_test.go
// ---------------------------------------------------------------------------

// Go: wildcarddirectories_test.go:10 TestGetWildcardDirectories_DotPrefixedIncludeWithDotDirExclude
#[test]
fn get_wildcard_directories_dot_prefixed_include_with_dot_dir_exclude() {
    // https://github.com/microsoft/typescript-go/issues/3733
    // "./"-prefixed include specs must be fully normalized before being tested
    // against exclude patterns; otherwise the leftover literal "." path segment
    // matches dot-directory excludes like "**/.*/", silently dropping every
    // wildcard directory (and with them, root file watching for the config).
    let include: Vec<String> = ["./app/**/*.ts", "./app/**/*.tsx"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let exclude: Vec<String> = ["**/node_modules", "**/.*/", "./build"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let result = get_wildcard_directories(
        &include,
        &exclude,
        &ComparePathsOptions {
            current_directory: "/home/projects/monorepo/apps/web".to_string(),
            use_case_sensitive_file_names: true,
        },
    );
    let expected: HashMap<String, bool> =
        HashMap::from([("/home/projects/monorepo/apps/web/app".to_string(), true)]);
    assert_eq!(result.into_iter().collect::<HashMap<_, _>>(), expected);
}

// Go: wildcarddirectories_test.go:27 TestGetWildcardDirectories_DriveRoot (ts#64159)
#[test]
fn get_wildcard_directories_drive_root() {
    let result = get_wildcard_directories(
        &["*.ts".to_string()],
        &[],
        &ComparePathsOptions {
            current_directory: "c:/".to_string(),
            use_case_sensitive_file_names: false,
        },
    );
    let expected: HashMap<String, bool> = HashMap::from([("c:/".to_string(), false)]);
    assert_eq!(result.into_iter().collect::<HashMap<_, _>>(), expected);
}

// Go: wildcarddirectories_test.go:39 TestGetWildcardDirectories_NonASCIICharacters
#[test]
fn get_wildcard_directories_non_ascii_characters() {
    struct Test {
        name: &'static str,
        include: &'static [&'static str],
        exclude: &'static [&'static str],
        current_directory: &'static str,
        use_case_sensitive_file_names: bool,
    }

    let tests = [
        Test {
            name: "Norwegian character æ in path",
            include: &["src/**/*.test.ts", "src/**/*.stories.ts", "src/**/*.mdx"],
            exclude: &["node_modules"],
            current_directory: "C:/Users/TobiasLægreid/dev/app/frontend/packages/react",
            use_case_sensitive_file_names: false,
        },
        Test {
            name: "Japanese characters in path",
            include: &["src/**/*.ts"],
            exclude: &["テスト"],
            current_directory: "/Users/ユーザー/プロジェクト",
            use_case_sensitive_file_names: true,
        },
        Test {
            name: "Chinese characters in path",
            include: &["源代码/**/*.js"],
            exclude: &["节点模块"],
            current_directory: "/home/用户/项目",
            use_case_sensitive_file_names: true,
        },
        Test {
            name: "Various Unicode characters",
            include: &["src/**/*.ts"],
            exclude: &["node_modules"],
            current_directory: "/Users/Müller/café/naïve/résumé",
            use_case_sensitive_file_names: false,
        },
    ];

    let mut t = Subtests::new("TestGetWildcardDirectories_NonASCIICharacters");
    for tt in &tests {
        t.run(tt.name, || {
            let compare_paths_options = ComparePathsOptions {
                current_directory: tt.current_directory.to_string(),
                use_case_sensitive_file_names: tt.use_case_sensitive_file_names,
            };

            let include: Vec<String> = tt.include.iter().map(|s| (*s).to_string()).collect();
            let exclude: Vec<String> = tt.exclude.iter().map(|s| (*s).to_string()).collect();
            let result = get_wildcard_directories(&include, &exclude, &compare_paths_options);

            // PORT: Go fails on a nil map. The Rust function returns a map
            // value (never nil), so the check is that the call returns.
            let _ = result;
            Ok(())
        });
    }
    t.finish();
}
