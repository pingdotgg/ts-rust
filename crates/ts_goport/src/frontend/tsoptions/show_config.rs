//! Port of tsoptions/showconfig.go (`tsc --showConfig`).

use crate::frontend::prelude::*;

use crate::execute::incremental::build_info::marshal_any;
use crate::execute::incremental::snapshot_to_build_info::is_zero_compiler_option_value;

// Go: tsoptions/showconfig.go:11 computeFn
// computeFn wraps a typed getter method so it can be stored in an
// impliedOption's compute field (which has type func(*core.CompilerOptions) any).
// PORT: Go `any` is `CompilerOptionsValue`. The table below uses
// non-capturing closures that wrap each getter result in its variant.
type ShowConfigComputeFn = fn(&CompilerOptions) -> CompilerOptionsValue;

// Go: tsoptions/showconfig.go:19 impliedOption
// impliedOption describes a compiler option whose effective value can be derived from
// other options. This mirrors TypeScript's computedOptions concept used in convertToTSConfig.
struct ImpliedOption {
    // name is the Go struct field name of the CompilerOptions field (e.g., "Module").
    name: &'static str,
    // dependencies lists the Go struct field names that this option depends on.
    dependencies: &'static [&'static str],
    // compute returns the effective value of this option given compiler options.
    compute: ShowConfigComputeFn,
}

// Go: tsoptions/showconfig.go:31 impliedOptions
// impliedOptions lists the compiler options that may be implied by other options,
// mirroring TypeScript's computedOptions used in convertToTSConfig.
// Each compute function delegates directly to an existing core.CompilerOptions getter.
static IMPLIED_OPTIONS: [ImpliedOption; 14] = [
    ImpliedOption {
        name: "Module",
        dependencies: &["Target"],
        compute: |o| CompilerOptionsValue::ModuleKind(o.get_emit_module_kind()),
    },
    ImpliedOption {
        name: "ModuleResolution",
        dependencies: &["Module", "Target"],
        compute: |o| CompilerOptionsValue::ModuleResolutionKind(o.get_module_resolution_kind()),
    },
    ImpliedOption {
        name: "ModuleDetection",
        dependencies: &["Module", "Target"],
        compute: |o| CompilerOptionsValue::ModuleDetectionKind(o.get_emit_module_detection_kind()),
    },
    ImpliedOption {
        name: "IsolatedModules",
        dependencies: &["VerbatimModuleSyntax"],
        compute: |o| CompilerOptionsValue::Bool(o.get_isolated_modules()),
    },
    ImpliedOption {
        name: "PreserveConstEnums",
        dependencies: &["IsolatedModules", "VerbatimModuleSyntax"],
        compute: |o| CompilerOptionsValue::Bool(o.should_preserve_const_enums()),
    },
    ImpliedOption {
        name: "Declaration",
        dependencies: &["Composite"],
        compute: |o| CompilerOptionsValue::Bool(o.get_emit_declarations()),
    },
    ImpliedOption {
        name: "DeclarationMap",
        dependencies: &["Declaration", "Composite"],
        compute: |o| CompilerOptionsValue::Bool(o.get_are_declaration_maps_enabled()),
    },
    ImpliedOption {
        name: "Incremental",
        dependencies: &["Composite"],
        compute: |o| CompilerOptionsValue::Bool(o.is_incremental()),
    },
    ImpliedOption {
        name: "UseDefineForClassFields",
        dependencies: &["Target", "Module"],
        compute: |o| CompilerOptionsValue::Bool(o.get_use_define_for_class_fields()),
    },
    ImpliedOption {
        name: "ResolvePackageJsonExports",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: |o| CompilerOptionsValue::Bool(o.get_resolve_package_json_exports()),
    },
    ImpliedOption {
        name: "ResolvePackageJsonImports",
        dependencies: &[
            "ModuleResolution",
            "ResolvePackageJsonExports",
            "Module",
            "Target",
        ],
        compute: |o| CompilerOptionsValue::Bool(o.get_resolve_package_json_imports()),
    },
    ImpliedOption {
        name: "ResolveJsonModule",
        dependencies: &["ModuleResolution", "Module", "Target"],
        compute: |o| CompilerOptionsValue::Bool(o.get_resolve_json_module()),
    },
    ImpliedOption {
        name: "AllowJs",
        dependencies: &["CheckJs"],
        compute: |o| CompilerOptionsValue::Bool(o.get_allow_js()),
    },
    ImpliedOption {
        name: "AllowImportingTsExtensions",
        dependencies: &["RewriteRelativeImportExtensions"],
        compute: |o| CompilerOptionsValue::Bool(o.get_allow_importing_ts_extensions()),
    },
];

