use crate::contentmapper::Mapper;
use crate::frontend::json_ext::JsonValue;
use crate::frontend::prelude::*;

// Port of tsoptions/parsinghelpers.go.
//
// PORT: Go `any` values are `CompilerOptionsValue` (U13). Go
// `*collections.OrderedMap[string, any]` is `CompilerOptionsValue::Map` or
// `IndexMap<String, CompilerOptionsValue>`. `IndexMap::insert` keeps the
// position of an existing key, like Go `OrderedMap.Set`.

// Go: tsoptions/parsinghelpers.go:15 ParseTristate
pub fn parse_tristate(value: &CompilerOptionsValue) -> Tristate {
    if value.is_nil() {
        return Tristate::Unknown;
    }
    if let CompilerOptionsValue::Tristate(v) = value {
        return *v;
    }
    if *value == CompilerOptionsValue::Bool(true) {
        Tristate::True
    } else {
        Tristate::False
    }
}

// Go: tsoptions/parsinghelpers.go:29 ParseStringArray
// PORT: Go returns a nil slice (`None`) when the value is not `[]any` and
// for a nil `[]any` (`NilList`). A `List` gives a non-nil slice.
pub fn parse_string_array(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    if let CompilerOptionsValue::List(arr) = value {
        let mut result = Vec::with_capacity(arr.len());
        for v in arr {
            if let CompilerOptionsValue::String(str) = v {
                result.push(str.clone());
            }
        }
        return Some(result);
    }
    None
}

// Go: tsoptions/parsinghelpers.go:45 parseStringMap
// PORT: the Go map values are `[]string`; a nil slice (a non-array value)
// is `None`, so `verifyCompilerOptions` can tell nil from empty.
fn parse_string_map(value: &CompilerOptionsValue) -> Option<IndexMap<String, Option<Vec<String>>>> {
    if let CompilerOptionsValue::Map(m) = value {
        let mut result = IndexMap::with_capacity(m.len());
        for (k, v) in m {
            result.insert(k.clone(), parse_string_array(v));
        }
        return Some(result);
    }
    None
}

// Go: tsoptions/parsinghelpers.go:56 ParseString
pub fn parse_string(value: &CompilerOptionsValue) -> String {
    if let CompilerOptionsValue::String(str) = value {
        return str.clone();
    }
    String::new()
}

// Go: tsoptions/parsinghelpers.go:96 parseNumber
// PORT: Go `*int` (64-bit) is `Option<i64>`. Go `int(float64)` truncates
// toward zero, as `as i64` does in range. Out of range and NaN, Go leaves the
// result to the platform: amd64 gives the min int64, and Rust `as` would
// saturate, so the amd64 value is kept here.
pub fn parse_number(value: &CompilerOptionsValue) -> Option<i64> {
    if let CompilerOptionsValue::Int(num) = value {
        return Some(*num);
    }
    if let CompilerOptionsValue::Number(num) = value {
        // 2^63 is exact in f64; -2^63 itself converts to i64::MIN.
        const LIMIT: f64 = 9_223_372_036_854_775_808.0;
        return Some(if num.is_nan() || *num >= LIMIT || *num < -LIMIT {
            i64::MIN
        } else {
            *num as i64
        });
    }
    None
}

// Go: tsoptions/parsinghelpers.go:107 projectReferenceParseResult
#[derive(Clone, Debug, Default)]
pub struct ProjectReferenceParseResult {
    pub reference: ProjectReference,
    pub has_path: bool,
    pub path_valid: bool,
    pub has_circular: bool,
    pub circular_valid: bool,
}

// Go: tsoptions/parsinghelpers.go:116 parseProjectReference
// PORT: Go returns a nilable pointer; that is `Option`.
pub fn parse_project_reference(json: &CompilerOptionsValue) -> Option<ProjectReferenceParseResult> {
    if let CompilerOptionsValue::Map(v) = json {
        let mut result = ProjectReferenceParseResult::default();
        if let Some(value) = v.get("path") {
            result.has_path = true;
            if let CompilerOptionsValue::String(path) = value {
                result.reference.path = path.clone();
                result.path_valid = true;
            }
        }
        if let Some(value) = v.get("circular") {
            result.has_circular = true;
            if let CompilerOptionsValue::Bool(circular) = value {
                result.reference.circular = *circular;
                result.circular_valid = true;
            }
        }
        return Some(result);
    }
    None
}

