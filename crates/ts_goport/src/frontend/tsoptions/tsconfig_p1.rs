use crate::frontend::prelude::*;
use std::sync::LazyLock;

// This file ports tsoptions/tsconfigparsing.go lines 1 to 900.
// PORT: Go `any` values are `CompilerOptionsValue`. Go
// `*collections.OrderedMap[string, any]` is `IndexMap<String,
// CompilerOptionsValue>` (the `Map` variant). Go `[]*ast.Diagnostic` is
// `Vec<Diagnostic>`. Go `*ast.SourceFile` is the source file `Node`
// (`Node::NIL` is Go nil).

/// Leaks a declaration so it has the Go pointer lifetime.
fn leak_option(o: CommandLineOption) -> &'static CommandLineOption {
    Box::leak(Box::new(o))
}

// Go: tsoptions/tsconfigparsing.go:26 extendsResult
// PORT: Go `collections.Set[string]` is `FxHashSet<String>`. Readers sort
// the keys (`core.InsertSorted`), so order does not matter.
#[derive(Clone, Debug, Default)]
pub struct ExtendsResult {
    pub options: CompilerOptions,
    pub include: Option<Vec<CompilerOptionsValue>>,
    pub exclude: Option<Vec<CompilerOptionsValue>>,
    pub files: Option<Vec<CompilerOptionsValue>>,
    // tsgo#4712
    pub content_mappers: Option<Vec<CompilerOptionsValue>>,
    pub compile_on_save: bool,
    pub extended_source_files: FxHashSet<String>,
}

// Go: tsoptions/declarations_generated.go:1112 compilerOptionsDeclaration (ts#64457 generates it)
// PORT: Go package-level vars are `LazyLock` statics of leaked
// declarations, so Go pointer identity is `std::ptr::eq`.
pub static COMPILER_OPTIONS_DECLARATION: LazyLock<&'static CommandLineOption> =
    LazyLock::new(|| {
        leak_option(CommandLineOption {
            name: "compilerOptions",
            kind: CommandLineOptionKind::OBJECT,
            element_options: COMMAND_LINE_COMPILER_OPTIONS_MAP.clone(),
            ..Default::default()
        })
    });

// Go: tsoptions/declarations_generated.go:1135 compileOnSaveCommandLineOption (ts#64457 generates it)
pub static COMPILE_ON_SAVE_COMMAND_LINE_OPTION: LazyLock<&'static CommandLineOption> =
    LazyLock::new(|| {
        leak_option(CommandLineOption {
            name: "compileOnSave",
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        })
    });

// Go: tsoptions/declarations_generated.go:1124 extendsOptionDeclaration (ts#64457 generates it)
pub static EXTENDS_OPTION_DECLARATION: LazyLock<&'static CommandLineOption> = LazyLock::new(|| {
    leak_option(CommandLineOption {
        name: "extends",
        kind: CommandLineOptionKind::LIST_OR_ELEMENT,
        category: Some(diag::File_Management),
        element_options: command_line_options_to_map(&[leak_option(CommandLineOption {
            name: "extends",
            kind: CommandLineOptionKind::STRING,
            ..Default::default()
        })]),
        ..Default::default()
    })
});

// Go: tsoptions/declarations_generated.go:1141 tsconfigRootOptionsMap (ts#64457 generates it)
pub static TSCONFIG_ROOT_OPTIONS_MAP: LazyLock<&'static CommandLineOption> = LazyLock::new(|| {
    leak_option(CommandLineOption {
        name: "undefined", // should never be needed since this is root
        kind: CommandLineOptionKind::OBJECT,
        element_options: command_line_options_to_map(&[
            *COMPILER_OPTIONS_DECLARATION,
            *TYPE_ACQUISITION_DECLARATION,
            *EXTENDS_OPTION_DECLARATION,
            leak_option(CommandLineOption {
                name: "references",
                kind: CommandLineOptionKind::LIST, // should be a list of projectReference
                // Category: diagnostics.Projects,
                ..Default::default()
            }),
            // tsgo#4712
            leak_option(CommandLineOption {
                name: "contentMappers",
                kind: CommandLineOptionKind::LIST, // list of content mapper objects
                ..Default::default()
            }),
            leak_option(CommandLineOption {
                name: "files",
                kind: CommandLineOptionKind::LIST,
                // Category: diagnostics.File_Management,
                ..Default::default()
            }),
            leak_option(CommandLineOption {
                name: "include",
                kind: CommandLineOptionKind::LIST,
                // Category: diagnostics.File_Management,
                // DefaultValueDescription: diagnostics.if_files_is_specified_otherwise_Asterisk_Asterisk_Slash_Asterisk,
                ..Default::default()
            }),
            leak_option(CommandLineOption {
                name: "exclude",
                kind: CommandLineOptionKind::LIST,
                // Category: diagnostics.File_Management,
                // DefaultValueDescription: diagnostics.Node_modules_bower_components_jspm_packages_plus_the_value_of_outDir_if_one_is_specified,
                ..Default::default()
            }),
            *COMPILE_ON_SAVE_COMMAND_LINE_OPTION,
        ]),
        ..Default::default()
    })
});

// Go: tsoptions/tsconfigparsing.go:36 configFileSpecs
#[derive(Clone, Debug, Default)]
pub struct ConfigFileSpecs {
    pub files_specs: CompilerOptionsValue,
    // Present to report errors (user specified specs), validatedIncludeSpecs are used for file name matching
    pub include_specs: CompilerOptionsValue,
    // Present to report errors (user specified specs), validatedExcludeSpecs are used for file name matching
    pub exclude_specs: CompilerOptionsValue,
    pub validated_files_spec: Vec<String>,
    pub validated_include_specs: Vec<String>,
    pub validated_exclude_specs: Vec<String>,
    pub validated_files_spec_before_substitution: Vec<String>,
    pub validated_include_specs_before_substitution: Vec<String>,
    pub is_default_include_spec: bool,
}

impl ConfigFileSpecs {
    // Go: tsoptions/tsconfigparsing.go:108 (*configFileSpecs).matchesExclude (at 673a5f17d713;
    // removed by ts#64159)
    pub fn matches_exclude(
        &self,
        file_name: &str,
        compare_paths_options: &ComparePathsOptions,
    ) -> bool {
        if self.validated_exclude_specs.is_empty() {
            return false;
        }
        let Some(exclude_matcher) = new_spec_matcher(
            &self.validated_exclude_specs,
            &compare_paths_options.current_directory,
            Usage::Exclude,
            compare_paths_options.use_case_sensitive_file_names,
        ) else {
            return false;
        };
        if exclude_matcher.match_string(file_name) {
            return true;
        }
        if !has_extension(file_name)
            && exclude_matcher.match_string(&ensure_trailing_directory_separator(file_name))
        {
            return true;
        }
        false
    }