/// TSConfig represents the output structure for --showConfig
// Go: tsoptions/showconfig.go:49 TSConfig
// PORT: Go `CompilerOptions *collections.OrderedMap[string, any]` is never
// nil here, so it is a plain `IndexMap`. Go `References []any` holds
// `*collections.OrderedMap[string, any]` values (`CompilerOptionsValue::Map`).
// The Go `[]string` fields are `omitzero`, which omits only a nil slice.
// `ConvertToTSConfig` never sets a non-nil empty slice: it sets `References`,
// `Files` and `Include` only when they are non-empty, and `Exclude` is
// `configFileSpecs.validatedExcludeSpecs`, which `validateSpecs`
// (tsconfigparsing.go:1398) builds by `append` from a nil slice and
// `getSubstitutedStringArrayWithConfigDirTemplate` (tsconfigparsing.go:1538)
// replaces only with a non-nil copy of a non-empty list. So a `Vec` is
// empty exactly when the Go slice is nil. `CompileOnSave *bool` is
// `Option<bool>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TsConfig {
    pub compiler_options: IndexMap<String, CompilerOptionsValue>,
    pub references: Vec<CompilerOptionsValue>,
    pub files: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub compile_on_save: Option<bool>,
}

// Go: tsoptions/showconfig.go:49 TSConfig (JSON v2 struct marshaler)
// PORT: Go marshals the struct by reflection in field order with the tags
// `json:"compilerOptions"`, `json:"references,omitzero"`,
// `json:"files,omitzero"`, `json:"include,omitzero"`,
// `json:"exclude,omitzero"` and `json:"compileOnSave,omitzero"`. The
// `any` values use the v2 marshaler for their dynamic type (`marshal_any`),
// and the options map uses `collections.OrderedMap.MarshalJSONTo` (members
// in insertion order).
impl MarshalerTo for TsConfig {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('{');
        enc.push_str("\"compilerOptions\":");
        enc.push('{');
        for (i, (k, v)) in self.compiler_options.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            k.marshal_json_to(enc)?;
            enc.push(':');
            marshal_any(enc, v)?;
        }
        enc.push('}');
        if !self.references.is_empty() {
            enc.push_str(",\"references\":[");
            for (i, r) in self.references.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                marshal_any(enc, r)?;
            }
            enc.push(']');
        }
        if !self.files.is_empty() {
            enc.push_str(",\"files\":");
            self.files.marshal_json_to(enc)?;
        }
        if !self.include.is_empty() {
            enc.push_str(",\"include\":");
            self.include.marshal_json_to(enc)?;
        }
        if !self.exclude.is_empty() {
            enc.push_str(",\"exclude\":");
            self.exclude.marshal_json_to(enc)?;
        }
        if let Some(compile_on_save) = self.compile_on_save {
            enc.push_str(",\"compileOnSave\":");
            compile_on_save.marshal_json_to(enc)?;
        }
        enc.push('}');
        Ok(())
    }
}