// Go: tsoptions/parsinghelpers.go:138 parseContentMapper (tsgo#4712)
// PORT: Go returns a nilable `*contentmapper.Mapper`; that is an owned
// `Option<Mapper>`, which the caller changes and then puts in an `Rc`.
pub fn parse_content_mapper(value: &CompilerOptionsValue) -> (Option<Mapper>, Vec<Diagnostic>) {
    let CompilerOptionsValue::Map(v) = value else {
        return (None, Vec::new());
    };
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut mapper = Mapper::default();
    if let Some(pkg) = v.get("package") {
        match pkg {
            CompilerOptionsValue::String(str) if !str.is_empty() => {
                mapper.definition.package = str.clone();
            }
            _ => errors.push(new_compiler_diagnostic(
                diag::Compiler_option_0_requires_a_value_of_type_1,
                args!["contentMapper.package", "string"],
            )),
        }
    } else {
        errors.push(new_compiler_diagnostic(
            diag::Compiler_option_0_requires_a_value_of_type_1,
            args!["contentMapper.package", "string"],
        ));
    }
    if let Some(extensions) = v.get("extensions") {
        if let Some(strs) = parse_string_array_strict(extensions) {
            mapper.definition.extensions = strs;
        } else {
            errors.push(new_compiler_diagnostic(
                diag::Compiler_option_0_requires_a_value_of_type_1,
                args!["contentMapper.extensions", "string[]"],
            ));
        }
    } else {
        errors.push(new_compiler_diagnostic(
            diag::Compiler_option_0_requires_a_value_of_type_1,
            args!["contentMapper.extensions", "string[]"],
        ));
    }
    if let Some(options) = v.get("options") {
        if !matches!(options, CompilerOptionsValue::Map(_)) {
            errors.push(new_compiler_diagnostic(
                diag::Compiler_option_0_requires_a_value_of_type_1,
                args!["contentMapper.options", "object"],
            ));
        } else {
            // Go: `mapper.Options, _ = json.Marshal(options)` (compact JSON v2).
            let mut json = String::new();
            super::tsconfig_p2::stringify_json(options, &mut json);
            // PORT: a `json.Value` holds Go bytes; the text is in the port
            // form.
            mapper.definition.options =
                JsonValue(crate::scanner_util::go_string_bytes(&json).into_owned());
        }
    }
    if !errors.is_empty() {
        return (None, errors);
    }
    (Some(mapper), errors)
}

// Go: tsoptions/parsinghelpers.go:178 parseStringArrayStrict (tsgo#4712)
// parseStringArrayStrict returns the string slice and true only if value is an array whose
// elements are all strings. A missing element or wrong element type yields false.
// PORT: Go `([]string, bool)` is `Option<Vec<String>>`.
pub fn parse_string_array_strict(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    let Some(arr) = value.as_any_slice() else {
        return None;
    };
    let mut result = Vec::with_capacity(arr.len());
    for v in arr {
        let CompilerOptionsValue::String(str) = v else {
            return None;
        };
        result.push(str.clone());
    }
    Some(result)
}

// Go: tsoptions/parsinghelpers.go:194 parseJsonToStringKey
// PORT: Go returns a map pointer that is never nil. It is `Option` so it
// can go straight to the nilable `json` argument of
// `parseJsonConfigFileContentWorker`; it is always `Some`.
pub fn parse_json_to_string_key(
    json: &CompilerOptionsValue,
) -> Option<IndexMap<String, CompilerOptionsValue>> {
    let mut result = IndexMap::with_capacity(6);
    if let CompilerOptionsValue::Map(m) = json {
        if let Some(v) = m.get("include") {
            result.insert("include".to_string(), v.clone());
        }
        if let Some(v) = m.get("exclude") {
            result.insert("exclude".to_string(), v.clone());
        }
        if let Some(v) = m.get("files") {
            result.insert("files".to_string(), v.clone());
        }
        if let Some(v) = m.get("references") {
            result.insert("references".to_string(), v.clone());
        }
        // tsgo#4712
        if let Some(v) = m.get("contentMappers") {
            result.insert("contentMappers".to_string(), v.clone());
        }
        if let Some(v) = m.get("extends") {
            if let CompilerOptionsValue::String(str) = v {
                result.insert(
                    "extends".to_string(),
                    CompilerOptionsValue::List(vec![CompilerOptionsValue::String(str.clone())]),
                );
            }
            result.insert("extends".to_string(), v.clone());
        }
        if let Some(v) = m.get("compilerOptions") {
            result.insert("compilerOptions".to_string(), v.clone());
        }
        if let Some(v) = m.get("excludes") {
            result.insert("excludes".to_string(), v.clone());
        }
        if let Some(v) = m.get("typeAcquisition") {
            result.insert("typeAcquisition".to_string(), v.clone());
        }
    }
    Some(result)
}

// Go: tsoptions/parsinghelpers.go:231 optionParser
// PORT: Go `*diagnostics.Message` results are never nil for the ported
// parsers, so they are `&'static Message`.
pub trait OptionParser {
    fn parse_option(&mut self, key: &str, value: CompilerOptionsValue) -> Vec<Diagnostic>;
    fn unknown_option_diagnostic(&self) -> &'static Message;
    fn unknown_did_you_mean_diagnostic(&self) -> &'static Message;
}

// Go: tsoptions/parsinghelpers.go:237 compilerOptionsParser
// PORT: the embedded Go `*core.CompilerOptions` is a mutable borrow.
pub struct CompilerOptionsParser<'a> {
    pub compiler_options: &'a mut CompilerOptions,
}

impl OptionParser for CompilerOptionsParser<'_> {
    // Go: tsoptions/parsinghelpers.go:242 (*compilerOptionsParser).ParseOption
    fn parse_option(&mut self, key: &str, value: CompilerOptionsValue) -> Vec<Diagnostic> {
        parse_compiler_options(key, value, self.compiler_options)
    }

    // Go: tsoptions/parsinghelpers.go:275 (*compilerOptionsParser).UnknownOptionDiagnostic
    fn unknown_option_diagnostic(&self) -> &'static Message {
        extra_key_diagnostics("compilerOptions").expect("known key")
    }

    // Go: tsoptions/parsinghelpers.go:279 (*compilerOptionsParser).UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean_diagnostic(&self) -> &'static Message {
        extra_key_did_you_mean_diagnostics("compilerOptions").expect("known key")
    }
}

// Go: tsoptions/parsinghelpers.go:283 typeAcquisitionParser
// PORT: the embedded Go `*core.TypeAcquisition` is a mutable borrow.
pub struct TypeAcquisitionParser<'a> {
    pub type_acquisition: &'a mut TypeAcquisition,
}