    // Go: tsoptions/tsconfigparsing.go:52 (*configFileSpecs).getMatchedIncludeSpec
    pub fn get_matched_include_spec(
        &self,
        file_name: &str,
        compare_paths_options: &ComparePathsOptions,
    ) -> String {
        if self.validated_include_specs.is_empty() {
            return String::new();
        }
        for (index, spec) in self.validated_include_specs.iter().enumerate() {
            let include_matcher = new_spec_matcher(
                std::slice::from_ref(spec),
                &compare_paths_options.current_directory,
                Usage::Files,
                compare_paths_options.use_case_sensitive_file_names,
            );
            if let Some(include_matcher) = include_matcher
                && include_matcher.match_string(file_name)
            {
                return self.validated_include_specs_before_substitution[index].clone();
            }
        }
        String::new()
    }

    // Go: tsoptions/tsconfigparsing.go:65 (*configFileSpecs).getMatchedFileSpec
    pub fn get_matched_file_spec(
        &self,
        file_name: &str,
        compare_paths_options: &ComparePathsOptions,
    ) -> String {
        if self.validated_files_spec.is_empty() {
            return String::new();
        }
        let file_path = to_path(
            file_name,
            &compare_paths_options.current_directory,
            compare_paths_options.use_case_sensitive_file_names,
        );
        for (index, spec) in self.validated_files_spec.iter().enumerate() {
            if to_path(
                spec,
                &compare_paths_options.current_directory,
                compare_paths_options.use_case_sensitive_file_names,
            ) == file_path
            {
                return self.validated_files_spec_before_substitution[index].clone();
            }
        }
        String::new()
    }
}

// Go: tsoptions/tsconfigparsing.go:148 FileExtensionInfo
// tsgo#4712 removes it: extra extensions are plain strings now.

// Go: tsoptions/tsconfigparsing.go:69 ExtendedConfigCache
// PORT: Go returns a shared `*ExtendedConfigCacheEntry`, so this returns
// an `Rc`.
pub trait ExtendedConfigCache {
    fn get_extended_config(
        &self,
        file_name: &str,
        path: &Path,
        resolution_stack: &[Path],
        host: &dyn ParseConfigHost,
    ) -> Rc<ExtendedConfigCacheEntry>;
}

// Go: tsoptions/tsconfigparsing.go:73 ExtendedConfigCacheEntry
// PORT: Go pointers are `Option<Rc<..>>`. `ParseExtendedConfig` finishes
// the `TsConfigSourceFile` before the entry is shared, so no `RefCell`.
#[derive(Clone, Debug, Default)]
pub struct ExtendedConfigCacheEntry {
    pub extended_result: Option<Rc<TsConfigSourceFile>>,
    pub extended_config: Option<Rc<ParsedTsconfig>>,
    pub errors: Vec<Diagnostic>,
}

impl ExtendedConfigCacheEntry {
    // Go: tsoptions/tsconfigparsing.go:79 (*ExtendedConfigCacheEntry).ExtendedFileNames
    pub fn extended_file_names(&self) -> &[String] {
        if let Some(extended_result) = &self.extended_result {
            return &extended_result.extended_source_files;
        }
        &[]
    }
}

// Go: tsoptions/tsconfigparsing.go:86 parsedTsconfig
// PORT: Go `extendedConfigPath any` only ever holds a `[]string` (typed,
// so never Go nil once set) or Go nil. `Some` is "set", `None` is Go nil.
#[derive(Clone, Debug, Default)]
pub struct ParsedTsconfig {
    pub raw: CompilerOptionsValue,
    pub options: Option<CompilerOptions>,
    pub type_acquisition: Option<TypeAcquisition>,
    // Note that the case of the config path has not yet been normalized, as no files have been imported into the project yet
    pub extended_config_path: Option<Vec<String>>,
}

// Go: tsoptions/tsconfigparsing.go:105 parseOwnConfigOfJsonSourceFile
pub fn parse_own_config_of_json_source_file(
    source_file: Node,
    host: &dyn ParseConfigHost,
    base_path: &str,
    config_file_name: &str,
) -> (ParsedTsconfig, Vec<Diagnostic>) {
    let mut compiler_options = get_default_compiler_options(config_file_name);
    let mut type_acquisition = get_default_type_acquisition(config_file_name);
    let mut extended_config_path: Option<Vec<String>> = None;
    let mut root_compiler_options: Vec<Node> = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();
    let on_property_set = |key_text: &str,
                           value: &CompilerOptionsValue,
                           property_assignment: Node,
                           parent_option: Option<&'static CommandLineOption>, // TsConfigOnlyOption,
                           option: Option<&'static CommandLineOption>|
     -> Vec<Diagnostic> {
        let mut value = value;
        let converted;
        let is_extends = |o: Option<&'static CommandLineOption>| {
            o.is_some_and(|o| std::ptr::eq(o, *EXTENDS_OPTION_DECLARATION))
        };
        // Ensure value is verified except for extends which is handled in its own way for error reporting
        let mut property_set_errors: Vec<Diagnostic> = Vec::new();
        if let Some(option) = option
            && !is_extends(Some(option))
        {
            (converted, property_set_errors) = convert_json_option(
                option,
                value.clone(),
                base_path,
                property_assignment,
                property_assignment.initializer(),
                source_file,
            );
            value = &converted;
        }
        if let Some(parent_option) = parent_option
            && parent_option.name != "undefined"
            && !value.is_nil()
        {
            if let Some(option) = option
                && !option.name.is_empty()
            {
                // PORT: Go passes the `any` value; the parsers get a copy.
                let parse_diagnostics = match parent_option.name {
                    "compilerOptions" => {
                        parse_compiler_options(option.name, value.clone(), &mut compiler_options)
                    }
                    "typeAcquisition" => {
                        parse_type_acquisition(option.name, value.clone(), &mut type_acquisition)
                    }
                    _ => Vec::new(),
                };
                property_set_errors.extend(parse_diagnostics);
            } else if !key_text.is_empty()
                && let Some(unknown_name_diag) = extra_key_diagnostics(parent_option.name)
            {
                if !parent_option.element_options.is_nil() {
                    let mut possible_option = parent_option.element_options.get(key_text);
                    if possible_option.is_none() {
                        possible_option = parent_option
                            .element_options
                            .get_spelling_suggestion(key_text);
                    }
                    if let Some(possible_option) = possible_option
                        && possible_option.name != key_text
                    {
                        property_set_errors.push(create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                            source_file,
                            property_assignment.name(),
                            extra_key_did_you_mean_diagnostics(parent_option.name)
                                .expect("extraKeyDiagnostics and extraKeyDidYouMeanDiagnostics share keys"),
                            args![key_text, possible_option.name],
                        ));
                    } else {
                        property_set_errors.push(create_unknown_option_error(
                            key_text,
                            unknown_name_diag,
                            "", /*unknownOptionErrorText*/
                            property_assignment.name(),
                            source_file,
                            None, /*alternateMode*/
                            None, /*unknownDidYouMeanDiagnostic*/
                            None, /*optionsNameMap*/
                        ));
                    }
                } else {
                    // errors = append(errors, ast.NewCompilerDiagnostic(diagnostics.Unknown_compiler_option_0_Did_you_mean_1, keyText, core.FindKey(parentOption.ElementOptions, keyText)))
                }
            }
        } else if parent_option.is_some_and(|p| std::ptr::eq(p, *TSCONFIG_ROOT_OPTIONS_MAP)) {
            if is_extends(option) {
                let (config_path, err) = get_extends_config_path_or_array(
                    value,
                    host,
                    base_path,
                    config_file_name,
                    property_assignment,
                    property_assignment.initializer(),
                    source_file,
                );
                extended_config_path = Some(config_path);
                property_set_errors.extend(err);
            } else if option.is_none() {
                if key_text == "excludes" {
                    property_set_errors.push(create_diagnostic_for_node_in_source_file(
                        source_file,
                        property_assignment.name(),
                        diag::Unknown_option_excludes_Did_you_mean_exclude,
                        args![],
                    ));
                }
                if OPTIONS_FOR_COMPILER
                    .iter()
                    .any(|option| option.name == key_text)
                {
                    root_compiler_options.push(property_assignment.name());
                }
            }
        }
        property_set_errors
    };

    let (json, err) = {
        let mut notifier = JsonConversionNotifier {
            root_options: *TSCONFIG_ROOT_OPTIONS_MAP,
            on_property_set: Box::new(on_property_set),
        };
        convert_config_file_to_object(source_file, Some(&mut notifier))
    };
    errors.extend(err);
    if let CompilerOptionsValue::Map(json_object) = &json
        && !root_compiler_options.is_empty()
        && !json_object.contains_key("compilerOptions")
    {
        errors.push(create_diagnostic_for_node_in_source_file(
            source_file,
            root_compiler_options[0],
            diag::X_0_should_be_set_inside_the_compilerOptions_object_of_the_config_json_file,
            args![get_text_of_property_name(root_compiler_options[0])],
        ));
    }
    (
        ParsedTsconfig {
            raw: json,
            options: Some(compiler_options),
            type_acquisition: Some(type_acquisition),
            extended_config_path,
        },
        errors,
    )
}

