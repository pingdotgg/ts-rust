use crate::contentmapper::{Mapper, OptionPathSegment};
use crate::frontend::prelude::*;

// This file ports tsoptions/tsconfigparsing.go lines 901 to 1833.
// PORT: Go `any` values are `CompilerOptionsValue`. Go
// `*collections.OrderedMap[string, any]` is `IndexMap<String,
// CompilerOptionsValue>` (the `Map` variant). Go
// `collections.OrderedMap[string, string]` is `IndexMap<String, String>`
// (`FxIndexMap` in `get_file_names_from_config_specs`).
// Go `*ast.SourceFile` is the source file `Node` (`Node::NIL` is Go nil).
// Go `[][]string` extension groups are `Vec<Vec<String>>`. Go `int` is
// `i32`.

// Go: tsoptions/options_generated.go:313 getDefaultTypeAcquisition (ts#64457 generates it)
pub fn get_default_type_acquisition(config_file_name: &str) -> TypeAcquisition {
    let mut options = TypeAcquisition::default();
    if !config_file_name.is_empty() && get_base_file_name(config_file_name) == "jsconfig.json" {
        options.enable = Tristate::True;
    }
    options
}

// Go: tsoptions/tsconfigparsing.go:825 convertCompilerOptionsFromJsonWorker
fn convert_compiler_options_from_json_worker(
    json_options: &CompilerOptionsValue,
    base_path: &str,
    config_file_name: &str,
) -> (CompilerOptions, Vec<Diagnostic>) {
    let mut options = get_default_compiler_options(config_file_name);
    let (_, errors) = convert_options_from_json(
        &COMMAND_LINE_COMPILER_OPTIONS_MAP,
        json_options,
        base_path,
        CompilerOptionsParser {
            compiler_options: &mut options,
        },
    );
    if !config_file_name.is_empty() {
        options.config_file_path = normalize_slashes(config_file_name);
    }
    (options, errors)
}

// Go: tsoptions/tsconfigparsing.go:837 convertTypeAcquisitionFromJsonWorker
fn convert_type_acquisition_from_json_worker(
    json_options: &CompilerOptionsValue,
    base_path: &str,
    config_file_name: &str,
) -> (TypeAcquisition, Vec<Diagnostic>) {
    let mut options = get_default_type_acquisition(config_file_name);
    let (_, errors) = convert_options_from_json(
        &TYPE_ACQUISITION_DECLARATION.element_options,
        json_options,
        base_path,
        TypeAcquisitionParser {
            type_acquisition: &mut options,
        },
    );
    (options, errors)
}