impl OptionParser for TypeAcquisitionParser<'_> {
    // Go: tsoptions/parsinghelpers.go:287 (*typeAcquisitionParser).ParseOption
    fn parse_option(&mut self, key: &str, value: CompilerOptionsValue) -> Vec<Diagnostic> {
        parse_type_acquisition(key, value, self.type_acquisition)
    }

    // Go: tsoptions/parsinghelpers.go:291 (*typeAcquisitionParser).UnknownOptionDiagnostic
    fn unknown_option_diagnostic(&self) -> &'static Message {
        extra_key_diagnostics("typeAcquisition").expect("known key")
    }

    // Go: tsoptions/parsinghelpers.go:295 (*typeAcquisitionParser).UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean_diagnostic(&self) -> &'static Message {
        extra_key_did_you_mean_diagnostics("typeAcquisition").expect("known key")
    }
}

// Go: tsoptions/parsinghelpers.go:299 buildOptionsParser
// PORT: ported next to `core.BuildOptions` in execute/build/command_line.rs.

// Go: tsoptions/parsinghelpers.go:315 ParseCompilerOptions
// PORT: Go `allOptions` can be nil; every Rust caller has options, so the
// nil check is dropped.
pub fn parse_compiler_options(
    key: &str,
    value: CompilerOptionsValue,
    all_options: &mut CompilerOptions,
) -> Vec<Diagnostic> {
    if value.is_nil() {
        return Vec::new();
    }
    parse_compiler_options_worker(key, &value, all_options);
    Vec::new()
}