// Go: tsoptions/showconfig.go:60 ConvertToTSConfig
// ConvertToTSConfig generates a complete tsconfig representation for --showConfig output,
// matching the behavior of TypeScript's convertToTSConfig function.
#[must_use]
pub fn convert_to_ts_config(
    config_parse_result: &ParsedCommandLine,
    config_file_name: &str,
) -> TsConfig {
    let config_file_name = if config_file_name.is_empty() {
        "tsconfig.json"
    } else {
        config_file_name
    };
    let normalized_config_path = get_normalized_absolute_path(
        config_file_name,
        config_parse_result.get_current_directory(),
    );
    let compare_paths_options = ComparePathsOptions {
        current_directory: config_parse_result.get_current_directory().to_string(),
        use_case_sensitive_file_names: config_parse_result.use_case_sensitive_file_names(),
    };

    // Build the list of all resolved files as relative paths from the config file.
    let mut files: Vec<String> = Vec::new();
    for f in config_parse_result.file_names() {
        let normalized_file_path =
            get_normalized_absolute_path(f, config_parse_result.get_current_directory());
        let relative_path = get_relative_path_from_file(
            &normalized_config_path,
            &normalized_file_path,
            &compare_paths_options,
        );
        files.push(relative_path);
    }

    // Serialize compiler options
    let mut option_map = serialize_compiler_options(
        config_parse_result.compiler_options(),
        &normalized_config_path,
        &compare_paths_options,
    );

    // Add implied compiler options (options that are derived from explicitly set options,
    // such as moduleResolution implied by module, or useDefineForClassFields implied by target).
    // This mirrors TypeScript's convertToTSConfig computedOptions logic.
    add_implied_options(
        &mut option_map,
        config_parse_result.compiler_options(),
        &normalized_config_path,
        &compare_paths_options,
    );

    let mut config = TsConfig {
        compiler_options: option_map,
        ..Default::default()
    };

    // Add references
    let refs = config_parse_result.project_references();
    if !refs.is_empty() {
        let mut references: Vec<CompilerOptionsValue> = Vec::new();
        for r in refs {
            let mut ref_: IndexMap<String, CompilerOptionsValue> = IndexMap::new();
            ref_.insert(
                "path".to_string(),
                CompilerOptionsValue::String(r.original_path.clone()),
            );
            if r.circular {
                ref_.insert("circular".to_string(), CompilerOptionsValue::Bool(true));
            }
            references.push(CompilerOptionsValue::Map(ref_));
        }
        config.references = references;
    }

    // Add files
    if !files.is_empty() {
        config.files = files;
    }

    // Add include/exclude from configFileSpecs
    if let Some(specs) = config_parse_result
        .config_file
        .as_ref()
        .and_then(|config_file| config_file.config_file_specs.as_ref())
    {
        let include = filter_same_as_default_include(&specs.validated_include_specs);
        if !include.is_empty() {
            config.include = include;
        }
        config.exclude = specs.validated_exclude_specs.clone();
    }

    // Add compileOnSave
    if config_parse_result.compile_on_save == Some(true) {
        config.compile_on_save = Some(true);
    }

    config
}

// Go: tsoptions/showconfig.go:133 filterSameAsDefaultInclude
// filterSameAsDefaultInclude returns nil if specs is the default include spec ["**/*"]
// PORT: an empty `Vec` is the Go nil slice.
fn filter_same_as_default_include(specs: &[String]) -> Vec<String> {
    if specs.is_empty() {
        return Vec::new();
    }
    if specs.len() == 1 && specs[0] == DEFAULT_INCLUDE_SPEC {
        return Vec::new();
    }
    specs.to_vec()
}

// Go: tsoptions/showconfig.go:145 getNameOfCompilerOptionValue
// getNameOfCompilerOptionValue returns the string key for a given enum value by
// searching the option's enum map.
// PORT: Go `v == value` on two `any` values is `PartialEq` on
// `CompilerOptionsValue` (same dynamic type and value). Go returns "" when
// nothing matches.
fn get_name_of_compiler_option_value(
    value: &CompilerOptionsValue,
    enum_map: &CommandLineOptionEnumMap,
) -> String {
    for (k, v) in enum_map {
        if v == value {
            return k.clone();
        }
    }
    String::new()
}

/// Go `reflect.Value.IsZero` on one `core.CompilerOptions` field.
// PORT: all six Go `[]string` fields and the Go `[]PluginImport` field
// (ts#64397) are `Option<Vec<_>>` in Rust, and
// `compiler_options_field_values` lists `None` as an empty list. The shared
// helper reads `Lib` and `TypeRoots` from the struct; the other five are
// read here, so a Go non-nil empty slice (for example `"types": []` or
// `"plugins": []`) is not zero and is shown, as in Go.
fn is_zero_show_config_field(
    field_name: &str,
    value: &CompilerOptionsValue,
    options: &CompilerOptions,
) -> bool {
    match field_name {
        "CustomConditions" => options.custom_conditions.is_none(),
        "ModuleSuffixes" => options.module_suffixes.is_none(),
        "RootDirs" => options.root_dirs.is_none(),
        "Types" => options.types.is_none(),
        "Plugins" => options.plugins.is_none(),
        _ => is_zero_compiler_option_value(field_name, value, options),
    }
}