// Go: tsoptions/tsconfigparsing.go:843 parseOwnConfigOfJson
// PORT: Go stores a `[]string` (maybe nil) in the `any` field, so the field
// is never Go nil. It is always `Some` here.
// PORT: Go sets the converted `compileOnSave` in the caller's map, which
// becomes `raw`. The port changes its own copy, which becomes `raw`; the
// caller does not read `json` again.
fn parse_own_config_of_json(
    json: &IndexMap<String, CompilerOptionsValue>,
    host: &dyn ParseConfigHost,
    base_path: &str,
    config_file_name: &str,
) -> (ParsedTsconfig, Vec<Diagnostic>) {
    let mut json = json.clone();
    let nil = CompilerOptionsValue::Nil;
    let mut errors: Vec<Diagnostic> = Vec::new();
    if json.contains_key("excludes") {
        errors.push(new_compiler_diagnostic(
            diag::Unknown_option_excludes_Did_you_mean_exclude,
            args![],
        ));
    }
    let (options, err) = convert_compiler_options_from_json_worker(
        json.get("compilerOptions").unwrap_or(&nil),
        base_path,
        config_file_name,
    );
    let (type_acquisition, err2) = convert_type_acquisition_from_json_worker(
        json.get("typeAcquisition").unwrap_or(&nil),
        base_path,
        config_file_name,
    );
    errors.extend(err);
    errors.extend(err2);
    if let Some(compile_on_save) = json.get("compileOnSave").cloned() {
        let (converted, compile_on_save_errors) = convert_json_option(
            *COMPILE_ON_SAVE_COMMAND_LINE_OPTION,
            compile_on_save,
            base_path,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        errors.extend(compile_on_save_errors);
        json.insert("compileOnSave".to_string(), converted);
    }
    let mut extended_config_path: Vec<String> = Vec::new();
    let extends = json.get("extends").unwrap_or(&nil);
    if !extends.is_nil() && *extends != CompilerOptionsValue::String(String::new()) {
        let err;
        (extended_config_path, err) = get_extends_config_path_or_array(
            extends,
            host,
            base_path,
            config_file_name,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        errors.extend(err);
    }
    let parsed_config = ParsedTsconfig {
        raw: CompilerOptionsValue::Map(json),
        options: Some(options),
        type_acquisition: Some(type_acquisition),
        extended_config_path: Some(extended_config_path),
    };
    (parsed_config, errors)
}

// Go: tsoptions/tsconfigparsing.go:875 readJsonConfigFile
// PORT: the parser keeps source text for the program, so the text and the
// file name are leaked. The empty file gets its own node store, like the
// parsed one.
fn read_json_config_file(
    file_name: &str,
    path: Path,
    read_file: &dyn Fn(&str) -> (String, bool),
) -> (TsConfigSourceFile, Vec<Diagnostic>) {
    let (text, diagnostic) =
        try_read_file(file_name, &mut |name: &str| read_file(name), Vec::new());
    if !text.is_empty() {
        let source_file = parse_source_file(
            &SourceFileParseOptions {
                file_name: file_name.to_string(),
                path,
                ..Default::default()
            },
            FileText::Static(Box::leak(text.into_boxed_str())),
            ScriptKind::JSON,
        );
        (
            TsConfigSourceFile {
                source_file: source_file.root,
                path: source_file.path().clone(),
                file_name: source_file.file_name().to_string(),
                ..Default::default()
            },
            diagnostic,
        )
    } else {
        let factory = NodeFactory::for_file(new_file_store(
            Box::leak(file_name.to_string().into_boxed_str()),
            "",
        ));
        let file = TsConfigSourceFile {
            path: path.clone(),
            file_name: file_name.to_string(),
            source_file: factory.new_parsed_source_file(
                &SourceFileParseOptions {
                    file_name: file_name.to_string(),
                    path,
                    ..Default::default()
                },
                "",
                factory.new_node_list(&[]),
                factory.new_token(SyntaxKind::EndOfFile),
            ),
            ..Default::default()
        };
        set_source_file_diagnostics(file.source_file, diagnostic.clone());
        (file, diagnostic)
    }
}

// Go: tsoptions/tsconfigparsing.go:894 getExtendedConfig
fn get_extended_config(
    source_file: Option<&TsConfigSourceFile>,
    extended_config_file_name: &str,
    host: &dyn ParseConfigHost,
    resolution_stack: &[Path],
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
    result: &mut ExtendsResult,
) -> (Option<Rc<ParsedTsconfig>>, Vec<Diagnostic>) {
    let mut errors: Vec<Diagnostic> = Vec::new();
    let extended_config_path = to_path(
        extended_config_file_name,
        &host.get_current_directory(),
        host.fs().use_case_sensitive_file_names(),
    );

    // Bypass the cache when we detect a cycle in the resolution stack.
    // The cache locks entries during parsing, and a cycle would cause the same goroutine
    // to re-lock the same entry, resulting in a deadlock. Let parseConfig handle the
    // circularity error via its own resolution stack check.
    let cache_entry: Rc<ExtendedConfigCacheEntry> = match extended_config_cache {
        Some(cache) if !resolution_stack.contains(&extended_config_path) => cache
            .get_extended_config(
                extended_config_file_name,
                &extended_config_path,
                resolution_stack,
                host,
            ),
        _ => Rc::new(parse_extended_config(
            extended_config_file_name,
            extended_config_path,
            resolution_stack,
            host,
            extended_config_cache,
        )),
    };

    if !cache_entry.errors.is_empty() {
        errors.extend(cache_entry.errors.iter().cloned());
    }

    if let Some(extended_result) = &cache_entry.extended_result
        && source_file.is_some()
    {
        result
            .extended_source_files
            .insert(source_file_file_name(extended_result.source_file).to_string());
        for extended_source_file in &extended_result.extended_source_files {
            result
                .extended_source_files
                .insert(extended_source_file.clone());
        }
    }
    (cache_entry.extended_config.clone(), errors)
}

// Go: tsoptions/tsconfigparsing.go:931 ParseExtendedConfig
// PORT: Go returns a pointer; this returns the value. Callers wrap it in
// `Rc` where Go shares it.
pub fn parse_extended_config(
    file_name: &str,
    path: Path,
    resolution_stack: &[Path],
    host: &dyn ParseConfigHost,
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> ExtendedConfigCacheEntry {
    let (mut extended_result, read_errors) =
        read_json_config_file(file_name, path, &|name| host.fs().read_file(name));
    let mut entry = ExtendedConfigCacheEntry::default();

    if !read_errors.is_empty() {
        entry.extended_result = Some(Rc::new(extended_result));
        entry.errors = read_errors;
        return entry;
    }

    let parse_diagnostics = parsed_source_file_diagnostics(extended_result.source_file);
    if !parse_diagnostics.is_empty() {
        entry.extended_result = Some(Rc::new(extended_result));
        entry.errors = parse_diagnostics.to_vec();
        return entry;
    }

    let (extended_config, parse_errors) = parse_config(
        None,
        Some(&mut extended_result),
        host,
        &get_directory_path(file_name),
        &get_base_file_name(file_name),
        resolution_stack,
        extended_config_cache,
    );
    entry.extended_result = Some(Rc::new(extended_result));
    entry.extended_config = Some(Rc::new(extended_config));
    entry.errors = parse_errors;
    entry
}

/// Go `rawMap.(*collections.OrderedMap[string, any])` with the `ok` form.
fn raw_as_map(raw: &CompilerOptionsValue) -> Option<&IndexMap<String, CompilerOptionsValue>> {
    match raw {
        CompilerOptionsValue::Map(m) => Some(m),
        _ => None,
    }
}

/// Go `ownConfig.raw.(*collections.OrderedMap[string, any])`, which panics
/// when the value is not a map.
fn raw_as_map_mut(raw: &mut CompilerOptionsValue) -> &mut IndexMap<String, CompilerOptionsValue> {
    match raw {
        CompilerOptionsValue::Map(m) => m,
        _ => panic!(
            "interface conversion: raw config is not *collections.OrderedMap[string,interface {{}}]"
        ),
    }
}

// parseConfig just extracts options/include/exclude/files out of a config file.
// It does not resolve the included files.
// Go: tsoptions/tsconfigparsing.go:961 parseConfig
// PORT: Go `json` is a nilable map pointer (`Option`, owned). Go
// `sourceFile` is a nilable pointer that this function changes, so it is
// `Option<&mut TsConfigSourceFile>`. The Go `applyExtendedConfig` closure
// is inlined into the loop over the extended config paths. Go
// `extendedConfigPath` only ever holds a `[]string`, so the Go string case
// is not reachable and not ported.
pub fn parse_config(
    json: Option<IndexMap<String, CompilerOptionsValue>>,
    mut source_file: Option<&mut TsConfigSourceFile>,
    host: &dyn ParseConfigHost,
    base_path: &str,
    config_file_name: &str,
    resolution_stack: &[Path],
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> (ParsedTsconfig, Vec<Diagnostic>) {
    let base_path = normalize_slashes(base_path);
    let resolved_path = to_path(
        config_file_name,
        &base_path,
        host.fs().use_case_sensitive_file_names(),
    );
    let mut errors: Vec<Diagnostic> = Vec::new();
    if resolution_stack.contains(&resolved_path) {
        errors.push(new_compiler_diagnostic(
            diag::Circularity_detected_while_resolving_configuration_Colon_0,
            args![],
        ));
        let result;
        if json.as_ref().map_or(0, IndexMap::len) == 0 {
            // PORT: Go stores the (maybe nil) map pointer. A nil map is `Nil`.
            result = ParsedTsconfig {
                raw: json.map_or(CompilerOptionsValue::Nil, CompilerOptionsValue::Map),
                ..Default::default()
            };
        } else {
            // PORT: Go reads `sourceFile.SourceFile` here, which panics when
            // `sourceFile` is nil (the `json` path). Kept as Go does.
            let (raw_result, err) = convert_to_object(
                source_file
                    .as_deref()
                    .expect("nil pointer dereference: sourceFile")
                    .source_file,
            );
            errors.extend(err);
            result = ParsedTsconfig {
                raw: raw_result,
                ..Default::default()
            };
        }
        return (result, errors);
    }

    let (mut own_config, err) = match &json {
        Some(json) => parse_own_config_of_json(json, host, &base_path, config_file_name),
        None => parse_own_config_of_json_source_file(
            tsconfig_to_source_file(source_file.as_deref()),
            host,
            &base_path,
            config_file_name,
        ),
    };
    errors.extend(err);
    if let Some(options) = own_config.options.as_mut()
        && options.paths.is_some()
    {
        // If we end up needing to resolve relative paths from 'paths' relative to
        // the config file location, we'll need to know where that config file was.
        // Since 'paths' can be inherited from an extended config in another directory,
        // we wouldn't know which directory to use unless we store it here.
        options.paths_base_path = base_path.clone();
    }

    if let Some(extended_config_paths) = own_config.extended_config_path.clone() {
        // copy the resolution stack so it is never reused between branches in potential diamond-problem scenarios.
        let mut resolution_stack = resolution_stack.to_vec();
        resolution_stack.push(resolved_path);
        let mut result = ExtendsResult::default();
        for extended_config_path in &extended_config_paths {
            // Go: applyExtendedConfig(result, extendedConfigPath)
            let (extended_config, extended_errors) = get_extended_config(
                source_file.as_deref(),
                extended_config_path,
                host,
                &resolution_stack,
                extended_config_cache,
                &mut result,
            );
            errors.extend(extended_errors);
            if let Some(extended_config) = extended_config
                && extended_config.options.is_some()
            {
                let extends_raw = &extended_config.raw;
                let mut relative_difference = String::new();
                for property_name in ["include", "exclude", "files"] {
                    // Go: setPropertyValue(propertyName)
                    if let Some(raw_map) = raw_as_map(&own_config.raw)
                        && raw_map.contains_key(property_name)
                    {
                        continue;
                    }
                    if let Some(raw_map) = raw_as_map(extends_raw)
                        && raw_map.contains_key(property_name)
                        && let Some(CompilerOptionsValue::List(slice)) = raw_map.get(property_name)
                    {
                        let value: Vec<CompilerOptionsValue> = slice
                            .iter()
                            .map(|path| {
                                let CompilerOptionsValue::String(path_str) = path else {
                                    return path.clone();
                                };
                                if starts_with_config_dir_template(path)
                                    || is_rooted_disk_path(path_str)
                                {
                                    CompilerOptionsValue::String(path_str.clone())
                                } else {
                                    if relative_difference.is_empty() {
                                        let t = ComparePathsOptions {
                                            use_case_sensitive_file_names: host
                                                .fs()
                                                .use_case_sensitive_file_names(),
                                            current_directory: base_path.clone(),
                                        };
                                        relative_difference = convert_to_relative_path(
                                            &get_directory_path(extended_config_path),
                                            &t,
                                        );
                                    }
                                    CompilerOptionsValue::String(combine_paths(
                                        &relative_difference,
                                        &[path_str],
                                    ))
                                }
                            })
                            .collect();
                        match property_name {
                            "include" => result.include = Some(value),
                            "exclude" => result.exclude = Some(value),
                            _ => result.files = Some(value),
                        }
                    }
                }
                // tsgo#4712. PORT: Go `result.contentMappers, _ = ....([]any)`
                // sets nil for a value that is not an array.
                if let Some(extended_raw_map) = raw_as_map(extends_raw)
                    && extended_raw_map.contains_key("contentMappers")
                {
                    result.content_mappers = match extended_raw_map.get("contentMappers") {
                        Some(CompilerOptionsValue::List(content_mappers)) => {
                            Some(content_mappers.clone())
                        }
                        _ => None,
                    };
                }
                if let Some(extended_raw_map) = raw_as_map(extends_raw)
                    && extended_raw_map.contains_key("compileOnSave")
                    && let Some(CompilerOptionsValue::Bool(compile_on_save)) =
                        extended_raw_map.get("compileOnSave")
                {
                    result.compile_on_save = *compile_on_save;
                }
                merge_compiler_options_with_paths(
                    &mut result.options,
                    extended_config.options.as_ref(),
                    raw_as_map(extends_raw),
                    extended_config_path,
                    &base_path,
                );
            }
        }
        if let Some(include) = result.include.take() {
            raw_as_map_mut(&mut own_config.raw)
                .insert("include".to_string(), CompilerOptionsValue::List(include));
        }
        if let Some(exclude) = result.exclude.take() {
            raw_as_map_mut(&mut own_config.raw)
                .insert("exclude".to_string(), CompilerOptionsValue::List(exclude));
        }
        if let Some(files) = result.files.take() {
            raw_as_map_mut(&mut own_config.raw)
                .insert("files".to_string(), CompilerOptionsValue::List(files));
        }
        // tsgo#4712
        if let Some(content_mappers) = result.content_mappers.take()
            && !raw_as_map_mut(&mut own_config.raw).contains_key("contentMappers")
        {
            raw_as_map_mut(&mut own_config.raw).insert(
                "contentMappers".to_string(),
                CompilerOptionsValue::List(content_mappers),
            );
        }
        if result.compile_on_save
            && !raw_as_map_mut(&mut own_config.raw).contains_key("compileOnSave")
        {
            raw_as_map_mut(&mut own_config.raw).insert(
                "compileOnSave".to_string(),
                CompilerOptionsValue::Bool(result.compile_on_save),
            );
        }
        if let Some(source_file) = source_file.as_deref_mut() {
            for extended_source_file in &result.extended_source_files {
                // Go: core.InsertSorted(..., cmp.Compare)
                let i = match source_file
                    .extended_source_files
                    .binary_search(extended_source_file)
                {
                    Ok(i) | Err(i) => i,
                };
                source_file
                    .extended_source_files
                    .insert(i, extended_source_file.clone());
            }
        }
        let own_options = own_config.options.take();
        merge_compiler_options_with_paths(
            &mut result.options,
            own_options.as_ref(),
            raw_as_map(&own_config.raw),
            config_file_name,
            &base_path,
        );
        own_config.options = Some(result.options);
    }
    (own_config, errors)
}

// Go: tsoptions/tsconfigparsing.go:1100 defaultIncludeSpec
pub const DEFAULT_INCLUDE_SPEC: &str = "**/*";

// Go: tsoptions/tsconfigparsing.go:1102 propOfRaw
// PORT: Go nil `sliceValue` is `None`.
struct PropOfRaw {
    slice_value: Option<Vec<CompilerOptionsValue>>,
    wrong_value: &'static str,
}

// Go: tsoptions/tsconfigparsing.go:1210 getPropFromRaw (closure in parseJsonConfigFileContentWorker)
// PORT: the Go closure is a private function. `is_json` is Go
// `sourceFile == nil`. A nil element fails both element checks, as in Go.
// A Go `[]string` value makes the `.([]any)` assertion panic; so does this
// port.
fn get_prop_from_raw(
    raw_config: &IndexMap<String, CompilerOptionsValue>,
    is_json: bool,
    errors: &mut Vec<Diagnostic>,
    prop: &str,
    validate_element: fn(&CompilerOptionsValue) -> bool,
    element_type_name: &str,
) -> PropOfRaw {
    if let Some(value) = raw_config.get(prop)
        && !value.is_nil()
    {
        match value {
            CompilerOptionsValue::List(result) => {
                if is_json && !result.iter().all(validate_element) {
                    errors.push(new_compiler_diagnostic(
                        diag::Compiler_option_0_requires_a_value_of_type_1,
                        args![prop, element_type_name],
                    ));
                }
                return PropOfRaw {
                    slice_value: Some(result.clone()),
                    wrong_value: "",
                };
            }
            // A nil `[]any` (an array of nulls): Go `core.Every` of no
            // elements is true, and the nil `sliceValue` keeps no wrong value.
            CompilerOptionsValue::NilList => {
                return PropOfRaw {
                    slice_value: None,
                    wrong_value: "",
                };
            }
            CompilerOptionsValue::StringList(_) => {
                panic!("interface conversion: raw value is []string, not []interface {{}}");
            }
            _ => {
                if is_json {
                    errors.push(new_compiler_diagnostic(
                        diag::Compiler_option_0_requires_a_value_of_type_1,
                        args![prop, "Array"],
                    ));
                    return PropOfRaw {
                        slice_value: None,
                        wrong_value: "not-array",
                    };
                }
            }
        }
    }
    PropOfRaw {
        slice_value: None,
        wrong_value: "no-prop",
    }
}

/// Go `reflect.TypeOf(element) == orderedMapType`.
fn is_map_element(element: &CompilerOptionsValue) -> bool {
    matches!(element, CompilerOptionsValue::Map(_))
}

// Go: tsoptions/tsconfigparsing.go:1107 isStringValue
fn is_string_value(element: &CompilerOptionsValue) -> bool {
    matches!(element, CompilerOptionsValue::String(_))
}

/// Go `configFileSpecs` stores each `[]any` spec list in an `any`.
// PORT: a Go nil `[]any` in an `any` is not Go nil, but the only reader
// (the No_inputs diagnostic) prints it and a nil list alike as `[]`.
fn spec_list_value(specs: &Option<Vec<CompilerOptionsValue>>) -> CompilerOptionsValue {
    match specs {
        Some(specs) => CompilerOptionsValue::List(specs.clone()),
        None => CompilerOptionsValue::Nil,
    }
}

/// Go `core.StringifyJson(value, "", "")` for the spec lists. Go uses
/// `internal/json` (json v2): compact output, a nil slice is `[]`, and only
/// `"`, `\` and control characters are escaped.
// PORT: only the value kinds that JSON conversion makes are handled.
pub(crate) fn stringify_json(value: &CompilerOptionsValue, out: &mut String) {
    match value {
        CompilerOptionsValue::Nil | CompilerOptionsValue::NilList => out.push_str("[]"),
        CompilerOptionsValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CompilerOptionsValue::Int(i) => out.push_str(&i.to_string()),
        CompilerOptionsValue::Number(n) => out.push_str(&crate::jsnum::Number(*n).to_string()),
        CompilerOptionsValue::String(s) => append_json_quote(out, s),
        CompilerOptionsValue::StringList(list) => {
            out.push('[');
            for (i, s) in list.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                append_json_quote(out, s);
            }
            out.push(']');
        }
        CompilerOptionsValue::List(list) => {
            out.push('[');
            for (i, v) in list.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if v.is_nil() {
                    out.push_str("null");
                } else {
                    stringify_json(v, out);
                }
            }
            out.push(']');
        }
        CompilerOptionsValue::Map(m) => {
            out.push('{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                append_json_quote(out, k);
                out.push(':');
                if v.is_nil() {
                    out.push_str("null");
                } else {
                    stringify_json(v, out);
                }
            }
            out.push('}');
        }
        other => panic!("stringify_json: value kind not made by JSON conversion: {other:?}"),
    }
}

// parseJsonConfigFileContentWorker parses the contents of a config file from json or json source file (tsconfig.json).
// json: The contents of the config file to parse
// sourceFile: sourceFile corresponding to the Json
// host: Instance of ParseConfigHost used to enumerate files in folder.
// basePath: A root directory to resolve relative path entries in the config file to. e.g. outDir
// resolutionStack: Only present for backwards-compatibility. Should be empty.
// Go: tsoptions/tsconfigparsing.go:1118 parseJsonConfigFileContentWorker
// PORT: Go `sourceFile` is a pointer that the result keeps, so it moves in
// and into `ParsedCommandLine.config_file`. The Go `getFileNames` and
// `getProjectReferences` closures run inline, in Go order. When `parseConfig`
// hits a cycle, Go options are nil and `mergeCompilerOptions` panics if
// `existingOptions` is set; here the merge is skipped.
#[allow(clippy::too_many_arguments)]
pub fn parse_json_config_file_content_worker(
    json: Option<IndexMap<String, CompilerOptionsValue>>,
    source_file: Option<TsConfigSourceFile>,
    host: &dyn ParseConfigHost,
    base_path: &str,
    existing_options: Option<&CompilerOptions>,
    existing_options_raw: Option<&IndexMap<String, CompilerOptionsValue>>,
    config_file_name: &str,
    resolution_stack: &[Path],
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> ParsedCommandLine {
    debug_assert!(
        (json.is_none() && source_file.is_some()) || (json.is_some() && source_file.is_none())
    );
    let mut source_file = source_file;

    let base_path_for_file_names = if !config_file_name.is_empty() {
        normalize_path(&directory_of_combined_path(config_file_name, base_path))
    } else {
        normalize_path(base_path)
    };

    let (mut parsed_config, mut errors) = parse_config(
        json,
        source_file.as_mut(),
        host,
        base_path,
        config_file_name,
        resolution_stack,
        extended_config_cache,
    );
    if let Some(options) = parsed_config.options.as_mut() {
        merge_compiler_options_with_paths(
            options,
            existing_options,
            existing_options_raw,
            config_file_name,
            base_path,
        );
    }
    handle_option_config_dir_template_substitution(
        parsed_config.options.as_mut(),
        &base_path_for_file_names,
    );
    // PORT: `parse_json_to_string_key` is always `Some`.
    let raw_config = parse_json_to_string_key(&parsed_config.raw).unwrap_or_default();
    if !config_file_name.is_empty()
        && let Some(options) = parsed_config.options.as_mut()
    {
        options.config_file_path = normalize_slashes(config_file_name);
    }
    let is_json = source_file.is_none();
    let references_of_raw = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "references",
        is_map_element,
        "object",
    );
    let file_specs = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "files",
        is_string_value,
        "string",
    );
    if file_specs.slice_value.is_some() || file_specs.wrong_value.is_empty() {
        let mut has_zero_or_no_references = false;
        if references_of_raw.wrong_value == "no-prop"
            || references_of_raw.wrong_value == "not-array"
            || references_of_raw.slice_value.as_ref().map_or(0, Vec::len) == 0
        {
            has_zero_or_no_references = true;
        }
        let has_extends = raw_config.get("extends").is_some_and(|v| !v.is_nil());
        if file_specs.slice_value.as_ref().is_some_and(Vec::is_empty)
            && has_zero_or_no_references
            && !has_extends
        {
            if let Some(source_file) = &source_file {
                let file_name = if !config_file_name.is_empty() {
                    config_file_name
                } else {
                    "tsconfig.json"
                };
                let diagnostic_message = diag::The_files_list_in_config_file_0_is_empty;
                // In Go (tsconfigparsing.go:1300) `nodeValue` is nil when the top
                // level is not an object (`[{"files": []}]`), and
                // CreateDiagnosticForNodeInSourceFile (errors.go:93) then
                // dereferences it.
                let node_value =
                    for_each_tsconfig_prop_array(source_file.source_file, "files", |property| {
                        Some(property.initializer())
                    })
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                errors.push(create_diagnostic_for_node_in_source_file(
                    source_file.source_file,
                    node_value,
                    diagnostic_message,
                    args![file_name],
                ));
            } else {
                errors.push(new_compiler_diagnostic(
                    diag::The_files_list_in_config_file_0_is_empty,
                    args![config_file_name],
                ));
            }
        }
    }
    let mut include_specs = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "include",
        is_string_value,
        "string",
    );
    let mut exclude_specs = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "exclude",
        is_string_value,
        "string",
    );
    let mut is_default_include_spec = false;
    if exclude_specs.wrong_value == "no-prop"
        && let Some(options) = &parsed_config.options
    {
        let out_dir = &options.out_dir;
        let declaration_dir = &options.declaration_dir;
        if !out_dir.is_empty() || !declaration_dir.is_empty() {
            let mut values: Vec<CompilerOptionsValue> = Vec::new();
            if !out_dir.is_empty() {
                values.push(CompilerOptionsValue::String(out_dir.clone()));
            }
            if !declaration_dir.is_empty() {
                values.push(CompilerOptionsValue::String(declaration_dir.clone()));
            }
            exclude_specs = PropOfRaw {
                slice_value: Some(values),
                wrong_value: "",
            };
        }
    }
    if file_specs.slice_value.is_none() && include_specs.slice_value.is_none() {
        include_specs = PropOfRaw {
            slice_value: Some(vec![CompilerOptionsValue::String(
                DEFAULT_INCLUDE_SPEC.to_string(),
            )]),
            wrong_value: "",
        };
        is_default_include_spec = true;
    }
    let mut validated_include_specs: Vec<String> = Vec::new();
    let mut validated_include_specs_before_substitution: Vec<String> = Vec::new();
    let mut validated_exclude_specs: Vec<String> = Vec::new();
    let mut validated_files_spec: Vec<String> = Vec::new();
    let mut validated_files_spec_before_substitution: Vec<String> = Vec::new();
    let tsconfig_node = tsconfig_to_source_file(source_file.as_ref());
    // The exclude spec list is converted into a regular expression, which allows us to quickly
    // test whether a file or directory should be excluded before recursively traversing the
    // file system.
    if let Some(specs) = &include_specs.slice_value {
        let err;
        (validated_include_specs_before_substitution, err) = validate_specs(
            specs,
            true, /*disallowTrailingRecursion*/
            tsconfig_node,
            "include",
        );
        errors.extend(err);
        validated_include_specs = match get_substituted_string_array_with_config_dir_template(
            &validated_include_specs_before_substitution,
            &base_path_for_file_names,
        ) {
            Some(substituted) => substituted,
            None => validated_include_specs_before_substitution.clone(),
        };
    }
    if let Some(specs) = &exclude_specs.slice_value {
        let err;
        (validated_exclude_specs, err) = validate_specs(
            specs,
            false, /*disallowTrailingRecursion*/
            tsconfig_node,
            "exclude",
        );
        errors.extend(err);
        if let Some(validated_exclude_specs_with_substitution) =
            get_substituted_string_array_with_config_dir_template(
                &validated_exclude_specs,
                &base_path_for_file_names,
            )
        {
            validated_exclude_specs = validated_exclude_specs_with_substitution;
        }
    }
    if let Some(specs) = &file_specs.slice_value {
        for spec in specs {
            if let CompilerOptionsValue::String(spec) = spec {
                validated_files_spec_before_substitution.push(spec.clone());
            }
        }
        validated_files_spec = match get_substituted_string_array_with_config_dir_template(
            &validated_files_spec_before_substitution,
            &base_path_for_file_names,
        ) {
            Some(substituted) => substituted,
            None => validated_files_spec_before_substitution.clone(),
        };
    }
    let config_file_specs = ConfigFileSpecs {
        files_specs: spec_list_value(&file_specs.slice_value),
        include_specs: spec_list_value(&include_specs.slice_value),
        exclude_specs: spec_list_value(&exclude_specs.slice_value),
        validated_files_spec,
        validated_include_specs,
        validated_exclude_specs,
        validated_files_spec_before_substitution,
        validated_include_specs_before_substitution,
        is_default_include_spec,
    };

    if let Some(source_file) = source_file.as_mut() {
        source_file.config_file_specs = Some(config_file_specs.clone());
    }

    // tsgo#4712
    let content_mapper_source_file = tsconfig_node;
    let mut content_mappers: Vec<Mapper> = Vec::new();
    let mut content_mapper_indices: Vec<i32> = Vec::new();
    let content_mappers_of_raw = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "contentMappers",
        is_map_element,
        "object",
    );
    for (i, element) in content_mappers_of_raw
        .slice_value
        .iter()
        .flatten()
        .enumerate()
    {
        let (mapper, mapper_errors) = parse_content_mapper(element);
        for mapper_error in mapper_errors {
            errors.push(set_content_mapper_diagnostic_location(
                mapper_error,
                content_mapper_source_file,
                get_content_mapper_syntax(content_mapper_source_file, i as i32, ""),
            ));
        }
        if let Some(mapper) = mapper {
            content_mappers.push(mapper);
            content_mapper_indices.push(i as i32);
        }
    }
    let total_content_mapper_extensions: usize = content_mappers
        .iter()
        .map(|mapper| mapper.definition.extensions.len())
        .sum();
    let mut seen_content_mapper_extensions: FxHashSet<String> =
        FxHashSet::with_capacity_and_hasher(total_content_mapper_extensions, Default::default());
    let mut content_mapper_extensions: Vec<String> =
        Vec::with_capacity(total_content_mapper_extensions);
    let native_extensions: Vec<&str> = ALL_SUPPORTED_EXTENSIONS_WITH_JSON
        .iter()
        .flat_map(|group| group.iter().copied())
        .collect();
    let canonical_extension = |extension: &str| -> String {
        get_canonical_file_name(extension, host.fs().use_case_sensitive_file_names())
    };
    for (j, mapper) in content_mappers.iter_mut().enumerate() {
        let mut valid_extensions: Vec<String> =
            Vec::with_capacity(mapper.definition.extensions.len());
        for ext in &mapper.definition.extensions {
            let ext_node = get_content_mapper_extension_syntax(
                content_mapper_source_file,
                content_mapper_indices[j],
                ext,
            );
            let canonical_ext = canonical_extension(ext);
            if !ext.starts_with('.') {
                errors.push(set_content_mapper_diagnostic_location(
                    new_compiler_diagnostic(
                        diag::Content_mapper_file_extension_0_must_begin_with_a,
                        args![ext],
                    ),
                    content_mapper_source_file,
                    ext_node,
                ));
            } else if native_extensions.iter().any(|native_extension| {
                crate::frontend::vfs::vfsmatch::equal_fold(
                    native_extension.as_bytes(),
                    ext.as_bytes(),
                )
            }) {
                errors.push(set_content_mapper_diagnostic_location(
                    new_compiler_diagnostic(
                        diag::Content_mapper_file_extension_0_is_a_built_in_extension_and_cannot_be_registered_by_a_content_mapper,
                        args![ext],
                    ),
                    content_mapper_source_file,
                    ext_node,
                ));
            } else if seen_content_mapper_extensions.contains(&canonical_ext) {
                errors.push(set_content_mapper_diagnostic_location(
                    new_compiler_diagnostic(
                        diag::Content_mapper_file_extension_0_is_registered_by_more_than_one_content_mapper,
                        args![ext],
                    ),
                    content_mapper_source_file,
                    ext_node,
                ));
            } else {
                seen_content_mapper_extensions.insert(canonical_ext);
                content_mapper_extensions.push(ext.clone());
                valid_extensions.push(ext.clone());
            }
        }
        mapper.definition.extensions = valid_extensions;
    }
    if !content_mappers.is_empty()
        && !parsed_config
            .options
            .as_ref()
            .is_some_and(|options| options.run_external_code.is_true())
    {
        errors.push(set_content_mapper_diagnostic_location(
            new_compiler_diagnostic(
                diag::Content_mappers_require_the_runExternalCode_command_line_flag_to_be_enabled,
                args![],
            ),
            content_mapper_source_file,
            get_content_mappers_key_syntax(content_mapper_source_file),
        ));
        // Without the flag the mappers are not trusted to run, so drop them entirely: their extensions are
        // not registered and their files are not intercepted (they are treated as unknown foreign files).
        content_mappers = Vec::new();
        content_mapper_extensions = Vec::new();
    } else if !content_mappers.is_empty() {
        // Resolve each mapper's package.json now so its name, version, and run command are available to
        // everything downstream (diagnostics, build-info staleness) without executing anything.
        let containing_file = if config_file_name.is_empty() {
            combine_paths(&base_path_for_file_names, &["tsconfig.json"])
        } else {
            config_file_name.to_string()
        };
        let mut resolved_content_mappers: Vec<Mapper> = Vec::with_capacity(content_mappers.len());
        for (j, mut mapper) in content_mappers.into_iter().enumerate() {
            let (manifest, package_directory, diagnostic) =
                resolve_content_mapper_manifest(host, &containing_file, &mapper.definition.package);
            mapper.package_directory = package_directory;
            if let Some(diagnostic) = diagnostic {
                errors.push(set_content_mapper_diagnostic_location(
                    diagnostic,
                    content_mapper_source_file,
                    get_content_mapper_syntax(
                        content_mapper_source_file,
                        content_mapper_indices[j],
                        "package",
                    ),
                ));
                continue;
            }
            mapper.manifest = manifest;
            resolved_content_mappers.push(mapper);
        }
        content_mappers = resolved_content_mappers;
        content_mapper_extensions = content_mappers
            .iter()
            .flat_map(|mapper| mapper.definition.extensions.iter().cloned())
            .collect();
    }

    // Effect-TS/tsgo patch 030: validate the final Effect plugin options.
    if let Some(options) = parsed_config.options.as_ref() {
        errors.extend(crate::effect::configcheck::validate(options, tsconfig_node));
    }

    // Go: getFileNames(basePathForFileNames)
    let (file_names, literal_file_names_len) = {
        let parsed_config_options = parsed_config.options.as_ref();
        let (file_names, literal_file_names_len) = host.get_file_names_from_config_specs(
            config_file_name,
            &config_file_specs,
            &base_path_for_file_names,
            parsed_config_options,
            &content_mapper_extensions,
        );
        if should_report_no_input_files(
            &file_names,
            can_json_report_no_input_files(&raw_config),
            resolution_stack,
        ) {
            let mut include_json = String::new();
            stringify_json(&config_file_specs.include_specs, &mut include_json);
            let mut exclude_json = String::new();
            stringify_json(&config_file_specs.exclude_specs, &mut exclude_json);
            errors.push(new_compiler_diagnostic(
                diag::No_inputs_were_found_in_config_file_0_Specified_include_paths_were_1_and_exclude_paths_were_2,
                args![config_file_name, include_json, exclude_json],
            ));
        }
        (file_names, literal_file_names_len)
    };
    let mut compile_on_save = false;
    if let CompilerOptionsValue::Map(raw) = &parsed_config.raw
        && let Some(CompilerOptionsValue::Bool(value)) = raw.get("compileOnSave")
    {
        compile_on_save = *value;
    }

    // Go: getProjectReferences(basePathForFileNames)
    // PORT: Go nil is `None`. A `references` list, even `[]`, is `Some`.
    let mut project_references: Option<Vec<ProjectReference>> = None;
    let new_references_of_raw = get_prop_from_raw(
        &raw_config,
        is_json,
        &mut errors,
        "references",
        is_map_element,
        "object",
    );
    if let Some(references) = &new_references_of_raw.slice_value {
        let project_references = project_references.insert(Vec::new());
        for (index, reference) in references.iter().enumerate() {
            let Some(r) = parse_project_reference(reference) else {
                continue;
            };
            if !r.has_path || !r.path_valid {
                errors.push(create_diagnostic_at_project_reference_property(
                    source_file.as_ref(),
                    index,
                    "path",
                    diag::Compiler_option_0_requires_a_value_of_type_1,
                    args!["reference.path", "string"],
                ));
                continue;
            }
            if r.reference.path.is_empty() {
                errors.push(create_diagnostic_at_project_reference_property(
                    source_file.as_ref(),
                    index,
                    "path",
                    diag::Compiler_option_0_cannot_be_given_an_empty_string,
                    args!["reference.path"],
                ));
                continue;
            }
            if r.has_circular && !r.circular_valid {
                errors.push(create_diagnostic_at_project_reference_property(
                    source_file.as_ref(),
                    index,
                    "circular",
                    diag::Compiler_option_0_requires_a_value_of_type_1,
                    args!["reference.circular", "boolean"],
                ));
            }
            project_references.push(ProjectReference {
                path: get_normalized_absolute_path(&r.reference.path, &base_path_for_file_names),
                original_path: r.reference.path.clone(),
                circular: r.reference.circular,
            });
        }
    }

    ParsedCommandLine {
        parsed_config: ParsedOptions {
            // PORT: Go nil options become the default options.
            compiler_options: Rc::new(parsed_config.options.unwrap_or_default()),
            type_acquisition: parsed_config.type_acquisition,
            file_names,
            project_references,
            content_mappers: content_mappers.into_iter().map(Rc::new).collect(),
        },
        config_file: source_file.map(Rc::new),
        raw: parsed_config.raw,
        errors,
        compile_on_save: Some(compile_on_save),

        compare_paths_options: ComparePathsOptions {
            use_case_sensitive_file_names: host.fs().use_case_sensitive_file_names(),
            current_directory: base_path_for_file_names,
        },
        literal_file_names_len,
        ..Default::default()
    }
}