// Go: tsoptions/tsconfigparsing.go:208 TsConfigSourceFile
// PORT: Go `*configFileSpecs` is `Option<ConfigFileSpecs>`; readers copy
// or borrow it and never share it.
#[derive(Clone, Debug, Default)]
// PORT: Go embeds `*ast.SourceFile`. `source_file` is its root node, and
// `path` and `file_name` keep its `Path()` and `FileName()`.
pub struct TsConfigSourceFile {
    pub extended_source_files: Vec<String>,
    pub config_file_specs: Option<ConfigFileSpecs>,
    pub source_file: Node,
    pub path: Path,
    pub file_name: String,
}

// Go: tsoptions/tsconfigparsing.go:214 tsconfigToSourceFile
pub fn tsconfig_to_source_file(tsconfig_source_file: Option<&TsConfigSourceFile>) -> Node {
    match tsconfig_source_file {
        None => Node::NIL,
        Some(tsconfig_source_file) => tsconfig_source_file.source_file,
    }
}

// Go: tsoptions/tsconfigparsing.go:221 NewTsconfigSourceFileFromFilePath
// PORT: Go returns a pointer to a new value; this returns the value. The
// parser keeps source text for the program, so the text is leaked.
// PORT: Go keeps the parser fields (`ParseOptions()`) on the returned
// `*ast.SourceFile`. The port keeps them in the parse, so the parse is
// recorded (`program::note_parsed_source_file`), as for other parses
// outside a program. Then the API encoder finds them for the config file
// (`getConfigSourceFile`), and a program version that publishes the file's
// store gives it its parser fields.
pub fn new_tsconfig_source_file_from_file_path(
    config_file_name: &str,
    config_path: Path,
    config_source_text: &str,
) -> TsConfigSourceFile {
    let source_file = Rc::new(parse_source_file(
        &SourceFileParseOptions {
            file_name: config_file_name.to_string(),
            path: config_path,
            ..Default::default()
        },
        FileText::Static(Box::leak(config_source_text.to_string().into_boxed_str())),
        ScriptKind::JSON,
    ));
    crate::program::note_parsed_source_file(&source_file);
    TsConfigSourceFile {
        source_file: source_file.root,
        path: source_file.path().clone(),
        file_name: source_file.file_name().to_string(),
        ..Default::default()
    }
}

/// Callback for `JsonConversionNotifier`: key text, value, property
/// assignment, parent option and option. Returns the converted value and
/// its diagnostics.
pub type OnPropertySet<'a> = Box<
    dyn FnMut(
            &str,
            &CompilerOptionsValue,
            Node,
            Option<&'static CommandLineOption>,
            Option<&'static CommandLineOption>,
        ) -> Vec<Diagnostic>
        + 'a,
>;

// Go: tsoptions/tsconfigparsing.go:231 jsonConversionNotifier
// PORT: Go func field is a boxed `FnMut`. Callers pass
// `Option<&mut JsonConversionNotifier>` where Go passes a nilable pointer.
// `on_property_set` borrows the value (Go passes the map pointer that the
// result also holds) and returns only the diagnostics, because Go's only
// caller drops the returned value.
pub struct JsonConversionNotifier<'a> {
    pub root_options: &'static CommandLineOption,
    pub on_property_set: OnPropertySet<'a>,
}

// Go: tsoptions/tsconfigparsing.go:236 convertConfigFileToObject
pub fn convert_config_file_to_object(
    source_file: Node,
    json_conversion_notifier: Option<&mut JsonConversionNotifier<'_>>,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let mut root_expression = Node::NIL;
    let statements = source_file.statements();
    if !statements.is_empty() {
        root_expression = statements.get(0).expression();
    }
    if root_expression.is_some() && root_expression.kind() != SyntaxKind::ObjectLiteralExpression {
        let mut base_file_name = "tsconfig.json";
        if get_base_file_name(source_file_file_name(source_file)) == "jsconfig.json" {
            base_file_name = "jsconfig.json";
        }
        let errors = vec![create_diagnostic_for_node_in_source_file(
            source_file,
            root_expression,
            diag::The_root_value_of_a_0_file_must_be_an_object,
            args![base_file_name],
        )];
        // Last-ditch error recovery. Somewhat useful because the JSON parser will recover from some parse errors by
        // synthesizing a top-level array literal expression. There's a reasonable chance the first element of that
        // array is a well-formed configuration object, made into an array element by stray characters.
        if is_array_literal_expression(root_expression) {
            let first_object = root_expression
                .elements()
                .iter()
                .find(|e| is_object_literal_expression(*e));
            if let Some(first_object) = first_object {
                return convert_to_json(
                    source_file,
                    first_object,
                    true, /*returnValue*/
                    json_conversion_notifier,
                );
            }
        }
        return (CompilerOptionsValue::Map(IndexMap::new()), errors);
    }
    convert_to_json(source_file, root_expression, true, json_conversion_notifier)
}