// Go: tsoptions/options_generated.go:1362 serializeCompilerOptions
// serializeCompilerOptions converts CompilerOptions to an ordered map with
// string names as keys and serialized values (enums as strings, paths as
// relative paths, etc.) matching the output of tsc --showConfig.
// PORT: since ts#64457 Go generates this function. Its option set is the
// options of tools/scripts/tsc/options.ts that are not `showConfig: false`
// (listFiles, listEmittedFiles) and not in the Command_line_Options or
// Output_Formatting category, in CompilerOptions field order. That is the
// old Go set minus listFiles; the old Go delete list after serialization is
// gone. The port keeps its field walk and skips those two options.
fn serialize_compiler_options(
    options: &CompilerOptions,
    config_file_path: &str,
    compare_paths_options: &ComparePathsOptions,
) -> IndexMap<String, CompilerOptionsValue> {
    let mut result: IndexMap<String, CompilerOptionsValue> = IndexMap::with_capacity(32);
    let config_dir = get_directory_path(config_file_path);

    // PORT: Go reflects over the exported `core.CompilerOptions` fields in
    // declaration order. `compiler_options_field_values` lists them in the
    // same order as (Go field name, value). The Go `field.IsExported()` check
    // has no counterpart: the list holds only exported fields.
    for (field_name, value) in compiler_options_field_values(options) {
        // PORT: `CommandLineOptionNameMap.get` falls back to the lowercase
        // name, like Go `NameMap.Get`.
        let Some(option_decl) = COMMAND_LINE_COMPILER_OPTIONS_MAP.get(field_name) else {
            continue;
        };

        // Skip command-line-only and output formatting options
        // PORT: Go compares the `*diagnostics.Message` pointers.
        if option_decl.category.is_some_and(|category| {
            std::ptr::eq(category, diag::Command_line_Options)
                || std::ptr::eq(category, diag::Output_Formatting)
        }) {
            continue;
        }

        // ts#64457: `showConfig: false` in Go options.ts.
        if matches!(option_decl.name, "listFiles" | "listEmittedFiles") {
            continue;
        }

        // Skip zero values (unset options)
        if is_zero_show_config_field(field_name, &value, options) {
            continue;
        }

        let name = option_decl.name;

        if let Some(enum_map) = option_decl.enum_map() {
            // Enum option - convert numeric value to string name
            let serialized = serialize_enum_value(&value, enum_map);
            if !serialized.is_empty() {
                result.insert(name.to_string(), CompilerOptionsValue::String(serialized));
            }
            continue;
        }

        match option_decl.kind {
            CommandLineOptionKind::LIST_OR_ELEMENT => {
                crate::go_assert!(false, "listOrElement option should not reach serialization");
            }
            CommandLineOptionKind::LIST => {
                let elem = option_decl.elements();
                if elem.is_some_and(|elem| elem.is_file_path) {
                    // List of file paths - make relative
                    if let CompilerOptionsValue::StringList(strs) = &value {
                        let mut rel_paths: Vec<String> = Vec::with_capacity(strs.len());
                        for s in strs {
                            let abs_path = get_normalized_absolute_path(s, &config_dir);
                            rel_paths.push(get_relative_path_from_file(
                                config_file_path,
                                &abs_path,
                                compare_paths_options,
                            ));
                        }
                        result.insert(
                            name.to_string(),
                            CompilerOptionsValue::StringList(rel_paths),
                        );
                        continue;
                    }
                }
                if let Some(elem_map) = elem.and_then(|elem| elem.enum_map()) {
                    // List of enum values (e.g., lib)
                    if let CompilerOptionsValue::StringList(strs) = &value {
                        let mut serialized: Vec<String> = Vec::with_capacity(strs.len());
                        for s in strs {
                            // lib values are already stored as the d.ts filename, need to find original key
                            let found = get_name_of_compiler_option_value(
                                &CompilerOptionsValue::String(s.clone()),
                                elem_map,
                            );
                            if !found.is_empty() {
                                serialized.push(found);
                            } else {
                                serialized.push(s.clone());
                            }
                        }
                        result.insert(
                            name.to_string(),
                            CompilerOptionsValue::StringList(serialized),
                        );
                        continue;
                    }
                }
                result.insert(name.to_string(), value);
            }

            CommandLineOptionKind::STRING => {
                if option_decl.is_file_path {
                    // File path option - make relative to config
                    if let CompilerOptionsValue::String(s) = &value {
                        if !s.is_empty() {
                            let abs_path = get_normalized_absolute_path(s, &config_dir);
                            result.insert(
                                name.to_string(),
                                CompilerOptionsValue::String(get_relative_path_from_file(
                                    config_file_path,
                                    &abs_path,
                                    compare_paths_options,
                                )),
                            );
                            continue;
                        }
                    }
                }
                result.insert(name.to_string(), value);
            }

            CommandLineOptionKind::BOOLEAN => {
                if let CompilerOptionsValue::Tristate(t) = value {
                    if t.is_true() {
                        result.insert(name.to_string(), CompilerOptionsValue::Bool(true));
                    } else if t.is_false() {
                        result.insert(name.to_string(), CompilerOptionsValue::Bool(false));
                    }
                } else {
                    result.insert(name.to_string(), value);
                }
            }

            CommandLineOptionKind::NUMBER => {
                result.insert(name.to_string(), value);
            }

            _ => {
                result.insert(name.to_string(), value);
            }
        }
    }

    result
}