// Go: tsoptions/tsconfigparsing.go:1435 canJsonReportNoInputFiles
fn can_json_report_no_input_files(raw_config: &IndexMap<String, CompilerOptionsValue>) -> bool {
    let files_exists = raw_config.contains_key("files");
    let references_exists = raw_config.contains_key("references");
    !files_exists && !references_exists
}

// Go: tsoptions/tsconfigparsing.go:1441 shouldReportNoInputFiles
fn should_report_no_input_files(
    file_names: &[String],
    can_json_report_no_input_files: bool,
    resolution_stack: &[Path],
) -> bool {
    file_names.is_empty() && can_json_report_no_input_files && resolution_stack.is_empty()
}

// Go: tsoptions/tsconfigparsing.go:1445 validateSpecs
// PORT: Go `specs any` always holds a `[]any`, so it is a slice here.
fn validate_specs(
    specs: &[CompilerOptionsValue],
    disallow_trailing_recursion: bool,
    json_source_file: Node,
    spec_key: &str,
) -> (Vec<String>, Vec<Diagnostic>) {
    let create_diagnostic = |message: &'static Message, spec: &str| -> Diagnostic {
        let element = get_tsconfig_prop_array_element_value(json_source_file, spec_key, spec);
        create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
            json_source_file,
            element,
            message,
            args![spec],
        )
    };
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut final_specs: Vec<String> = Vec::new();
    for value in specs {
        let CompilerOptionsValue::String(spec) = value else {
            continue;
        };
        let diag = spec_to_diagnostic(spec, disallow_trailing_recursion);
        if let Some(diag) = diag {
            errors.push(create_diagnostic(diag, spec));
        } else {
            final_specs.push(spec.clone());
        }
    }
    (final_specs, errors)
}