/// Go `reflect.TypeOf(value).Kind() == reflect.Slice` on a non-nil value.
fn is_slice_value(value: &CompilerOptionsValue) -> bool {
    matches!(
        value,
        CompilerOptionsValue::List(_)
            | CompilerOptionsValue::NilList
            | CompilerOptionsValue::StringList(_)
    )
}

// Go: tsoptions/tsconfigparsing.go:266 isCompilerOptionsValue
// PORT: Go `reflect` kind tests are variant matches (Go `orderedMapType`, line 333, is the `Map` match). Slice kinds are `List`
// and `StringList`; `orderedMapType` is only the `Map` variant.
pub fn is_compiler_options_value(
    option: Option<&CommandLineOption>,
    value: &CompilerOptionsValue,
) -> bool {
    if let Some(option) = option {
        if value.is_nil() {
            return !option.disallow_null_or_undefined();
        }
        if option.kind == CommandLineOptionKind::LIST {
            return is_slice_value(value);
        }
        if option.kind == CommandLineOptionKind::LIST_OR_ELEMENT {
            if is_slice_value(value) {
                return true;
            } else {
                return is_compiler_options_value(option.elements(), value);
            }
        }
        if option.kind == CommandLineOptionKind::STRING {
            return matches!(value, CompilerOptionsValue::String(_));
        }
        if option.kind == CommandLineOptionKind::BOOLEAN {
            return matches!(value, CompilerOptionsValue::Bool(_));
        }
        if option.kind == CommandLineOptionKind::NUMBER {
            return matches!(value, CompilerOptionsValue::Number(_));
        }
        if option.kind == CommandLineOptionKind::OBJECT {
            return matches!(value, CompilerOptionsValue::Map(_));
        }
        if option.kind == CommandLineOptionKind::ENUM
            && matches!(value, CompilerOptionsValue::String(_))
        {
            return true;
        }
    }
    false
}

/// Go `val.(string)`. Panics on another type, like Go.
fn as_string(value: &CompilerOptionsValue) -> &str {
    match value {
        CompilerOptionsValue::String(s) => s,
        other => panic!("interface conversion: value is {other:?}, not string"),
    }
}

// Go: tsoptions/tsconfigparsing.go:300 validateJsonOptionValue
pub fn validate_json_option_value(
    opt: &CommandLineOption,
    val: CompilerOptionsValue,
    value_expression: Node,
    source_file: Node,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if val.is_nil() {
        return (CompilerOptionsValue::Nil, Vec::new());
    }

    let mut errors: Vec<Diagnostic> = Vec::new();

    match opt.extra_validation {
        ExtraValidation::LOCALE => {
            let (_, ok) = crate::locale::parse(as_string(&val));
            if !ok {
                errors.push(
                    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                        source_file,
                        value_expression,
                        diag::Locale_must_be_an_IETF_BCP_47_language_tag_Examples_Colon_0_1,
                        args!["en", "ja-jp"],
                    ),
                );
            }
        }
        _ => {}
    }

    if !errors.is_empty() {
        return (CompilerOptionsValue::Nil, errors);
    }
    (val, Vec::new())
}

// Go: tsoptions/tsconfigparsing.go:325 convertJsonOptionOfListType
// PORT: Go returns a `[]any` that callers store in an `any`, where a nil
// slice is not Go nil. A non-nil input stays a non-nil `List` through
// `core.MapIndex` and `core.Filter`. A `NilList` input (an array of nulls)
// and a value that is not an array give a `NilList`, so the option stays
// unset. Go filters with `v != 0`, which never matches a JSON `float64`, so
// `Number(0.0)` is kept.
pub fn convert_json_option_of_list_type(
    option: &CommandLineOption,
    values: CompilerOptionsValue,
    base_path: &str,
    property_assignment: Node,
    value_expression: Node,
    source_file: Node,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let mut expression = Node::NIL;
    let mut errors: Vec<Diagnostic> = Vec::new();
    if let CompilerOptionsValue::List(values) = values {
        let elements = option.elements().expect("list option has elements");
        let mapped_values: Vec<CompilerOptionsValue> = values
            .into_iter()
            .enumerate()
            .map(|(index, v)| {
                if value_expression.is_some() {
                    expression = value_expression.elements().get(index);
                }
                let (result, err) = convert_json_option(
                    elements,
                    v,
                    base_path,
                    property_assignment,
                    expression,
                    source_file,
                );
                errors.extend(err);
                result
            })
            .collect();
        let mut filtered_values = mapped_values;
        if !option.list_preserve_falsy_values {
            filtered_values.retain(|v| {
                !matches!(
                    v,
                    CompilerOptionsValue::Nil | CompilerOptionsValue::Bool(false)
                ) && !matches!(v, CompilerOptionsValue::String(s) if s.is_empty())
            });
        }
        return (CompilerOptionsValue::List(filtered_values), errors);
    }
    (CompilerOptionsValue::NilList, errors)
}

// Go: tsoptions/tsconfigparsing.go:355 configDirTemplate
pub const CONFIG_DIR_TEMPLATE: &str = "${configDir}";

// Go: tsoptions/tsconfigparsing.go:357 startsWithConfigDirTemplate
pub fn starts_with_config_dir_template(value: &CompilerOptionsValue) -> bool {
    let CompilerOptionsValue::String(str) = value else {
        return false;
    };
    str.to_lowercase()
        .starts_with(&CONFIG_DIR_TEMPLATE.to_lowercase())
}

// Go: tsoptions/tsconfigparsing.go:361 normalizeNonListOptionValue
pub fn normalize_non_list_option_value(
    option: &CommandLineOption,
    base_path: &str,
    value: CompilerOptionsValue,
) -> CompilerOptionsValue {
    let mut value = value;
    if option.is_file_path {
        value = CompilerOptionsValue::String(normalize_slashes(as_string(&value)));
        if !starts_with_config_dir_template(&value) {
            value = CompilerOptionsValue::String(get_normalized_absolute_path(
                as_string(&value),
                base_path,
            ));
        }
        if as_string(&value).is_empty() {
            value = CompilerOptionsValue::String(".".to_string());
        }
    }
    value
}