// Go: tsoptions/options_generated.go:11 parseCompilerOptions (ts#64457 generates it)
// PORT: renamed, because the snake_case form of the Go name is the same as
// `ParseCompilerOptions`. Go `[]string` results go to `Option<Vec<String>>`
// fields, so an explicit empty list stays different from nil.
fn parse_compiler_options_worker(
    key: &str,
    value: &CompilerOptionsValue,
    all_options: &mut CompilerOptions,
) -> bool {
    let mut key = key;
    let option = COMMAND_LINE_COMPILER_OPTIONS_MAP.get(key);
    if let Some(option) = option {
        key = option.name;
    }
    match key {
        "allowJs" => all_options.allow_js = parse_tristate(value),
        "allowImportingTsExtensions" => {
            all_options.allow_importing_ts_extensions = parse_tristate(value)
        }
        "allowSyntheticDefaultImports" => {
            all_options.allow_synthetic_default_imports = parse_tristate(value)
        }
        "allowNonTsExtensions" => all_options.allow_non_ts_extensions = parse_tristate(value),
        "allowUmdGlobalAccess" => all_options.allow_umd_global_access = parse_tristate(value),
        "allowUnreachableCode" => all_options.allow_unreachable_code = parse_tristate(value),
        "allowUnusedLabels" => all_options.allow_unused_labels = parse_tristate(value),
        "allowArbitraryExtensions" => {
            all_options.allow_arbitrary_extensions = parse_tristate(value)
        }
        "alwaysStrict" => all_options.always_strict = parse_tristate(value),
        "assumeChangesOnlyAffectDirectDependencies" => {
            all_options.assume_changes_only_affect_direct_dependencies = parse_tristate(value);
        }
        "baseUrl" => all_options.base_url = parse_string(value),
        "build" => all_options.build = parse_tristate(value),
        "checkJs" => all_options.check_js = parse_tristate(value),
        "customConditions" => all_options.custom_conditions = parse_string_array(value),
        "composite" => all_options.composite = parse_tristate(value),
        "declarationDir" => all_options.declaration_dir = parse_string(value),
        "deduplicatePackages" => all_options.deduplicate_packages = parse_tristate(value),
        "diagnostics" => all_options.diagnostics = parse_tristate(value),
        "disableSizeLimit" => all_options.disable_size_limit = parse_tristate(value),
        "disableSourceOfProjectReferenceRedirect" => {
            all_options.disable_source_of_project_reference_redirect = parse_tristate(value);
        }
        "disableSolutionSearching" => {
            all_options.disable_solution_searching = parse_tristate(value)
        }
        "disableReferencedProjectLoad" => {
            all_options.disable_referenced_project_load = parse_tristate(value)
        }
        "declarationMap" => all_options.declaration_map = parse_tristate(value),
        "declaration" => all_options.declaration = parse_tristate(value),
        "downlevelIteration" => all_options.downlevel_iteration = parse_tristate(value),
        "erasableSyntaxOnly" => all_options.erasable_syntax_only = parse_tristate(value),
        "emitDeclarationOnly" => all_options.emit_declaration_only = parse_tristate(value),
        "extendedDiagnostics" => all_options.extended_diagnostics = parse_tristate(value),
        "emitDecoratorMetadata" => all_options.emit_decorator_metadata = parse_tristate(value),
        "emitBOM" => all_options.emit_bom = parse_tristate(value),
        "esModuleInterop" => all_options.es_module_interop = parse_tristate(value),
        "exactOptionalPropertyTypes" => {
            all_options.exact_optional_property_types = parse_tristate(value)
        }
        "explainFiles" => all_options.explain_files = parse_tristate(value),
        "experimentalDecorators" => all_options.experimental_decorators = parse_tristate(value),
        "forceConsistentCasingInFileNames" => {
            all_options.force_consistent_casing_in_file_names = parse_tristate(value)
        }
        "generateCpuProfile" => all_options.generate_cpu_profile = parse_string(value),
        "generateTrace" => all_options.generate_trace = parse_string(value),
        "isolatedModules" => all_options.isolated_modules = parse_tristate(value),
        "ignoreConfig" => all_options.ignore_config = parse_tristate(value),
        "ignoreDeprecations" => all_options.ignore_deprecations = parse_string(value),
        "importHelpers" => all_options.import_helpers = parse_tristate(value),
        "incremental" => all_options.incremental = parse_tristate(value),
        "init" => all_options.init = parse_tristate(value),
        "inlineSourceMap" => all_options.inline_source_map = parse_tristate(value),
        "inlineSources" => all_options.inline_sources = parse_tristate(value),
        "isolatedDeclarations" => all_options.isolated_declarations = parse_tristate(value),
        "jsx" => all_options.jsx = float_or_int32_to_flag(value, as_jsx_emit, JsxEmit),
        "jsxFactory" => all_options.jsx_factory = parse_string(value),
        "jsxFragmentFactory" => all_options.jsx_fragment_factory = parse_string(value),
        "jsxImportSource" => all_options.jsx_import_source = parse_string(value),
        "lib" => {
            if let CompilerOptionsValue::StringList(lib) = value {
                all_options.lib = Some(lib.clone());
            } else {
                all_options.lib = parse_string_array(value);
            }
        }
        "libReplacement" => all_options.lib_replacement = parse_tristate(value),
        "listEmittedFiles" => all_options.list_emitted_files = parse_tristate(value),
        "listFiles" => all_options.list_files = parse_tristate(value),
        "listFilesOnly" => all_options.list_files_only = parse_tristate(value),
        "locale" => all_options.locale = parse_string(value),
        "mapRoot" => all_options.map_root = parse_string(value),
        "module" => all_options.module = float_or_int32_to_flag(value, as_module_kind, ModuleKind),
        "moduleDetectionKind" => {
            all_options.module_detection =
                float_or_int32_to_flag(value, as_module_detection_kind, ModuleDetectionKind);
        }
        "moduleResolution" => {
            all_options.module_resolution =
                float_or_int32_to_flag(value, as_module_resolution_kind, ModuleResolutionKind);
        }
        "moduleSuffixes" => all_options.module_suffixes = parse_string_array(value),
        "moduleDetection" => {
            all_options.module_detection =
                float_or_int32_to_flag(value, as_module_detection_kind, ModuleDetectionKind);
        }
        "noCheck" => all_options.no_check = parse_tristate(value),
        "noFallthroughCasesInSwitch" => {
            all_options.no_fallthrough_cases_in_switch = parse_tristate(value)
        }
        "noEmitForJsFiles" => all_options.no_emit_for_js_files = parse_tristate(value),
        "noErrorTruncation" => all_options.no_error_truncation = parse_tristate(value),
        "noImplicitAny" => all_options.no_implicit_any = parse_tristate(value),
        "noImplicitThis" => all_options.no_implicit_this = parse_tristate(value),
        "noLib" => all_options.no_lib = parse_tristate(value),
        "noPropertyAccessFromIndexSignature" => {
            all_options.no_property_access_from_index_signature = parse_tristate(value)
        }
        "noUncheckedIndexedAccess" => {
            all_options.no_unchecked_indexed_access = parse_tristate(value)
        }
        "noEmitHelpers" => all_options.no_emit_helpers = parse_tristate(value),
        "noEmitOnError" => all_options.no_emit_on_error = parse_tristate(value),
        "noImplicitReturns" => all_options.no_implicit_returns = parse_tristate(value),
        "noUnusedLocals" => all_options.no_unused_locals = parse_tristate(value),
        "noUnusedParameters" => all_options.no_unused_parameters = parse_tristate(value),
        "noImplicitOverride" => all_options.no_implicit_override = parse_tristate(value),
        "noUncheckedSideEffectImports" => {
            all_options.no_unchecked_side_effect_imports = parse_tristate(value)
        }
        "outFile" => all_options.out_file = parse_string(value),
        "noResolve" => all_options.no_resolve = parse_tristate(value),
        "paths" => all_options.paths = parse_string_map(value),
        "plugins" => {
            // Effect-TS/tsgo patch 013: parse @effect/language-service plugin configuration.
            all_options.effect =
                crate::effect::etscore::parse_from_plugins(value).map(std::sync::Arc::new);
            // Native TypeScript does not load plugins; retain them only so tools can report the incompatibility.
            // PORT: Go `core.Map` keeps a nil `[]any` (`NilList`) nil.
            if let CompilerOptionsValue::NilList = value {
                all_options.plugins = None;
            } else if let CompilerOptionsValue::List(plugins) = value {
                all_options.plugins = Some(
                    plugins
                        .iter()
                        .map(|plugin| {
                            if let CompilerOptionsValue::Map(plugin_map) = plugin {
                                return PluginImport {
                                    name: parse_string(
                                        plugin_map
                                            .get("name")
                                            .unwrap_or(&CompilerOptionsValue::Nil),
                                    ),
                                };
                            }
                            PluginImport::default()
                        })
                        .collect(),
                );
            }
        }
        "preserveWatchOutput" => all_options.preserve_watch_output = parse_tristate(value),
        "preserveConstEnums" => all_options.preserve_const_enums = parse_tristate(value),
        "preserveSymlinks" => all_options.preserve_symlinks = parse_tristate(value),
        "project" => all_options.project = parse_string(value),
        "pretty" => all_options.pretty = parse_tristate(value),
        "resolveJsonModule" => all_options.resolve_json_module = parse_tristate(value),
        "resolvePackageJsonExports" => {
            all_options.resolve_package_json_exports = parse_tristate(value)
        }
        "resolvePackageJsonImports" => {
            all_options.resolve_package_json_imports = parse_tristate(value)
        }
        "reactNamespace" => all_options.react_namespace = parse_string(value),
        "rewriteRelativeImportExtensions" => {
            all_options.rewrite_relative_import_extensions = parse_tristate(value)
        }
        "rootDir" => all_options.root_dir = parse_string(value),
        "rootDirs" => all_options.root_dirs = parse_string_array(value),
        "removeComments" => all_options.remove_comments = parse_tristate(value),
        "stableTypeOrdering" => all_options.stable_type_ordering = parse_tristate(value),
        "strict" => all_options.strict = parse_tristate(value),
        "strictBindCallApply" => all_options.strict_bind_call_apply = parse_tristate(value),
        "strictBuiltinIteratorReturn" => {
            all_options.strict_builtin_iterator_return = parse_tristate(value)
        }
        "strictFunctionTypes" => all_options.strict_function_types = parse_tristate(value),
        "strictNullChecks" => all_options.strict_null_checks = parse_tristate(value),
        "strictPropertyInitialization" => {
            all_options.strict_property_initialization = parse_tristate(value)
        }
        "skipDefaultLibCheck" => all_options.skip_default_lib_check = parse_tristate(value),
        "sourceMap" => all_options.source_map = parse_tristate(value),
        "sourceRoot" => all_options.source_root = parse_string(value),
        "stripInternal" => all_options.strip_internal = parse_tristate(value),
        "suppressOutputPathCheck" => all_options.suppress_output_path_check = parse_tristate(value),
        "target" => {
            all_options.target = float_or_int32_to_flag(value, as_script_target, ScriptTarget)
        }
        "traceResolution" => all_options.trace_resolution = parse_tristate(value),
        "tsBuildInfoFile" => all_options.ts_build_info_file = parse_string(value),
        "typeRoots" => all_options.type_roots = parse_string_array(value),
        "types" => all_options.types = parse_string_array(value),
        "useDefineForClassFields" => {
            all_options.use_define_for_class_fields = parse_tristate(value)
        }
        "useUnknownInCatchVariables" => {
            all_options.use_unknown_in_catch_variables = parse_tristate(value)
        }
        "verbatimModuleSyntax" => all_options.verbatim_module_syntax = parse_tristate(value),
        "version" => all_options.version = parse_tristate(value),
        "help" => all_options.help = parse_tristate(value),
        "all" => all_options.all = parse_tristate(value),
        "maxNodeModuleJsDepth" => all_options.max_node_module_js_depth = parse_number(value),
        "skipLibCheck" => all_options.skip_lib_check = parse_tristate(value),
        "noEmit" => all_options.no_emit = parse_tristate(value),
        "showConfig" => all_options.show_config = parse_tristate(value),
        "configFilePath" => all_options.config_file_path = parse_string(value),
        "noDtsResolution" => all_options.no_dts_resolution = parse_tristate(value),
        "pathsBasePath" => all_options.paths_base_path = parse_string(value),
        "outDir" => all_options.out_dir = parse_string(value),
        "newLine" => {
            all_options.new_line = float_or_int32_to_flag(value, as_new_line_kind, NewLineKind)
        }
        "watch" => all_options.watch = parse_tristate(value),
        "pprofDir" => all_options.pprof_dir = parse_string(value),
        "singleThreaded" => all_options.single_threaded = parse_tristate(value),
        "quiet" => all_options.quiet = parse_tristate(value),
        "checkers" => all_options.checkers = parse_number(value),
        "runExternalCode" => all_options.run_external_code = parse_tristate(value),
        _ => {
            // different than any key above
            return false;
        }
    }
    true
}