// Go: tsoptions/tsconfigparsing.go:1471 specToDiagnostic
pub(crate) fn spec_to_diagnostic(
    spec: &str,
    disallow_trailing_recursion: bool,
) -> Option<&'static Message> {
    if disallow_trailing_recursion && invalid_trailing_recursion(spec) {
        return Some(diag::File_specification_cannot_end_in_a_recursive_directory_wildcard_Asterisk_Asterisk_Colon_0);
    }
    if invalid_dot_dot_after_recursive_wildcard(spec) {
        return Some(
            diag::File_specification_cannot_contain_a_parent_directory_that_appears_after_a_recursive_directory_wildcard_Asterisk_Asterisk_Colon_0,
        );
    }
    None
}

// Go: tsoptions/tsconfigparsing.go:1481 invalidTrailingRecursion
fn invalid_trailing_recursion(spec: &str) -> bool {
    // Matches **, /**, **/, and /**/, but not a**b.
    // Strip optional trailing slash, then check if it ends with /** or is just **
    let s = spec.strip_suffix('/').unwrap_or(spec);
    s == "**" || s.ends_with("/**")
}

// Go: tsoptions/tsconfigparsing.go:1488 invalidDotDotAfterRecursiveWildcard
// PORT: Go string indexes are byte offsets, as are Rust `find` results.
fn invalid_dot_dot_after_recursive_wildcard(s: &str) -> bool {
    // We used to use the regex /(^|\/)\*\*\/(.*\/)?\.\.($|\/)/ to check for this case, but
    // in v8, that has polynomial performance because the recursive wildcard match - **/ -
    // can be matched in many arbitrary positions when multiple are present, resulting
    // in bad backtracking (and we don't care which is matched - just that some /.. segment
    // comes after some **/ segment).
    let wildcard_index: i64 = if s.starts_with("**/") {
        0
    } else {
        s.find("/**/").map_or(-1, |i| i as i64)
    };
    if wildcard_index == -1 {
        return false;
    }
    let last_dot_index: i64 = if s.ends_with("/..") {
        s.len() as i64
    } else {
        s.rfind("/../").map_or(-1, |i| i as i64)
    };
    last_dot_index > wildcard_index
}