// Go: tsoptions/tsconfigparsing.go:375 convertJsonOption
pub fn convert_json_option(
    opt: &CommandLineOption,
    value: CompilerOptionsValue,
    base_path: &str,
    property_assignment: Node,
    value_expression: Node,
    source_file: Node,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if opt.is_command_line_only {
        let mut node_value = Node::NIL;
        if property_assignment.is_some() {
            node_value = property_assignment.name();
        }
        if source_file.is_nil() && node_value.is_nil() {
            return (
                CompilerOptionsValue::Nil,
                vec![new_compiler_diagnostic(
                    diag::Option_0_can_only_be_specified_on_command_line,
                    args![opt.name],
                )],
            );
        } else {
            return (
                CompilerOptionsValue::Nil,
                vec![
                    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                        source_file,
                        node_value,
                        diag::Option_0_can_only_be_specified_on_command_line,
                        args![opt.name],
                    ),
                ],
            );
        }
    }
    if is_compiler_options_value(Some(opt), &value) {
        match opt.kind {
            CommandLineOptionKind::LIST => {
                return convert_json_option_of_list_type(
                    opt,
                    value,
                    base_path,
                    property_assignment,
                    value_expression,
                    source_file,
                ); // as ArrayLiteralExpression | undefined
            }
            CommandLineOptionKind::LIST_OR_ELEMENT => {
                // PORT: Go `reflect.TypeOf(nil).Kind()` panics. No
                // listOrElement option accepts null (`extends` is the only
                // one), so a nil value never gets here.
                if is_slice_value(&value) {
                    return convert_json_option_of_list_type(
                        opt,
                        value,
                        base_path,
                        property_assignment,
                        value_expression,
                        source_file,
                    );
                } else {
                    return convert_json_option(
                        opt.elements().expect("listOrElement option has elements"),
                        value,
                        base_path,
                        property_assignment,
                        value_expression,
                        source_file,
                    );
                }
            }
            CommandLineOptionKind::ENUM => {
                if value.is_nil() {
                    return (CompilerOptionsValue::Nil, Vec::new());
                }
                return convert_json_option_of_enum_type(
                    opt,
                    as_string(&value),
                    value_expression,
                    source_file,
                );
            }
            _ => {}
        }

        let (validated_value, errors) =
            validate_json_option_value(opt, value, value_expression, source_file);
        if !errors.is_empty() || validated_value.is_nil() {
            (validated_value, errors)
        } else {
            (
                normalize_non_list_option_value(opt, base_path, validated_value),
                errors,
            )
        }
    } else {
        (
            CompilerOptionsValue::Nil,
            vec![
                create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                    source_file,
                    value_expression,
                    diag::Compiler_option_0_requires_a_value_of_type_1,
                    args![opt.name, get_compiler_option_value_type_string(opt)],
                ),
            ],
        )
    }
}

// Go: tsoptions/tsconfigparsing.go:422 getExtendsConfigPathOrArray
// PORT: Go returns a `[]string` that may be nil. Every caller stores it in
// an `any`, where nil and empty are both non-nil, so this returns a `Vec`.
pub fn get_extends_config_path_or_array(
    value: &CompilerOptionsValue,
    host: &dyn ParseConfigHost,
    base_path: &str,
    config_file_name: &str,
    property_assignment: Node,
    value_expression: Node,
    source_file: Node,
) -> (Vec<String>, Vec<Diagnostic>) {
    let mut extended_config_path_array: Vec<String> = Vec::new();
    let mut new_base = base_path.to_string();
    if !config_file_name.is_empty() {
        new_base = directory_of_combined_path(config_file_name, base_path);
    }
    if value.is_nil() {
        let (_, errors) = convert_json_option(
            *EXTENDS_OPTION_DECLARATION,
            value.clone(),
            base_path,
            property_assignment,
            value_expression,
            source_file,
        );
        return (extended_config_path_array, errors);
    }
    if let CompilerOptionsValue::String(value) = value {
        let (val, err) =
            get_extends_config_path(value, host, &new_base, value_expression, source_file);
        if !val.is_empty() {
            extended_config_path_array.push(val);
        }
        return (extended_config_path_array, err);
    }
    let mut errors: Vec<Diagnostic> = Vec::new();
    if is_slice_value(value) {
        // PORT: Go `value.([]any)` panics for a `[]string`. Config JSON
        // values are `List` or `NilList`, so only those get here.
        let Some(values) = value.as_any_slice() else {
            panic!("interface conversion: value is {value:?}, not []any");
        };
        for (index, file_name) in values.iter().enumerate() {
            let mut expression = Node::NIL;
            if value_expression.is_some() {
                expression = value_expression.elements().get(index);
            }
            if let CompilerOptionsValue::String(file_name) = file_name {
                let (val, err) =
                    get_extends_config_path(file_name, host, &new_base, expression, source_file);
                if !val.is_empty() {
                    extended_config_path_array.push(val);
                }
                errors.extend(err);
            } else {
                // PORT: Go passes the whole `value` here, not `fileName`.
                // Kept as Go does.
                let (_, err) = convert_json_option(
                    EXTENDS_OPTION_DECLARATION
                        .elements()
                        .expect("extends has elements"),
                    value.clone(),
                    base_path,
                    property_assignment,
                    expression,
                    source_file,
                );
                errors.extend(err);
            }
        }
    } else {
        (_, errors) = convert_json_option(
            *EXTENDS_OPTION_DECLARATION,
            value.clone(),
            base_path,
            property_assignment,
            value_expression,
            source_file,
        );
    }
    (extended_config_path_array, errors)
}

// Go: tsoptions/tsconfigparsing.go:471 getExtendsConfigPath
pub fn get_extends_config_path(
    extended_config: &str,
    host: &dyn ParseConfigHost,
    base_path: &str,
    value_expression: Node,
    source_file: Node,
) -> (String, Vec<Diagnostic>) {
    let extended_config = normalize_slashes(extended_config);
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut error_file = Node::NIL;
    if source_file.is_some() {
        error_file = source_file;
    }
    if is_rooted_disk_path(&extended_config)
        || extended_config.starts_with("./")
        || extended_config.starts_with("../")
    {
        let mut extended_config_path = get_normalized_absolute_path(&extended_config, base_path);
        if !host.fs().file_exists(&extended_config_path)
            && !extended_config_path.ends_with(EXTENSION_JSON)
        {
            extended_config_path = extended_config_path + EXTENSION_JSON;
            if !host.fs().file_exists(&extended_config_path) {
                errors.push(
                    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                        error_file,
                        value_expression,
                        diag::File_0_not_found,
                        args![extended_config],
                    ),
                );
                return (String::new(), errors);
            }
        }
        return (extended_config_path, errors);
    }
    // If the path isn't a rooted or relative path, resolve like a module
    let resolver_host: Rc<dyn ResolutionHost> = Rc::new(ResolverHost {
        fs: host.fs(),
        current_directory: host.get_current_directory(),
    });
    let resolved = resolve_config(
        &extended_config,
        &combine_paths(base_path, &["tsconfig.json"]),
        resolver_host,
    );
    if resolved.is_resolved() {
        return (resolved.resolved_file_name, errors);
    }
    if extended_config.is_empty() {
        errors.push(
            create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                error_file,
                value_expression,
                diag::Compiler_option_0_cannot_be_given_an_empty_string,
                args!["extends"],
            ),
        );
    } else {
        errors.push(
            create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                error_file,
                value_expression,
                diag::File_0_not_found,
                args![extended_config],
            ),
        );
    }
    (String::new(), errors)
}

// Go: tsoptions/tsconfigparsing.go:507 tsConfigOptions
// PORT: Go declares this struct and never uses it. It is not ported, so no
// `core.ProjectReference` type is needed here.