/// Go `reflect.Value.CanInt()` and `Int()` on the dynamic value of an `any`.
// PORT: the signed integer kinds among `CompilerOptionsValue` variants are Go
// `int` and the `int32` enum types. `Tristate` is a Go `uint8`, `Number` a
// `float64` and `IntPtr` a pointer, so they cannot `Int()`.
fn show_config_value_as_int(value: &CompilerOptionsValue) -> Option<i64> {
    use CompilerOptionsValue as V;
    match value {
        V::Int(i) => Some(i64::from(*i)),
        V::ScriptTarget(k) => Some(i64::from(k.0)),
        V::ModuleKind(k) => Some(i64::from(k.0)),
        V::ModuleResolutionKind(k) => Some(i64::from(k.0)),
        V::ModuleDetectionKind(k) => Some(i64::from(k.0)),
        V::JsxEmit(k) => Some(i64::from(k.0)),
        V::NewLineKind(k) => Some(i64::from(k.0)),
        _ => None,
    }
}

// Go: tsoptions/showconfig.go:280 serializeEnumValue (removed by ts#64457; Go N' uses
// tsoptions/options_generated.go:1704 serializeCompilerOptionEnum)
// serializeEnumValue converts an enum field value to its corresponding string key
// using the option's enum map. It handles int32-based enum types.
fn serialize_enum_value(
    value: &CompilerOptionsValue,
    enum_map: &CommandLineOptionEnumMap,
) -> String {
    // The enum maps store values as core.ModuleKind, core.ScriptTarget, etc.
    // But those are all int32 underneath. We need to compare by the underlying int32 value.
    if let Some(int_val) = show_config_value_as_int(value) {
        for (k, v) in enum_map {
            if show_config_value_as_int(v) == Some(int_val) {
                return k.clone();
            }
        }
    }
    // Fallback: direct comparison
    get_name_of_compiler_option_value(value, enum_map)
}

// Go: tsoptions/showconfig.go:176 addImpliedOptions
// addImpliedOptions adds compiler options that are implied by other explicitly-set options,
// mirroring TypeScript's convertToTSConfig behavior for computedOptions.
// For example, when module: nodenext is set, moduleResolution: nodenext is implied.
fn add_implied_options(
    option_map: &mut IndexMap<String, CompilerOptionsValue>,
    options: &CompilerOptions,
    _config_file_path: &str,
    _compare_paths_options: &ComparePathsOptions,
) {
    // Build the set of explicitly provided option JSON names (e.g., "module", "target").
    let mut provided: FxHashSet<String> =
        FxHashSet::with_capacity_and_hasher(option_map.len(), Default::default());
    for k in option_map.keys() {
        provided.insert(k.clone());
    }

    let default_opts = CompilerOptions::default();

    for entry in &IMPLIED_OPTIONS {
        // Get the option declaration for this implied option (using case-insensitive lookup).
        let Some(option_decl) = COMMAND_LINE_COMPILER_OPTIONS_MAP.get(entry.name) else {
            continue;
        };

        // Skip if this option is already explicitly provided.
        if provided.contains(option_decl.name) {
            continue;
        }

        // Check if any direct dependency is in the provided set.
        // This mirrors TypeScript's optionDependsOn check.
        if !any_dependency_provided(entry.dependencies, &provided) {
            continue;
        }

        // Compute the effective value with current options and the default value with empty options.
        let implied = (entry.compute)(options);
        let default_val = (entry.compute)(&default_opts);

        // If the implied value equals the default, this option doesn't add useful information.
        // PORT: Go `reflect.DeepEqual` is `PartialEq`.
        if implied == default_val {
            continue;
        }

        // Serialize the implied value and add it to the option map.
        let serialized = serialize_implied_option_value(option_decl, implied);
        if serialized.is_nil() {
            continue;
        }
        option_map.insert(option_decl.name.to_string(), serialized);
    }
}