// Go: tsoptions/tsconfigparsing.go:1512 GetTsConfigPropArrayElementValue
// PORT: Go returns `*ast.StringLiteral`; that is a `Node` (`NIL` for nil).
pub fn get_tsconfig_prop_array_element_value(
    tsconfig_source_file: Node,
    prop_key: &str,
    element_value: &str,
) -> Node {
    let callback = get_callback_for_finding_property_assignment_by_value(element_value);
    for_each_tsconfig_prop_array(tsconfig_source_file, prop_key, |property| {
        callback(property)
    })
    .unwrap_or(Node::NIL)
}

// Go: tsoptions/tsconfigparsing.go:1522 ForEachTsConfigPropArray
// PORT: Go `*T` results are `Option<T>`.
pub fn for_each_tsconfig_prop_array<T>(
    tsconfig_source_file: Node,
    prop_key: &str,
    callback: impl FnMut(Node) -> Option<T>,
) -> Option<T> {
    if tsconfig_source_file.is_some() {
        return for_each_property_assignment(
            get_tsconfig_object_literal_expression(tsconfig_source_file),
            prop_key,
            callback,
            &[],
        );
    }
    None
}

// Go: tsoptions/tsconfigparsing.go:1529 CreateDiagnosticAtReferenceSyntax
// PORT: Go returns a nilable `*ast.Diagnostic`; that is `Option`. Go reads
// `config.ConfigFile.SourceFile`, which panics for a nil `ConfigFile`.
pub fn create_diagnostic_at_reference_syntax(
    config: &ParsedCommandLine,
    index: usize,
    message: &'static Message,
    args: Vec<String>,
) -> Option<Diagnostic> {
    let source_file = config
        .config_file
        .as_ref()
        .expect("nil pointer dereference: config.ConfigFile")
        .source_file;
    for_each_tsconfig_prop_array(source_file, "references", |property| {
        if is_array_literal_expression(property.initializer()) {
            let value = property.initializer().elements();
            if value.len() > index {
                return Some(create_diagnostic_for_node_in_source_file(
                    source_file,
                    value.get(index),
                    message,
                    args.clone(),
                ));
            }
        }
        None
    })
}

// Go: tsoptions/tsconfigparsing.go:1544 createDiagnosticAtProjectReferenceProperty
// PORT: Go `*TsConfigSourceFile` is `Option<&TsConfigSourceFile>`; the Go
// variadic `args` is a `Vec`.
fn create_diagnostic_at_project_reference_property(
    source_file: Option<&TsConfigSourceFile>,
    index: usize,
    property_name: &str,
    message: &'static Message,
    args: Vec<String>,
) -> Diagnostic {
    let mut node = Node::NIL;
    if let Some(source_file) = source_file {
        node = for_each_tsconfig_prop_array(source_file.source_file, "references", |property| {
            if is_array_literal_expression(property.initializer()) {
                let elements = property.initializer().elements();
                if elements.len() > index && is_object_literal_expression(elements.get(index)) {
                    if let Some(property_node) = for_each_property_assignment(
                        elements.get(index),
                        property_name,
                        |property| Some(property.initializer()),
                        &[],
                    ) {
                        return Some(property_node);
                    }
                    return Some(elements.get(index));
                }
            }
            None
        })
        .unwrap_or(Node::NIL);
    }
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        tsconfig_to_source_file(source_file),
        node,
        message,
        args,
    )
}