// PORT: Go `value.(T)` for the enum types of `floatOrInt32ToFlag`. One
// accessor per Go type argument.
fn as_jsx_emit(value: &CompilerOptionsValue) -> Option<JsxEmit> {
    if let CompilerOptionsValue::JsxEmit(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn as_module_kind(value: &CompilerOptionsValue) -> Option<ModuleKind> {
    if let CompilerOptionsValue::ModuleKind(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn as_module_detection_kind(value: &CompilerOptionsValue) -> Option<ModuleDetectionKind> {
    if let CompilerOptionsValue::ModuleDetectionKind(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn as_module_resolution_kind(value: &CompilerOptionsValue) -> Option<ModuleResolutionKind> {
    if let CompilerOptionsValue::ModuleResolutionKind(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn as_script_target(value: &CompilerOptionsValue) -> Option<ScriptTarget> {
    if let CompilerOptionsValue::ScriptTarget(v) = value {
        Some(*v)
    } else {
        None
    }
}
fn as_new_line_kind(value: &CompilerOptionsValue) -> Option<NewLineKind> {
    if let CompilerOptionsValue::NewLineKind(v) = value {
        Some(*v)
    } else {
        None
    }
}

// Go: tsoptions/parsinghelpers.go:325 floatOrInt32ToFlag
// PORT: the Go type parameter `T ~int32` is the pair (`typed`, `from_i32`):
// `typed` is Go `value.(T)` and `from_i32` is the Go conversion `T(...)`.
// Go `value.(float64)` panics on another type; so does this port. A JSON
// value of another type comes from a bad `.tsbuildinfo` (`"module": "x"`),
// and the panic has Go's runtime text.
fn float_or_int32_to_flag<T>(
    value: &CompilerOptionsValue,
    typed: fn(&CompilerOptionsValue) -> Option<T>,
    from_i32: fn(i32) -> T,
) -> T {
    if let Some(v) = typed(value) {
        return v;
    }
    let go_type = match value {
        CompilerOptionsValue::Number(f) => return from_i32(*f as i32),
        CompilerOptionsValue::Nil => "nil",
        CompilerOptionsValue::Bool(_) => "bool",
        CompilerOptionsValue::String(_) => "string",
        CompilerOptionsValue::List(_) | CompilerOptionsValue::NilList => "[]interface {}",
        CompilerOptionsValue::Map(_) => "map[string]interface {}",
        _ => panic!("interface conversion: option value is not float64"),
    };
    go_panic(format!(
        "interface conversion: interface {{}} is {go_type}, not float64"
    ))
}

// Go: tsoptions/options_generated.go:322 ParseTypeAcquisition (ts#64457 generates it)
// PORT: Go `allOptions` can be nil; every Rust caller has one, so the nil
// check is dropped. Go `[]string` fields are `Vec<String>`.
pub fn parse_type_acquisition(
    key: &str,
    value: CompilerOptionsValue,
    all_options: &mut TypeAcquisition,
) -> Vec<Diagnostic> {
    if value.is_nil() {
        return Vec::new();
    }
    let value = &value;
    match key {
        "enable" => all_options.enable = parse_tristate(value),
        "include" => all_options.include = parse_string_array(value).unwrap_or_default(),
        "exclude" => all_options.exclude = parse_string_array(value).unwrap_or_default(),
        "disableFilenameBasedTypeAcquisition" => {
            all_options.disable_filename_based_type_acquisition = parse_tristate(value);
        }
        _ => {}
    }
    Vec::new()
}

// Go: tsoptions/options_generated.go:342 ParseBuildOptions (ts#64457 generates it)
// PORT: ported in execute/build/command_line.rs.

/// Effect-TS/tsgo patch 013 `mergeCompilerOptions` with the source config
/// path and base path: the standard merge, then the Effect plugin options
/// merge (`effect::configraw`). The standard merge leaves `effect` alone.
pub fn merge_compiler_options_with_paths<'a>(
    target_options: &'a mut CompilerOptions,
    source_options: Option<&CompilerOptions>,
    raw_source: Option<&IndexMap<String, CompilerOptionsValue>>,
    source_config_path: &str,
    base_path: &str,
) -> &'a mut CompilerOptions {
    let Some(source) = source_options else {
        return target_options;
    };
    merge_compiler_options(target_options, Some(source), raw_source);
    crate::effect::configraw::merge_effect_compiler_options(
        target_options,
        source,
        raw_source,
        source_config_path,
        base_path,
    );
    target_options
}

// Go: tsoptions/parsinghelpers.go:336 mergeCompilerOptions
// mergeCompilerOptions merges the source compiler options into the target compiler options
// with optional awareness of explicitly set null values in the raw JSON.
// Fields in the source options will overwrite the corresponding fields in the target options,
// including when they are explicitly set to null in the raw configuration (if rawSource is provided).
// PORT: Go loops over the struct fields by reflection. The Rust port lists
// each field with its Go json name, in Go field order. Go `IsZero` is
// `== Default::default()`. Go `[]string` fields are `Option<Vec<String>>`,
// so `None` (nil) is zero and `Some(vec![])` (empty non-nil) is not, as in
// Go. Go `rawSource any` is the map it must
// hold to have an effect, so it is `Option<&IndexMap>`. The Go merge
// copies the `Paths` pointer, so a later in-place change of `paths` also
// changes the source options; the Rust clone does not share it.
pub fn merge_compiler_options<'a>(
    target_options: &'a mut CompilerOptions,
    source_options: Option<&CompilerOptions>,
    raw_source: Option<&IndexMap<String, CompilerOptionsValue>>,
) -> &'a mut CompilerOptions {
    let Some(source_options) = source_options else {
        return target_options;
    };

    // Collect explicitly null field names from raw JSON
    let mut explicit_null_fields: FxHashSet<String> = FxHashSet::default();
    if let Some(raw_map) = raw_source {
        // Options are nested under "compilerOptions" in both tsconfig.json and wrapped command line options
        if let Some(CompilerOptionsValue::Map(compiler_options_map)) =
            raw_map.get("compilerOptions")
        {
            for (key, value) in compiler_options_map {
                if value.is_nil() {
                    explicit_null_fields.insert(key.clone());
                }
            }
        }
    }

    // Go `reflect.Value.IsZero` for one field.
    fn is_zero_value<T: Default + PartialEq>(value: &T) -> bool {
        *value == T::default()
    }

    // Do the merge, handling explicit nulls during the normal merge
    macro_rules! merge_fields {
        ($($field:ident => $json:literal,)*) => {
            $(
                // Get the JSON field name for this struct field and check if it's explicitly null
                if explicit_null_fields.contains($json) {
                    target_options.$field = Default::default();
                } else if !is_zero_value(&source_options.$field) {
                    // Normal merge behavior: copy non-zero fields
                    target_options.$field = source_options.$field.clone();
                }
            )*
        };
    }
    merge_fields! {
        allow_js => "allowJs",
        allow_arbitrary_extensions => "allowArbitraryExtensions",
        allow_importing_ts_extensions => "allowImportingTsExtensions",
        allow_non_ts_extensions => "allowNonTsExtensions",
        allow_umd_global_access => "allowUmdGlobalAccess",
        allow_unreachable_code => "allowUnreachableCode",
        allow_unused_labels => "allowUnusedLabels",
        assume_changes_only_affect_direct_dependencies => "assumeChangesOnlyAffectDirectDependencies",
        check_js => "checkJs",
        custom_conditions => "customConditions",
        composite => "composite",
        emit_declaration_only => "emitDeclarationOnly",
        emit_bom => "emitBOM",
        emit_decorator_metadata => "emitDecoratorMetadata",
        declaration => "declaration",
        declaration_dir => "declarationDir",
        declaration_map => "declarationMap",
        deduplicate_packages => "deduplicatePackages",
        disable_size_limit => "disableSizeLimit",
        disable_source_of_project_reference_redirect => "disableSourceOfProjectReferenceRedirect",
        disable_solution_searching => "disableSolutionSearching",
        disable_referenced_project_load => "disableReferencedProjectLoad",
        erasable_syntax_only => "erasableSyntaxOnly",
        exact_optional_property_types => "exactOptionalPropertyTypes",
        experimental_decorators => "experimentalDecorators",
        force_consistent_casing_in_file_names => "forceConsistentCasingInFileNames",
        isolated_modules => "isolatedModules",
        isolated_declarations => "isolatedDeclarations",
        ignore_config => "ignoreConfig",
        ignore_deprecations => "ignoreDeprecations",
        import_helpers => "importHelpers",
        inline_source_map => "inlineSourceMap",
        inline_sources => "inlineSources",
        init => "init",
        incremental => "incremental",
        jsx => "jsx",
        jsx_factory => "jsxFactory",
        jsx_fragment_factory => "jsxFragmentFactory",
        jsx_import_source => "jsxImportSource",
        lib => "lib",
        lib_replacement => "libReplacement",
        locale => "locale",
        map_root => "mapRoot",
        module => "module",
        module_resolution => "moduleResolution",
        module_suffixes => "moduleSuffixes",
        module_detection => "moduleDetection",
        new_line => "newLine",
        no_emit => "noEmit",
        no_check => "noCheck",
        no_error_truncation => "noErrorTruncation",
        no_fallthrough_cases_in_switch => "noFallthroughCasesInSwitch",
        no_implicit_any => "noImplicitAny",
        no_implicit_this => "noImplicitThis",
        no_implicit_returns => "noImplicitReturns",
        no_emit_helpers => "noEmitHelpers",
        no_lib => "noLib",
        no_property_access_from_index_signature => "noPropertyAccessFromIndexSignature",
        no_unchecked_indexed_access => "noUncheckedIndexedAccess",
        no_emit_on_error => "noEmitOnError",
        no_unused_locals => "noUnusedLocals",
        no_unused_parameters => "noUnusedParameters",
        no_resolve => "noResolve",
        no_implicit_override => "noImplicitOverride",
        no_unchecked_side_effect_imports => "noUncheckedSideEffectImports",
        out_dir => "outDir",
        paths => "paths",
        plugins => "plugins",
        preserve_const_enums => "preserveConstEnums",
        preserve_symlinks => "preserveSymlinks",
        project => "project",
        resolve_json_module => "resolveJsonModule",
        resolve_package_json_exports => "resolvePackageJsonExports",
        resolve_package_json_imports => "resolvePackageJsonImports",
        remove_comments => "removeComments",
        rewrite_relative_import_extensions => "rewriteRelativeImportExtensions",
        react_namespace => "reactNamespace",
        root_dir => "rootDir",
        root_dirs => "rootDirs",
        skip_lib_check => "skipLibCheck",
        stable_type_ordering => "stableTypeOrdering",
        strict => "strict",
        strict_bind_call_apply => "strictBindCallApply",
        strict_builtin_iterator_return => "strictBuiltinIteratorReturn",
        strict_function_types => "strictFunctionTypes",
        strict_null_checks => "strictNullChecks",
        strict_property_initialization => "strictPropertyInitialization",
        strip_internal => "stripInternal",
        skip_default_lib_check => "skipDefaultLibCheck",
        source_map => "sourceMap",
        source_root => "sourceRoot",
        suppress_output_path_check => "suppressOutputPathCheck",
        target => "target",
        trace_resolution => "traceResolution",
        ts_build_info_file => "tsBuildInfoFile",
        type_roots => "typeRoots",
        types => "types",
        use_define_for_class_fields => "useDefineForClassFields",
        use_unknown_in_catch_variables => "useUnknownInCatchVariables",
        verbatim_module_syntax => "verbatimModuleSyntax",
        max_node_module_js_depth => "maxNodeModuleJsDepth",
        allow_synthetic_default_imports => "allowSyntheticDefaultImports",
        always_strict => "alwaysStrict",
        base_url => "baseUrl",
        downlevel_iteration => "downlevelIteration",
        es_module_interop => "esModuleInterop",
        out_file => "outFile",
        config_file_path => "configFilePath",
        no_dts_resolution => "noDtsResolution",
        paths_base_path => "pathsBasePath",
        diagnostics => "diagnostics",
        extended_diagnostics => "extendedDiagnostics",
        generate_cpu_profile => "generateCpuProfile",
        generate_trace => "generateTrace",
        list_emitted_files => "listEmittedFiles",
        list_files => "listFiles",
        explain_files => "explainFiles",
        list_files_only => "listFilesOnly",
        no_emit_for_js_files => "noEmitForJsFiles",
        preserve_watch_output => "preserveWatchOutput",
        pretty => "pretty",
        version => "version",
        watch => "watch",
        show_config => "showConfig",
        build => "build",
        help => "help",
        all => "all",
        run_external_code => "runExternalCode",
        pprof_dir => "pprofDir",
        single_threaded => "singleThreaded",
        quiet => "quiet",
        checkers => "checkers",
    }

    target_options
}

// Go: tsoptions/parsinghelpers.go:386 convertToOptionsWithAbsolutePaths
// PORT: Go changes the map through its pointer and returns it. The Rust
// port takes the map by value and returns it.
pub fn convert_to_options_with_absolute_paths(
    options_base: Option<IndexMap<String, CompilerOptionsValue>>,
    option_map: &CommandLineOptionNameMap,
    cwd: &str,
) -> Option<IndexMap<String, CompilerOptionsValue>> {
    // !!! convert to options with absolute paths was previously done with `CompilerOptions` object, but for ease of implementation, we do it pre-conversion.
    // !!! Revisit this choice if/when refactoring when conversion is done in tsconfig parsing
    let mut options_base = options_base?;
    for (o, v) in options_base.iter_mut() {
        let (result, ok) = convert_option_to_absolute_path(o, v, option_map, cwd);
        if ok {
            *v = result;
        }
    }
    Some(options_base)
}

// Go: tsoptions/parsinghelpers.go:401 ConvertOptionToAbsolutePath
pub fn convert_option_to_absolute_path(
    o: &str,
    v: &CompilerOptionsValue,
    option_map: &CommandLineOptionNameMap,
    cwd: &str,
) -> (CompilerOptionsValue, bool) {
    let Some(option) = option_map.get(o) else {
        return (CompilerOptionsValue::Nil, false);
    };
    if option.kind == CommandLineOptionKind::LIST {
        // PORT: Go `option.Elements()` is never nil for a list option.
        if option
            .elements()
            .expect("list option has elements")
            .is_file_path
        {
            if let CompilerOptionsValue::StringList(arr) = v {
                return (
                    CompilerOptionsValue::StringList(
                        arr.iter()
                            .map(|item| get_normalized_absolute_path(item, cwd))
                            .collect(),
                    ),
                    true,
                );
            }
            if let CompilerOptionsValue::List(arr) = v {
                return (
                    CompilerOptionsValue::List(
                        arr.iter()
                            .map(|item| {
                                if let CompilerOptionsValue::String(s) = item {
                                    return CompilerOptionsValue::String(
                                        get_normalized_absolute_path(s, cwd),
                                    );
                                }
                                item.clone()
                            })
                            .collect(),
                    ),
                    true,
                );
            }
            // Go `core.Map` keeps a nil `[]any` nil.
            if let CompilerOptionsValue::NilList = v {
                return (CompilerOptionsValue::NilList, true);
            }
        }
    } else if option.is_file_path {
        if let CompilerOptionsValue::String(value) = v {
            return (
                CompilerOptionsValue::String(get_normalized_absolute_path(value, cwd)),
                true,
            );
        }
    }
    (CompilerOptionsValue::Nil, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go keeps an explicit `[]` as a non-nil slice, so it overrides the
    // extended config in `mergeCompilerOptions`.
    #[test]
    fn empty_string_array_overrides_extended_config() {
        let mut target = CompilerOptions {
            custom_conditions: Some(vec!["@tanstack/custom-condition".to_string()]),
            types: Some(vec!["node".to_string()]),
            ..Default::default()
        };
        let mut source = CompilerOptions::default();
        let empty = CompilerOptionsValue::List(Vec::new());
        parse_compiler_options_worker("customConditions", &empty, &mut source);
        parse_compiler_options_worker("types", &empty, &mut source);
        merge_compiler_options(&mut target, Some(&source), None);
        assert_eq!(target.custom_conditions, Some(Vec::new()));
        assert_eq!(target.types, Some(Vec::new()));
        assert_eq!(target.root_dirs, None);
    }

    // Go keeps a nil `[]any` nil. An array of nulls and a null list option
    // leave the option unset, so the extended config keeps its value.
    #[test]
    fn nil_list_leaves_list_options_unset() {
        use crate::frontend::tsoptions::{
            COMPILER_NAME_MAP, convert_json_option_of_list_type, parse_config_file_text_to_json,
        };
        let (json, errors) = parse_config_file_text_to_json(
            "/tsconfig.json",
            Path("/tsconfig.json".to_string()),
            r#"{"types": [null, null], "lib": []}"#,
        );
        assert!(errors.is_empty());
        let CompilerOptionsValue::Map(json) = json else {
            panic!("tsconfig JSON is an object");
        };
        assert_eq!(json["types"], CompilerOptionsValue::NilList);
        assert_eq!(json["lib"], CompilerOptionsValue::List(Vec::new()));

        let plugins = COMPILER_NAME_MAP.get("plugins").expect("plugins option");
        let (null_plugins, errors) = convert_json_option_of_list_type(
            plugins,
            CompilerOptionsValue::Nil,
            "/",
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        assert!(errors.is_empty());
        assert_eq!(null_plugins, CompilerOptionsValue::NilList);

        let mut source = CompilerOptions::default();
        parse_compiler_options_worker("types", &json["types"], &mut source);
        parse_compiler_options_worker("lib", &json["lib"], &mut source);
        parse_compiler_options_worker("plugins", &null_plugins, &mut source);
        assert_eq!(source.types, None);
        assert_eq!(source.lib, Some(Vec::new()));
        assert_eq!(source.plugins, None);

        let mut target = CompilerOptions {
            types: Some(vec!["node".to_string()]),
            ..Default::default()
        };
        merge_compiler_options(&mut target, Some(&source), None);
        assert_eq!(target.types, Some(vec!["node".to_string()]));
    }
}