// Go: tsoptions/tsconfigparsing.go:513 CommandLineOptionNameMap
// PORT: Go map type is a newtype. A Go nil map is an empty map; every
// non-nil map built here has entries, so `is_nil` is `is_empty`.
#[derive(Clone, Debug, Default)]
pub struct CommandLineOptionNameMap(pub FxHashMap<String, &'static CommandLineOption>);

impl CommandLineOptionNameMap {
    // Go: tsoptions/tsconfigparsing.go:515 CommandLineOptionNameMap.Get
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'static CommandLineOption> {
        match self.0.get(name) {
            Some(opt) => Some(*opt),
            None => self.0.get(&name.to_lowercase()).copied(),
        }
    }

    // Go: tsoptions/tsconfigparsing.go:523 CommandLineOptionNameMap.GetSpellingSuggestion
    // PORT: Go map order is random. The result does not depend on it: the
    // closest name wins and a tie goes to the smaller name (`compare`).
    #[must_use]
    pub fn get_spelling_suggestion(&self, name: &str) -> Option<&'static CommandLineOption> {
        get_spelling_suggestion(
            name,
            self.0.values().map(|option| Some(*option)),
            |option: &Option<&'static CommandLineOption>| option.map_or("", |option| option.name),
            |a: &Option<&'static CommandLineOption>, b: &Option<&'static CommandLineOption>| {
                let a = a.map_or("", |option| option.name);
                let b = b.map_or("", |option| option.name);
                match a.cmp(b) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }
            },
        )
    }

    /// Go `m == nil`.
    #[must_use]
    pub fn is_nil(&self) -> bool {
        self.0.is_empty()
    }
}