// Go: tsoptions/tsconfigparsing.go:1565 GetCallbackForFindingPropertyAssignmentByValue
pub fn get_callback_for_finding_property_assignment_by_value(
    value: &str,
) -> impl Fn(Node) -> Option<Node> + use<> {
    let value = value.to_string();
    move |property: Node| -> Option<Node> {
        if is_array_literal_expression(property.initializer()) {
            return property
                .initializer()
                .elements()
                .iter()
                .find(|element| is_string_literal(*element) && element.text() == value);
        }
        None
    }
}

// Go: tsoptions/tsconfigparsing.go:1576 GetOptionsSyntaxByArrayElementValue
pub fn get_options_syntax_by_array_element_value(
    object_literal: Node,
    prop_key: &str,
    element_value: &str,
) -> Node {
    for_each_property_assignment(
        object_literal,
        prop_key,
        get_callback_for_finding_property_assignment_by_value(element_value),
        &[],
    )
    .unwrap_or(Node::NIL)
}

// Go: tsoptions/tsconfigparsing.go:1584 getContentMapperSyntax (tsgo#4712)
// getContentMapperSyntax returns the tsconfig JSON node to attribute a diagnostic about the content
// mapper at index to: the value of subKey within that mapper's object (when subKey is non-empty),
// falling back to the mapper element, then to the "contentMappers" array. An index outside the array
// (e.g. -1) yields the array itself. Returns nil when there is no source file (JSON API).
// PORT: Go nil `*ast.Node` is `Node::NIL`.
fn get_content_mapper_syntax(source_file: Node, index: i32, sub_key: &str) -> Node {
    if source_file.is_nil() {
        return Node::NIL;
    }
    for_each_tsconfig_prop_array(source_file, "contentMappers", |property| {
        if !is_array_literal_expression(property.initializer()) {
            return Some(property.initializer());
        }
        let elements = property.initializer().elements();
        if index < 0 || index as usize >= elements.len() {
            return Some(property.initializer());
        }
        let element = elements.get(index as usize);
        if !sub_key.is_empty() && is_object_literal_expression(element) {
            let node = for_each_property_assignment(
                element,
                sub_key,
                |property| Some(property.initializer()),
                &[],
            );
            if let Some(node) = node
                && node.is_some()
            {
                return Some(node);
            }
        }
        Some(element)
    })
    .unwrap_or(Node::NIL)
}

// Go: tsoptions/tsconfigparsing.go:1608 GetContentMapperOptionDiagnosticLocation (tsgo#4712)
// PORT: Go returns `(*ast.SourceFile, core.TextRange)`; the file is a
// `Node` (`Node::NIL` for nil). Go `slices.Index` compares the mapper
// pointers (`Rc::ptr_eq`). Go indexes the array with a negative
// `segment.Index` and panics; here a negative index finds no element.
pub fn get_content_mapper_option_diagnostic_location(
    config: Option<&ParsedCommandLine>,
    mapper: &Rc<Mapper>,
    path: &[OptionPathSegment],
) -> (Node, TextRange) {
    let Some(config) = config else {
        return (Node::NIL, TextRange::undefined());
    };
    let Some(config_file) = &config.config_file else {
        return (Node::NIL, TextRange::undefined());
    };
    let index = config
        .content_mappers()
        .iter()
        .position(|m| Rc::ptr_eq(m, mapper))
        .map_or(-1, |i| i as i32);
    let mapper_node = get_content_mapper_syntax(config_file.source_file, index, "");
    let mut node = get_content_mapper_syntax(config_file.source_file, index, "options");
    if node.is_nil() {
        node = mapper_node;
    }
    for segment in path {
        let mut next = Node::NIL;
        if segment.is_index && is_array_literal_expression(node) {
            let elements = node.elements();
            if let Ok(i) = usize::try_from(segment.index)
                && i < elements.len()
            {
                next = elements.get(i);
            }
        } else if !segment.is_index && is_object_literal_expression(node) {
            next = for_each_property_assignment(
                node,
                &segment.property,
                |property| Some(property.initializer()),
                &[],
            )
            .unwrap_or(Node::NIL);
        }
        if next.is_nil() {
            break;
        }
        node = next;
    }
    if node.is_nil() {
        return (Node::NIL, TextRange::undefined());
    }
    let file = config_file.source_file;
    (
        file,
        TextRange::new(skip_trivia(&source_file_text(file), node.pos()), node.end()),
    )
}

// Go: tsoptions/tsconfigparsing.go:1645 getContentMappersKeySyntax (tsgo#4712)
// getContentMappersKeySyntax returns the "contentMappers" property key node, used to attribute a
// diagnostic about the setting as a whole rather than a specific mapper.
fn get_content_mappers_key_syntax(source_file: Node) -> Node {
    if source_file.is_nil() {
        return Node::NIL;
    }
    for_each_tsconfig_prop_array(source_file, "contentMappers", |property| {
        Some(property.name())
    })
    .unwrap_or(Node::NIL)
}

// Go: tsoptions/tsconfigparsing.go:1656 getContentMapperExtensionSyntax (tsgo#4712)
// getContentMapperExtensionSyntax returns the node for a specific extension string within the content
// mapper at index, falling back to the "extensions" array or the mapper element.
fn get_content_mapper_extension_syntax(source_file: Node, index: i32, ext: &str) -> Node {
    let node = get_content_mapper_syntax(source_file, index, "extensions");
    if node.is_some() && is_array_literal_expression(node) {
        if let Some(element) = node
            .elements()
            .iter()
            .find(|element| is_string_literal(*element) && element.text() == ext)
        {
            return element;
        }
    }
    node
}

// Go: tsoptions/tsconfigparsing.go:1671 setContentMapperDiagnosticLocation (tsgo#4712)
// setContentMapperDiagnosticLocation attaches a source location to a content mapper diagnostic when a
// tsconfig source file and node are available (the jsonSourceFile API), leaving it as a location-less
// compiler diagnostic otherwise (the JSON API).
fn set_content_mapper_diagnostic_location(
    mut diagnostic: Diagnostic,
    source_file: Node,
    node: Node,
) -> Diagnostic {
    if source_file.is_some() && node.is_some() {
        diagnostic.set_file(source_file);
        diagnostic.set_location(TextRange::new(
            skip_trivia(&source_file_text(source_file), node.pos()),
            node.end(),
        ));
    }
    diagnostic
}

// Go: tsoptions/tsconfigparsing.go:1679 ForEachPropertyAssignment
// PORT: Go `*T` results are `Option<T>`. The Go variadic `key2` is a slice.
pub fn for_each_property_assignment<T>(
    object_literal: Node,
    key: &str,
    mut callback: impl FnMut(Node) -> Option<T>,
    key2: &[&str],
) -> Option<T> {
    if object_literal.is_some() {
        for property in object_literal.properties().iter() {
            if !is_property_assignment(property) {
                continue;
            }
            let (prop_name, ok) = try_get_text_of_property_name(property.name());
            if ok && (prop_name == key || (!key2.is_empty() && key2[0] == prop_name)) {
                return callback(property);
            }
        }
    }
    None
}

// Go: tsoptions/tsconfigparsing.go:1695 getTsConfigObjectLiteralExpression
fn get_tsconfig_object_literal_expression(tsconfig_source_file: Node) -> Node {
    if tsconfig_source_file.is_some() {
        let statements = tsconfig_source_file.statements();
        if !statements.is_empty() {
            let expression = statements.get(0).expression();
            if is_object_literal_expression(expression) {
                return expression;
            }
        }
    }
    Node::NIL
}

// Go: tsoptions/tsconfigparsing.go:1705 getSubstitutedPathWithConfigDirTemplate
pub(crate) fn get_substituted_path_with_config_dir_template(
    value: &str,
    base_path: &str,
) -> String {
    get_normalized_absolute_path(&value.replacen(CONFIG_DIR_TEMPLATE, "./", 1), base_path)
}

// Go: tsoptions/tsconfigparsing.go:1710 getSubstitutedStringArrayWithConfigDirTemplate
// PORT: Go returns a nil slice for "no change"; that is `None`. Go passes
// the string as `any` to `startsWithConfigDirTemplate`.
fn get_substituted_string_array_with_config_dir_template(
    list: &[String],
    base_path: &str,
) -> Option<Vec<String>> {
    let mut result: Option<Vec<String>> = None;
    for (i, element) in list.iter().enumerate() {
        if starts_with_config_dir_template(&CompilerOptionsValue::String(element.clone())) {
            let result = result.get_or_insert_with(|| list.to_vec());
            result[i] = get_substituted_path_with_config_dir_template(element, base_path);
        }
    }
    result
}

// Go: tsoptions/options_generated.go:1332 handleOptionConfigDirTemplateSubstitution (ts#64457 generates it)
// PORT: Go clones the shared `Paths` map before the first change (tsgo#4362)
// so a cached extended config keeps its value. Each options value here owns
// its `paths`, so no clone is needed.
fn handle_option_config_dir_template_substitution(
    compiler_options: Option<&mut CompilerOptions>,
    base_path: &str,
) {
    let Some(compiler_options) = compiler_options else {
        return;
    };

    // !!! don't hardcode this; use options declarations?

    if let Some(paths) = compiler_options.paths.as_mut() {
        for v in paths.values_mut() {
            // Go ranges over a nil slice as zero items and returns nil.
            if let Some(substitution) = get_substituted_string_array_with_config_dir_template(
                v.as_deref().unwrap_or_default(),
                base_path,
            ) {
                *v = Some(substitution);
            }
        }
    }

    if let Some(root_dirs) = compiler_options.root_dirs.as_deref()
        && let Some(root_dirs) =
            get_substituted_string_array_with_config_dir_template(root_dirs, base_path)
    {
        compiler_options.root_dirs = Some(root_dirs);
    }
    if let Some(type_roots) = compiler_options.type_roots.as_deref()
        && let Some(type_roots) =
            get_substituted_string_array_with_config_dir_template(type_roots, base_path)
    {
        compiler_options.type_roots = Some(type_roots);
    }
    macro_rules! substitute_string_fields {
        ($($field:ident),*) => {
            $(
                if starts_with_config_dir_template(&CompilerOptionsValue::String(compiler_options.$field.clone())) {
                    compiler_options.$field = get_substituted_path_with_config_dir_template(&compiler_options.$field, base_path);
                }
            )*
        };
    }
    substitute_string_fields!(
        generate_cpu_profile,
        generate_trace,
        out_file,
        out_dir,
        root_dir,
        ts_build_info_file,
        base_url,
        declaration_dir
    );
}