// Go: tsoptions/showconfig.go:226 anyDependencyProvided
// anyDependencyProvided returns true if any of the given dependency names
// (using Go field names like "Target") corresponds to an option in the provided set.
// PORT: Go `map[string]bool` with only true values is a set.
fn any_dependency_provided(dependencies: &[&str], provided: &FxHashSet<String>) -> bool {
    for dep in dependencies {
        if let Some(dep_decl) = COMMAND_LINE_COMPILER_OPTIONS_MAP.get(dep) {
            if provided.contains(dep_decl.name) {
                return true;
            }
        }
    }
    false
}

// Go: tsoptions/showconfig.go:239 serializeImpliedOptionValue
// serializeImpliedOptionValue converts a computed implied option value to its serializable form.
// For enum options, it converts numeric values to their string names.
// For boolean options, it returns the bool directly.
// PORT: Go returns nil `any` as `CompilerOptionsValue::Nil`.
fn serialize_implied_option_value(
    option_decl: &CommandLineOption,
    value: CompilerOptionsValue,
) -> CompilerOptionsValue {
    if value.is_nil() {
        return CompilerOptionsValue::Nil;
    }
    if let Some(enum_map) = option_decl.enum_map() {
        let s = serialize_enum_value(&value, enum_map);
        if !s.is_empty() {
            return CompilerOptionsValue::String(s);
        }
        return CompilerOptionsValue::Nil;
    }
    match value {
        CompilerOptionsValue::Bool(v) => CompilerOptionsValue::Bool(v),
        CompilerOptionsValue::Tristate(v) => {
            if v.is_true() {
                CompilerOptionsValue::Bool(true)
            } else if v.is_false() {
                CompilerOptionsValue::Bool(false)
            } else {
                CompilerOptionsValue::Nil
            }
        }
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected bytes are tsgo `--showConfig` output (execute/tsc.go:405,
    // prefix "", indent four spaces) for a solution config with two references.
    #[test]
    fn marshal_matches_tsgo_show_config() {
        let mut circular = IndexMap::new();
        circular.insert(
            "path".to_string(),
            CompilerOptionsValue::String("./packages/b/tsconfig.json".to_string()),
        );
        circular.insert("circular".to_string(), CompilerOptionsValue::Bool(true));
        let mut plain = IndexMap::new();
        plain.insert(
            "path".to_string(),
            CompilerOptionsValue::String("./packages/a".to_string()),
        );
        let config = TsConfig {
            references: vec![
                CompilerOptionsValue::Map(plain),
                CompilerOptionsValue::Map(circular),
            ],
            ..TsConfig::default()
        };
        let mut out: Vec<u8> = Vec::new();
        crate::frontend::json::json_marshal_indent_write(&mut out, &config, "", "    ").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n    \"compilerOptions\": {},\n    \"references\": [\n        {\n            \"path\": \"./packages/a\"\n        },\n        {\n            \"path\": \"./packages/b/tsconfig.json\",\n            \"circular\": true\n        }\n    ]\n}"
        );
    }

    // ts#64397: Go `reflect.Value.IsZero` of `Plugins` is false for a
    // non-nil empty slice, so tsgo `--showConfig` prints `"plugins": []`
    // for a config that sets it, and nothing for a config that does not.
    #[test]
    fn show_config_keeps_an_empty_plugins_list() {
        let compare = ComparePathsOptions::default();
        let shown = |plugins: Option<Vec<crate::options::PluginImport>>| {
            let options = CompilerOptions {
                plugins,
                ..CompilerOptions::default()
            };
            serialize_compiler_options(&options, "/project/tsconfig.json", &compare)
        };
        assert_eq!(
            shown(Some(Vec::new())).get("plugins"),
            Some(&CompilerOptionsValue::List(Vec::new()))
        );
        assert_eq!(shown(None).get("plugins"), None);
    }
}