// Go: tsoptions/tsconfigparsing.go:532 commandLineOptionsToMap
pub fn command_line_options_to_map(
    compiler_options: &[&'static CommandLineOption],
) -> CommandLineOptionNameMap {
    let mut result: FxHashMap<String, &'static CommandLineOption> =
        FxHashMap::with_capacity_and_hasher(compiler_options.len() * 2, Default::default());
    for option in compiler_options {
        result.insert(option.name.to_string(), option);
        result.insert(option.name.to_lowercase(), option);
    }
    CommandLineOptionNameMap(result)
}

// Go: tsoptions/tsconfigparsing.go:541 CommandLineCompilerOptionsMap
pub static COMMAND_LINE_COMPILER_OPTIONS_MAP: LazyLock<CommandLineOptionNameMap> =
    LazyLock::new(|| command_line_options_to_map(&OPTIONS_DECLARATIONS));

// Go: tsoptions/tsconfigparsing.go:543 convertMapToOptions
pub fn convert_map_to_options<O: OptionParser>(
    compiler_options: &IndexMap<String, CompilerOptionsValue>,
    mut result: O,
) -> O {
    // this assumes any `key`, `value` pair in `options` will have `value` already be the correct type. this function should no error handling
    for (key, value) in compiler_options {
        result.parse_option(key, value.clone());
    }
    result
}

// Go: tsoptions/tsconfigparsing.go:551 convertOptionsFromJson
pub fn convert_options_from_json<O: OptionParser>(
    options_name_map: &CommandLineOptionNameMap,
    json_options: &CompilerOptionsValue,
    base_path: &str,
    mut result: O,
) -> (O, Vec<Diagnostic>) {
    if json_options.is_nil() {
        return (result, Vec::new());
    }
    let CompilerOptionsValue::Map(json_map) = json_options else {
        // !!! probably should be an error
        return (result, Vec::new());
    };
    let mut errors: Vec<Diagnostic> = Vec::new();
    for (key, value) in json_map {
        let opt = options_name_map.get(key);
        if let Some(opt) = opt
            && opt.name != key
        {
            // Case-insensitive match found but exact case doesn't match - provide "did you mean" suggestion
            errors.push(
                create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                    Node::NIL,
                    Node::NIL,
                    result.unknown_did_you_mean_diagnostic(),
                    args![key, opt.name],
                ),
            );
            continue;
        }
        let Some(opt) = opt else {
            errors.push(create_unknown_option_error(
                key,
                result.unknown_option_diagnostic(),
                "",
                Node::NIL,
                Node::NIL,
                None,
                Some(result.unknown_did_you_mean_diagnostic()),
                Some(options_name_map),
            ));
            continue;
        };

        let (convert_json, err) = convert_json_option(
            opt,
            value.clone(),
            base_path,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        errors.extend(err);
        let compiler_options_err = result.parse_option(key, convert_json);
        errors.extend(compiler_options_err);
    }
    (result, errors)
}

// Go: tsoptions/tsconfigparsing.go:580 convertArrayLiteralExpressionToJson
// PORT: A Go nil `[]any` (every element converted to nil) is still a
// non-nil `any`. It is a `NilList`, so readers can tell it from `[]`.
pub fn convert_array_literal_expression_to_json(
    source_file: Node,
    elements: NodeSlice,
    element_option: Option<&'static CommandLineOption>,
    return_value: bool,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if !return_value {
        for element in elements.iter() {
            let _ = convert_property_value_to_json(
                source_file,
                element,
                element_option,
                return_value,
                None,
            );
        }
        return (CompilerOptionsValue::Nil, Vec::new());
    }
    // Filter out invalid values
    if elements.is_empty() {
        // Always return an empty array, even if elements is nil.
        // The parser will produce nil slices instead of allocating empty ones.
        return (CompilerOptionsValue::List(Vec::new()), Vec::new());
    }
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut value: Vec<CompilerOptionsValue> = Vec::new();
    for element in elements.iter() {
        let (converted_value, err) = convert_property_value_to_json(
            source_file,
            element,
            element_option,
            return_value,
            None,
        );
        errors.extend(err);
        if !converted_value.is_nil() {
            value.push(converted_value);
        }
    }
    if value.is_empty() {
        return (CompilerOptionsValue::NilList, errors);
    }
    (CompilerOptionsValue::List(value), errors)
}

// Go: tsoptions/tsconfigparsing.go:694 directoryOfCombinedPath (at 673a5f17d713;
// removed by ts#64159)
pub fn directory_of_combined_path(file_name: &str, base_path: &str) -> String {
    // Use the `getNormalizedAbsolutePath` function to avoid canonicalizing the path, as it must remain noncanonical
    // until consistent casing errors are reported
    get_directory_path(&get_normalized_absolute_path(file_name, base_path))
}

// ParseConfigFileTextToJson parses the text of the tsconfig.json file
// fileName is the path to the config file
// jsonText is the text of the config file
// Go: tsoptions/tsconfigparsing.go:613 ParseConfigFileTextToJson
pub fn parse_config_file_text_to_json(
    file_name: &str,
    path: Path,
    json_text: &str,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let json_source_file = parse_source_file(
        &SourceFileParseOptions {
            file_name: file_name.to_string(),
            path,
            ..Default::default()
        },
        FileText::Static(Box::leak(json_text.to_string().into_boxed_str())),
        ScriptKind::JSON,
    )
    .root;
    let (config, mut errors) =
        convert_config_file_to_object(json_source_file /*jsonConversionNotifier*/, None);
    let diagnostics = parsed_source_file_diagnostics(json_source_file);
    if !diagnostics.is_empty() {
        errors = vec![diagnostics[0].clone()];
    }
    (config, errors)
}

// Go: tsoptions/tsconfigparsing.go:715 ParseConfigHost (at 673a5f17d713; removed by ts#64159)
// PORT: Go `FS()` returns the `vfs.FS` interface. This returns a shared
// `Rc<dyn Fs>`, so the resolver host can keep it.
pub trait ParseConfigHost {
    fn fs(&self) -> Rc<dyn Fs>;
    fn get_current_directory(&self) -> String;

    /// Go `getFileNamesFromConfigSpecs` on `FS()` for the config
    /// `config_file_name`: its file names, and how many of them are
    /// literal files.
    // PORT: not in Go. The `tsc -b` host can give the file names that a
    // thread matched ahead of it (execute/build/config_prefetch.rs), as
    // Go parses the configs of a build in parallel.
    fn get_file_names_from_config_specs(
        &self,
        _config_file_name: &str,
        config_file_specs: &ConfigFileSpecs,
        base_path: &str,
        options: Option<&CompilerOptions>,
        extra_extensions: &[String],
    ) -> (Vec<String>, i32) {
        get_file_names_from_config_specs(
            config_file_specs,
            base_path,
            options,
            &*self.fs(),
            extra_extensions,
        )
    }
}

// Go: tsoptions/tsconfigparsing.go:720 resolverHost (at 673a5f17d713; removed by ts#64159)
// PORT: Go embeds the ParseConfigHost interface. This keeps its `FS()` and
// `GetCurrentDirectory()` values, because `ResolutionHost` returns borrows.
pub struct ResolverHost {
    pub fs: Rc<dyn Fs>,
    pub current_directory: String,
}

impl ResolverHost {
    // Go: tsoptions/tsconfigparsing.go:724 (*resolverHost).Trace (at 673a5f17d713;
    // removed by ts#64159)
    pub fn trace(&self, _msg: &str) {}
}

impl ResolutionHost for ResolverHost {
    fn fs(&self) -> &dyn Fs {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

// Go: tsoptions/tsconfigparsing.go:625 ParseJsonSourceFileConfigFileContent
// PORT: Go passes a pointer that no caller uses again, so the source file
// moves into the result.
#[allow(clippy::too_many_arguments)]
pub fn parse_json_source_file_config_file_content(
    source_file: TsConfigSourceFile,
    host: &dyn ParseConfigHost,
    base_path: &str,
    existing_options: Option<&CompilerOptions>,
    existing_options_raw: Option<&IndexMap<String, CompilerOptionsValue>>,
    config_file_name: &str,
    resolution_stack: &[Path],
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> ParsedCommandLine {
    // tracing?.push(tracing.Phase.Parse, "parseJsonSourceFileConfigFileContent", { path: sourceFile.fileName });
    // tracing?.pop();
    parse_json_config_file_content_worker(
        None, /*json*/
        Some(source_file),
        host,
        base_path,
        existing_options,
        existing_options_raw,
        config_file_name,
        resolution_stack,
        extended_config_cache,
    )
}

// Go: tsoptions/tsconfigparsing.go:640 convertObjectLiteralExpressionToJson
// PORT: Go returns a nil map when `returnValue` is false. Every caller
// passes true; `Nil` stands for the nil map.
pub fn convert_object_literal_expression_to_json(
    source_file: Node,
    return_value: bool,
    node: Node,
    object_option: Option<&'static CommandLineOption>,
    mut json_conversion_notifier: Option<&mut JsonConversionNotifier<'_>>,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let mut result: Option<IndexMap<String, CompilerOptionsValue>> = None;
    if return_value {
        result = Some(IndexMap::new());
    }
    let mut errors: Vec<Diagnostic> = Vec::new();
    for element in node.properties().iter() {
        if element.kind() != SyntaxKind::PropertyAssignment {
            errors.push(new_diagnostic(
                source_file,
                element.loc(),
                diag::Property_assignment_expected,
                args![],
            ));
            continue;
        }

        let token = element.question_token();
        if token.is_some() {
            errors.push(new_diagnostic(
                source_file,
                token.loc(),
                diag::The_0_modifier_can_only_be_used_in_TypeScript_files,
                args!["?"],
            ));
        }
        let mut text_of_key = String::new();
        if !is_computed_non_literal_name(element.name()) {
            (text_of_key, _) = try_get_text_of_property_name(element.name());
        }
        let key_text = text_of_key;
        let mut option: Option<&'static CommandLineOption> = None;
        if !key_text.is_empty()
            && let Some(object_option) = object_option
            && !object_option.element_options.is_nil()
        {
            option = object_option.element_options.get(&key_text);
            if option.is_some_and(|o| o.name != key_text) {
                option = None;
            }
        }
        let (value, err) = convert_property_value_to_json(
            source_file,
            element.initializer(),
            option,
            return_value,
            json_conversion_notifier.as_deref_mut(),
        );
        errors.extend(err);
        if !key_text.is_empty() {
            // PORT: Go sets the map value first and passes the same map
            // pointer to onPropertySet. Here the notifier borrows the value
            // and then the result takes it. A copy per level made deeply
            // nested objects quadratic.
            // Notify key value set, if user asked for it
            if let Some(notifier) = json_conversion_notifier.as_deref_mut() {
                let err =
                    (notifier.on_property_set)(&key_text, &value, element, object_option, option);
                errors.extend(err);
            }
            if let Some(result) = &mut result {
                result.insert(key_text, value);
            }
        }
    }
    (
        result.map_or(CompilerOptionsValue::Nil, CompilerOptionsValue::Map),
        errors,
    )
}

// convertToJson converts the json syntax tree into the json value and report errors
// This returns the json value (apart from checking errors) only if returnValue provided is true.
// Otherwise it just checks the errors and returns undefined
// Go: tsoptions/tsconfigparsing.go:692 convertToJson
// PORT: Go returns `struct{}{}` for a missing root. That is a non-nil
// value of no other type, so it is `CompilerOptionsValue::EmptyStruct`.
pub fn convert_to_json(
    source_file: Node,
    root_expression: Node,
    return_value: bool,
    json_conversion_notifier: Option<&mut JsonConversionNotifier<'_>>,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if root_expression.is_nil() {
        if return_value {
            return (CompilerOptionsValue::EmptyStruct, Vec::new());
        } else {
            return (CompilerOptionsValue::Nil, Vec::new());
        }
    }
    let mut root_options: Option<&'static CommandLineOption> = None;
    if let Some(notifier) = &json_conversion_notifier {
        root_options = Some(notifier.root_options);
    }
    convert_property_value_to_json(
        source_file,
        root_expression,
        root_options,
        return_value,
        json_conversion_notifier,
    )
}

// Go: tsoptions/tsconfigparsing.go:712 isDoubleQuotedString
fn is_double_quoted_string(node: Node) -> bool {
    is_string_literal(node)
}

// Go: tsoptions/tsconfigparsing.go:716 convertPropertyValueToJson
pub fn convert_property_value_to_json(
    source_file: Node,
    value_expression: Node,
    option: Option<&'static CommandLineOption>,
    return_value: bool,
    json_conversion_notifier: Option<&mut JsonConversionNotifier<'_>>,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    match value_expression.kind() {
        SyntaxKind::TrueKeyword => return (CompilerOptionsValue::Bool(true), Vec::new()),
        SyntaxKind::FalseKeyword => return (CompilerOptionsValue::Bool(false), Vec::new()),
        SyntaxKind::NullKeyword => return (CompilerOptionsValue::Nil, Vec::new()), // todo: how to manage null

        SyntaxKind::StringLiteral => {
            if !is_double_quoted_string(value_expression) {
                return (
                    CompilerOptionsValue::String(value_expression.text().to_string()),
                    vec![new_diagnostic(
                        source_file,
                        value_expression.loc(),
                        diag::String_literal_with_double_quotes_expected,
                        args![],
                    )],
                );
            }
            return (
                CompilerOptionsValue::String(value_expression.text().to_string()),
                Vec::new(),
            );
        }

        SyntaxKind::NumericLiteral => {
            return (
                CompilerOptionsValue::Number(
                    crate::jsnum::Number::from_string(value_expression.text()).0,
                ),
                Vec::new(),
            );
        }
        SyntaxKind::PrefixUnaryExpression => {
            // Go `break` for a non-JSON form falls through to the error below.
            if value_expression.operator() == SyntaxKind::MinusToken
                && value_expression.operand().kind() == SyntaxKind::NumericLiteral
            {
                return (
                    CompilerOptionsValue::Number(
                        -crate::jsnum::Number::from_string(value_expression.operand().text()).0,
                    ),
                    Vec::new(),
                );
            }
        }
        SyntaxKind::ObjectLiteralExpression => {
            // Currently having element option declaration in the tsconfig with type "object"
            // determines if it needs onSetValidOptionKeyValueInParent callback or not
            // At moment there are only "compilerOptions", "typeAcquisition" and "typingOptions"
            // that satisfies it and need it to modify options set in them (for normalizing file paths)
            // vs what we set in the json
            // If need arises, we can modify this interface and callbacks as needed
            return convert_object_literal_expression_to_json(
                source_file,
                return_value,
                value_expression,
                option,
                json_conversion_notifier,
            );
        }
        SyntaxKind::ArrayLiteralExpression => {
            return convert_array_literal_expression_to_json(
                source_file,
                value_expression.elements(),
                option,
                return_value,
            );
        }
        _ => {}
    }
    // Not in expected format
    let errors = if let Some(option) = option {
        vec![new_diagnostic(
            source_file,
            value_expression.loc(),
            diag::Compiler_option_0_requires_a_value_of_type_1,
            args![option.name, get_compiler_option_value_type_string(option)],
        )]
    } else {
        vec![new_diagnostic(
            source_file,
            value_expression.loc(),
            diag::Property_value_can_only_be_string_literal_numeric_literal_true_false_null_object_literal_or_array_literal,
            args![],
        )]
    };
    (CompilerOptionsValue::Nil, errors)
}

// ParseJsonConfigFileContent parses the contents of a config file (tsconfig.json).
// jsonNode: The contents of the config file to parse
// host: Instance of ParseConfigHost used to enumerate files in folder.
// basePath: A root directory to resolve relative path entries in the config file to. e.g. outDir
// Go: tsoptions/tsconfigparsing.go:770 ParseJsonConfigFileContent
#[allow(clippy::too_many_arguments)]
pub fn parse_json_config_file_content(
    json: &CompilerOptionsValue,
    host: &dyn ParseConfigHost,
    base_path: &str,
    existing_options: Option<&CompilerOptions>,
    config_file_name: &str,
    resolution_stack: &[Path],
    extended_config_cache: Option<&dyn ExtendedConfigCache>,
) -> ParsedCommandLine {
    // PORT: Go changes a caller's ordered map in place; the port normalizes a
    // copy.
    let normalized = normalize_json_value(json.clone());
    let json_object = match normalized {
        CompilerOptionsValue::Map(json_object) => json_object,
        _ => IndexMap::new(),
    };
    parse_json_config_file_content_worker(
        Some(json_object),
        None, /*sourceFile*/
        host,
        base_path,
        existing_options,
        None, /*existingOptionsRaw*/
        config_file_name,
        resolution_stack,
        extended_config_cache,
    )
}

// Go: tsoptions/tsconfigparsing.go:780 normalizeJsonValue
// PORT: `CompilerOptionsValue` has no form for a Go `map[string]any` (Go
// sorts its keys into an ordered map), so only the ordered map case is
// ported. A Go typed slice (`reflect` case) is `StringList`; it cannot be
// nil.
fn normalize_json_value(value: CompilerOptionsValue) -> CompilerOptionsValue {
    match value {
        CompilerOptionsValue::Map(mut value) => {
            for child in value.values_mut() {
                *child = normalize_json_value(std::mem::take(child));
            }
            CompilerOptionsValue::Map(value)
        }
        CompilerOptionsValue::List(value) => {
            CompilerOptionsValue::List(value.into_iter().map(normalize_json_value).collect())
        }
        // Go `case []any` makes a non-nil slice of the same length.
        CompilerOptionsValue::NilList => CompilerOptionsValue::List(Vec::new()),
        CompilerOptionsValue::StringList(value) => CompilerOptionsValue::List(
            value
                .into_iter()
                .map(|child| normalize_json_value(CompilerOptionsValue::String(child)))
                .collect(),
        ),
        value => value,
    }
}

// convertToObject converts the json syntax tree into the json value
// Go: tsoptions/tsconfigparsing.go:817 convertToObject
pub fn convert_to_object(source_file: Node) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let mut root_expression = Node::NIL;
    let statements = source_file.statements();
    if !statements.is_empty() {
        root_expression = statements.get(0).expression();
    }
    convert_to_json(
        source_file,
        root_expression,
        true, /*returnValue*/
        None, /*jsonConversionNotifier*/
    )
}

// Go: tsoptions/options_generated.go:297 getDefaultCompilerOptions (ts#64457 generates it)
pub fn get_default_compiler_options(config_file_name: &str) -> CompilerOptions {
    let mut options = CompilerOptions::default();
    if !config_file_name.is_empty() && get_base_file_name(config_file_name) == "jsconfig.json" {
        let depth = 2;
        options = CompilerOptions {
            allow_js: Tristate::True,
            max_node_module_js_depth: Some(depth),
            skip_lib_check: Tristate::True,
            no_emit: Tristate::True,
            ..Default::default()
        };
    }
    options
}