/// PORT: not in Go (perf). The extension group of one file and the
/// `ChangeExtension` results of that file, as the two extension priority
/// checks below read them. Go computes the group in each check and makes a
/// new string for each changed extension; here the group is computed once
/// per file and each changed name is built in one reused buffer, with the
/// same text.
struct ExtensionPriority<'e> {
    /// The extension groups of the supported extensions.
    groups: &'e [Vec<&'e str>],
    /// The extensions of the groups that contain the file's extension, in
    /// order (Go `extensionGroup`).
    group: Vec<&'e str>,
    /// `ChangeExtension(file, ext)` of the last `changed` call.
    changed: String,
    /// `ChangeExtension(file, "")`, made at the first `changed` call.
    stem: Option<String>,
}

impl<'e> ExtensionPriority<'e> {
    fn new(groups: &'e [Vec<&'e str>]) -> Self {
        ExtensionPriority {
            groups,
            group: Vec::new(),
            changed: String::new(),
            stem: None,
        }
    }

    /// Starts on `file`: its extension group.
    fn start(&mut self, file: &str) {
        self.group.clear();
        self.stem = None;
        for group in self.groups {
            if file_extension_is_one_of(file, group) {
                self.group.extend_from_slice(group);
            }
        }
    }

    /// Go `tspath.ChangeExtension(file, ext)` for a non-empty `ext`.
    // PORT: `change_extension(file, "")` is the file name without the
    // extension that `ChangeExtension` removes, or the file name when it
    // removes none; then `ChangeExtension` returns the file name unchanged.
    fn changed(&mut self, file: &str, ext: &str) -> &str {
        let stem = self.stem.get_or_insert_with(|| change_extension(file, ""));
        self.changed.clear();
        if stem.len() == file.len() {
            self.changed.push_str(file);
        } else {
            self.changed.push_str(stem);
            if !ext.starts_with('.') {
                self.changed.push('.');
            }
            self.changed.push_str(ext);
        }
        &self.changed
    }
}

// hasFileWithHigherPriorityExtension determines whether a literal or wildcard file has already been included that has a higher extension priority.
// file is the path to the file.
// Go: tsoptions/tsconfigparsing.go:1728 hasFileWithHigherPriorityExtension
// PORT: `priority` has the file's extension group (`ExtensionPriority::start`).
fn has_file_with_higher_priority_extension(
    file: &str,
    priority: &mut ExtensionPriority<'_>,
    has_file: impl Fn(&str) -> bool,
) -> bool {
    if priority.group.is_empty() {
        return false;
    }
    for index in 0..priority.group.len() {
        let ext = priority.group[index];
        // d.ts files match with .ts extension and with case sensitive sorting the file order for same files with ts tsx and dts extension is
        // d.ts, .ts, .tsx in that order so we need to handle tsx and dts of same same name case here and in remove files with same extensions
        // So dont match .d.ts files with .ts extension
        if file_extension_is(file, ext)
            && (ext != EXTENSION_TS || !file_extension_is(file, EXTENSION_DTS))
        {
            return false;
        }
        if has_file(priority.changed(file, ext)) {
            if ext == EXTENSION_DTS
                && (file_extension_is(file, EXTENSION_JS) || file_extension_is(file, EXTENSION_JSX))
            {
                // LEGACY BEHAVIOR: An off-by-one bug somewhere in the extension priority system for wildcard module loading allowed declaration
                // files to be loaded alongside their js(x) counterparts. We regard this as generally undesirable, but retain the behavior to
                // prevent breakage.
                continue;
            }
            return true;
        }
    }
    false
}

// Removes files included via wildcard expansion with a lower extension priority that have already been included.
// file is the path to the file.
// Go: tsoptions/tsconfigparsing.go:1760 removeWildcardFilesWithLowerPriorityExtension
// PORT: `priority` has the file's extension group (`ExtensionPriority::start`).
fn remove_wildcard_files_with_lower_priority_extension(
    file: &str,
    wildcard_files: &mut crate::core::FxIndexMap<String, String>,
    priority: &mut ExtensionPriority<'_>,
    use_case_sensitive_file_names: bool,
) {
    if priority.group.is_empty() {
        return;
    }
    for index in (0..priority.group.len()).rev() {
        let ext = priority.group[index];
        if file_extension_is(file, ext) {
            return;
        }
        let changed = priority.changed(file, ext);
        if use_case_sensitive_file_names {
            wildcard_files.shift_remove(changed);
        } else {
            wildcard_files.shift_remove(&get_canonical_file_name(changed, false));
        }
    }
}

// getFileNamesFromConfigSpecs gets the file names from the provided config file specs that contain, files, include, exclude and
// other properties needed to resolve the file names
// configFileSpecs is the config file specs extracted with file names to include, wildcards to include/exclude and other details
// basePath is the base path for any relative file specifications.
// options is the Compiler options.
// host is the host used to resolve files and directories.
// extraExtensions are additional file extensions (e.g. from content mappers) to treat as supported.
// Go: tsoptions/tsconfigparsing.go:1787 getFileNamesFromConfigSpecs
// PORT: Go `options` can be nil only after a config cycle; Go
// `GetSupportedExtensions` then panics on the nil pointer, and so does this
// port.
pub(crate) fn get_file_names_from_config_specs(
    config_file_specs: &ConfigFileSpecs,
    base_path: &str, // considering this is the current directory
    options: Option<&CompilerOptions>,
    host: &dyn Fs,
    extra_extensions: &[String],
) -> (Vec<String>, i32) {
    let base_path = normalize_path(base_path);
    let key_mappper = |value: &str| -> String {
        get_canonical_file_name(value, host.use_case_sensitive_file_names())
    };
    // Literal file names (provided via the "files" array in tsconfig.json) are stored in a
    // file map with a possibly case insensitive key. We use this map later when when including
    // wildcard paths.
    // PERF (cfgwalk1): the maps keep the insertion order (Go
    // `collections.OrderedMap`), so their hasher does not change the result.
    let mut literal_file_map: crate::core::FxIndexMap<String, String> = Default::default();
    // Wildcard paths (provided via the "includes" array in tsconfig.json) are stored in a
    // file map with a possibly case insensitive key. We use this map to store paths matched
    // via wildcard, and to handle extension priority.
    let mut wildcard_file_map: crate::core::FxIndexMap<String, String> = Default::default();
    // Wildcard paths of json files (provided via the "includes" array in tsconfig.json) are stored in a
    // file map with a possibly case insensitive key. We use this map to store paths matched
    // via wildcard of *.json kind
    let mut wild_card_json_file_map: crate::core::FxIndexMap<String, String> = Default::default();
    let validated_files_spec = &config_file_specs.validated_files_spec;
    let validated_include_specs = &config_file_specs.validated_include_specs;
    let validated_exclude_specs = &config_file_specs.validated_exclude_specs;
    // Rather than re-query this for each file and filespec, we query the supported extensions
    // once and store it on the expansion context.
    let supported_extensions = get_supported_extensions(
        options.expect("nil pointer dereference: options"),
        extra_extensions,
    );
    let supported_extensions_with_json_if_resolve_json_module =
        get_supported_extensions_with_json_if_resolve_json_module(
            options,
            supported_extensions.clone(),
        );
    // Literal files are always included verbatim. An "include" or "exclude" specification cannot
    // remove a literal file.
    // ts#64159 (tsconfigparsing.go:1820): the key is the PathKey of the
    // resolved file name (validatedFileNames), not the spec text, so
    // "./src/a.ts" and "src//a.ts" are one root file, and "include" does not
    // add a "files" entry again.
    for file_name in validated_files_spec {
        let file = get_normalized_absolute_path(file_name, &base_path);
        literal_file_map.insert(key_mappper(&file), file);
    }

    let mut json_only_include_matchers: Option<SpecMatcher> = None;
    let use_case_sensitive_file_names = host.use_case_sensitive_file_names();
    let extension_groups: Vec<Vec<&str>> = supported_extensions
        .iter()
        .map(|group| group.iter().map(String::as_str).collect())
        .collect();
    let mut priority = ExtensionPriority::new(&extension_groups);
    if !validated_include_specs.is_empty() {
        let flat_extensions: Vec<String> = supported_extensions_with_json_if_resolve_json_module
            .iter()
            .flatten()
            .cloned()
            .collect();
        let files = read_directory(
            host,
            &base_path,
            &base_path,
            &flat_extensions,
            validated_exclude_specs,
            validated_include_specs,
            UNLIMITED_DEPTH,
        );
        for file in &files {
            if file_extension_is(file, EXTENSION_JSON) {
                if json_only_include_matchers.is_none() {
                    let includes: Vec<String> = validated_include_specs
                        .iter()
                        .filter(|include| include.ends_with(EXTENSION_JSON))
                        .cloned()
                        .collect();
                    json_only_include_matchers = new_spec_matcher(
                        &includes,
                        &base_path,
                        Usage::Files,
                        host.use_case_sensitive_file_names(),
                    );
                }
                let mut include_index: i32 = -1;
                if let Some(matchers) = &json_only_include_matchers {
                    include_index = matchers.match_index(file);
                }
                if include_index != -1 {
                    let key = key_mappper(file);
                    if !literal_file_map.contains_key(&key)
                        && !wild_card_json_file_map.contains_key(&key)
                    {
                        wild_card_json_file_map.insert(key, file.clone());
                    }
                }
                continue;
            }
            // If we have already included a literal or wildcard path with a
            // higher priority extension, we should skip this file.
            //
            // This handles cases where we may encounter both <file>.ts and
            // <file>.d.ts (or <file>.js if "allowJs" is enabled) in the same
            // directory when they are compilation outputs.
            priority.start(file);
            if has_file_with_higher_priority_extension(file, &mut priority, |file_name| {
                let has = |key: &str| {
                    literal_file_map.contains_key(key) || wildcard_file_map.contains_key(key)
                };
                if use_case_sensitive_file_names {
                    has(file_name)
                } else {
                    has(&key_mappper(file_name))
                }
            }) {
                continue;
            }
            // We may have included a wildcard path with a lower priority
            // extension due to the user-defined order of entries in the
            // "include" array. If there is a lower priority extension in the
            // same directory, we should remove it.
            remove_wildcard_files_with_lower_priority_extension(
                file,
                &mut wildcard_file_map,
                &mut priority,
                use_case_sensitive_file_names,
            );
            let key = key_mappper(file);
            if !literal_file_map.contains_key(&key) && !wildcard_file_map.contains_key(&key) {
                wildcard_file_map.insert(key, file.clone());
            }
        }
    }
    let mut files: Vec<String> = Vec::with_capacity(
        literal_file_map.len() + wildcard_file_map.len() + wild_card_json_file_map.len(),
    );
    files.extend(literal_file_map.values().cloned());
    files.extend(wildcard_file_map.values().cloned());
    files.extend(wild_card_json_file_map.values().cloned());
    (files, literal_file_map.len() as i32)
}

/// Go `[][]string` copy of a tspath extension table.
fn owned_groups(groups: &[&[&str]]) -> Vec<Vec<String>> {
    groups
        .iter()
        .map(|group| group.iter().map(|ext| (*ext).to_string()).collect())
        .collect()
}

// Go: tsoptions/tsconfigparsing.go:1882 GetSupportedExtensions
// PORT: Go returns the shared tspath tables; this returns owned copies.
// tsgo#4712: the extra extensions are plain extension strings.
pub fn get_supported_extensions(
    compiler_options: &CompilerOptions,
    extra_extensions: &[String],
) -> Vec<Vec<String>> {
    let need_js_extensions = compiler_options.get_allow_js();
    let builtins = if need_js_extensions {
        owned_groups(ALL_SUPPORTED_EXTENSIONS)
    } else {
        owned_groups(SUPPORTED_TS_EXTENSIONS)
    };
    if extra_extensions.is_empty() {
        return builtins;
    }
    let flat_builtins: Vec<&String> = builtins.iter().flatten().collect();
    let mut result: Vec<Vec<String>> = Vec::new();
    for ext in extra_extensions {
        if !flat_builtins.contains(&ext) {
            result.push(vec![ext.clone()]);
        }
    }
    if result.is_empty() {
        return builtins;
    }
    let mut extensions = builtins.clone();
    extensions.extend(result);
    extensions
}

// Go: tsoptions/tsconfigparsing.go:1906 GetSupportedExtensionsWithJsonIfResolveJsonModule
// PORT: Go `core.Same` compares slice identity. Content equality gives the
// same result here: a new Go slice equal to a tspath table gets `.json`
// appended, which equals the matching `WITH_JSON` table.
pub fn get_supported_extensions_with_json_if_resolve_json_module(
    compiler_options: Option<&CompilerOptions>,
    supported_extensions: Vec<Vec<String>>,
) -> Vec<Vec<String>> {
    let Some(compiler_options) = compiler_options else {
        return supported_extensions;
    };
    if !compiler_options.get_resolve_json_module() {
        return supported_extensions;
    }
    if supported_extensions == owned_groups(ALL_SUPPORTED_EXTENSIONS) {
        return owned_groups(ALL_SUPPORTED_EXTENSIONS_WITH_JSON);
    }
    if supported_extensions == owned_groups(SUPPORTED_TS_EXTENSIONS) {
        return owned_groups(SUPPORTED_TS_EXTENSIONS_WITH_JSON);
    }
    let mut result = supported_extensions;
    result.push(vec![EXTENSION_JSON.to_string()]);
    result
}

// Reads the config file and reports errors.
// Go: tsoptions/tsconfigparsing.go:1920 GetParsedCommandLineOfConfigFile
// PORT: Go returns a nilable `*ParsedCommandLine`; that is `Option`.
pub fn get_parsed_command_line_of_config_file(
    config_file_name: &str,
    options: Option<&CompilerOptions>,
    options_raw: Option<&IndexMap<String, CompilerOptionsValue>>,
    sys: &dyn ParseConfigHost,
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> (Option<ParsedCommandLine>, Vec<Diagnostic>) {
    let config_file_name =
        get_normalized_absolute_path(config_file_name, &sys.get_current_directory());
    let path = to_path(
        &config_file_name,
        &sys.get_current_directory(),
        sys.fs().use_case_sensitive_file_names(),
    );
    get_parsed_command_line_of_config_file_path(
        &config_file_name,
        path,
        options,
        options_raw,
        sys,
        extended_config_cache,
    )
}

// Go: tsoptions/tsconfigparsing.go:1930 GetParsedCommandLineOfConfigFilePath
pub fn get_parsed_command_line_of_config_file_path(
    config_file_name: &str,
    path: Path,
    options: Option<&CompilerOptions>,
    options_raw: Option<&IndexMap<String, CompilerOptionsValue>>,
    sys: &dyn ParseConfigHost,
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> (Option<ParsedCommandLine>, Vec<Diagnostic>) {
    let errors: Vec<Diagnostic> = Vec::new();
    let (config_file_text, errors) = try_read_file(
        config_file_name,
        &mut |name: &str| sys.fs().read_file(name),
        errors,
    );
    if !errors.is_empty() {
        // these are unrecoverable errors--exit to report them as diagnostics
        return (None, errors);
    }

    let ts_config_source_file =
        new_tsconfig_source_file_from_file_path(config_file_name, path, &config_file_text);
    // tsConfigSourceFile.resolvedPath = tsConfigSourceFile.FileName()
    // tsConfigSourceFile.originalFileName = tsConfigSourceFile.FileName()
    (
        Some(parse_json_source_file_config_file_content(
            ts_config_source_file,
            sys,
            &get_directory_path(config_file_name),
            options,
            options_raw,
            config_file_name,
            &[],
            extended_config_cache,
        )),
        Vec::new(),
    )
}

#[cfg(test)]
mod extension_priority_tests {
    use super::*;

    // `ExtensionPriority::changed` is `change_extension`, and `start` finds
    // the groups that Go's checks find.
    #[test]
    fn changed_matches_change_extension() {
        let groups: Vec<Vec<&str>> = vec![
            vec![".ts", ".tsx", ".d.ts"],
            vec![".cts", ".d.cts"],
            vec![".mts", ".d.mts"],
            vec![".js", ".jsx"],
            vec![".vue"],
        ];
        let mut priority = ExtensionPriority::new(&groups);
        for file in [
            "/a/b.ts",
            "/a/b.d.ts",
            "/a/b.tsx",
            "/a/b.d.mts",
            "/a/b.js",
            "/a.dir/b.c.jsx",
            "/a/b.json",
            "/a/b.vue",
            "/a/b",
            "/a/.ts",
        ] {
            priority.start(file);
            let expected: Vec<&str> = groups
                .iter()
                .filter(|group| file_extension_is_one_of(file, group))
                .flatten()
                .copied()
                .collect();
            assert_eq!(priority.group, expected, "{file}");
            for ext in [".ts", ".d.ts", ".tsx", ".d.mts", ".js", "vue"] {
                assert_eq!(
                    priority.changed(file, ext),
                    change_extension(file, ext),
                    "{file} {ext}"
                );
            }
        }
    }
}

#[cfg(test)]
mod top_level_array_tests {
    use super::*;
    use crate::frontend::vfs::osvfs_fs;

    struct Host;

    impl ParseConfigHost for Host {
        fn fs(&self) -> Rc<dyn Fs> {
            osvfs_fs()
        }
        fn get_current_directory(&self) -> String {
            "/p".to_string()
        }
    }

    // In Go (tsconfigparsing.go:1300) a top-level array that holds
    // `"files": []` has no `files` node, so CreateDiagnosticForNodeInSourceFile
    // (errors.go:93) dereferences nil. Go N panics with the runtime text and
    // exits 2 (projfuzz1 GO_CRASH_ARR).
    #[test]
    fn empty_files_in_a_top_level_array_panics_like_go() {
        let source_file = new_tsconfig_source_file_from_file_path(
            "/p/tsconfig.json",
            to_path("/p/tsconfig.json", "", true),
            r#"[{"files": []}]"#,
        );
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parse_json_source_file_config_file_content(
                source_file,
                &Host,
                "/p",
                None,
                None,
                "/p/tsconfig.json",
                &[],
                None,
            )
        }))
        .expect_err("Go N panics");
        let panic = payload
            .downcast_ref::<crate::core::GoPanic>()
            .expect("a Go panic, not a port panic");
        assert_eq!(
            panic.message,
            "runtime error: invalid memory address or nil pointer dereference"
        );
    }
}
