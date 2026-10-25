//! The Go JSON v2 form of `core.CompilerOptions` and `core.TypeAcquisition`
//! (the `json` tags in core/compileroptions.go and core/typeacquisition.go),
//! and the v2 `omitempty` member rule.
//!
//! PORT: Go marshals these structs by reflection. The port wrote them by
//! hand for `api/proto.go`. Bump B wave 3 (tsgo#4712) moved them here from
//! `api/proto.rs`, because the content mappers (`crate::contentmapper`, on
//! the compiler side) use them too and must not depend on `api`.
//! `api::proto` re-exports them.

use crate::prelude::*;

use crate::frontend::json::{
    JsonDecoder, JsonError, MarshalerTo, UnmarshalerFrom, json_unmarshal_decode,
};
use crate::frontend::json_ext::{
    SemanticError, marshal_field, marshal_field_omitzero, marshal_opt_field,
    unmarshal_struct_fields, write_object_end, write_object_start,
};

/// Go v2 `omitempty`: the member is dropped when its value marshals as
/// `null`, `""`, `{}` or `[]` (v2 `UnwriteEmptyObjectMember`). A `false`
/// or `0` is written.
pub fn marshal_field_omitempty<T: MarshalerTo + ?Sized>(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: &T,
) -> Result<(), JsonError> {
    let mut v = String::new();
    value.marshal_json_to(&mut v)?;
    if matches!(v.as_str(), "null" | "\"\"" | "{}" | "[]") {
        return Ok(());
    }
    if !*first {
        enc.push(',');
    }
    *first = false;
    name.marshal_json_to(enc)?;
    enc.push(':');
    enc.push_str(&v);
    Ok(())
}

// ---------------------------------------------------------------------------
// core.CompilerOptions JSON
// ---------------------------------------------------------------------------

/// Go v2 marshal of `*core.CompilerOptions` (`ConfigFileResponse.Options`,
/// `ProjectResponse.CompilerOptions`).
/// PORT: Go marshals the struct by reflection over its `json` tags
/// (core/compileroptions.go:16, every field `omitzero`). The port has no
/// `MarshalerTo` for `CompilerOptions`, so this view writes the same members
/// in Go order: a `Tristate` writes its `MarshalJSON` text
/// (core/tristate.go:55), the enum types are Go integers, a `[]string` is
/// omitted only when nil, and `Paths` is an `OrderedMap` (insertion order;
/// a nil `[]string` value writes `[]`).
pub struct CompilerOptionsJSON<'a>(pub &'a CompilerOptions);

// A `Tristate` member with `omitzero` (`TSUnknown` is the zero value).
fn marshal_tristate_omitzero(
    enc: &mut String,
    first: &mut bool,
    name: &str,
    value: Tristate,
) -> Result<(), JsonError> {
    if value == Tristate::Unknown {
        return Ok(());
    }
    if !*first {
        enc.push(',');
    }
    *first = false;
    name.marshal_json_to(enc)?;
    enc.push(':');
    enc.push_str(std::str::from_utf8(value.marshal_json()).expect("Tristate JSON is ASCII"));
    Ok(())
}

// Go JSON v2 struct marshaler for `core.TypeAcquisition` (tags `enable`,
// `include`, `exclude` and `disableFilenameBasedTypeAcquisition`, all
// `omitzero`).
// PORT: a Go nil `Include` or `Exclude` is omitted and a non-nil empty one
// is written as `[]`. The Rust `Vec` has no nil, so an empty list is
// omitted.
pub struct TypeAcquisitionJSON<'a>(pub &'a crate::frontend::core_ext::TypeAcquisition);

impl MarshalerTo for TypeAcquisitionJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let ta = self.0;
        let first = &mut true;
        write_object_start(enc);
        marshal_tristate_omitzero(enc, first, "enable", ta.enable)?;
        if !ta.include.is_empty() {
            marshal_field(enc, first, "include", &ta.include)?;
        }
        if !ta.exclude.is_empty() {
            marshal_field(enc, first, "exclude", &ta.exclude)?;
        }
        marshal_tristate_omitzero(
            enc,
            first,
            "disableFilenameBasedTypeAcquisition",
            ta.disable_filename_based_type_acquisition,
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// Go `collections.OrderedMap[string, []string]` MarshalJSONTo.
struct PathsJSON<'a>(&'a IndexMap<String, Option<Vec<String>>>);

impl MarshalerTo for PathsJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('{');
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            k.marshal_json_to(enc)?;
            enc.push(':');
            match v {
                // A nil slice marshals as `[]` in v2.
                None => enc.push_str("[]"),
                Some(v) => v.marshal_json_to(enc)?,
            }
        }
        enc.push('}');
        Ok(())
    }
}

// Go: core/compileroptions.go:14 PluginImport (ts#64397), tag `json:"name"`.
// PORT: Go marshals and unmarshals it by reflection (the v2 default struct
// arshalers).
impl MarshalerTo for PluginImport {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "name", &self.name)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for PluginImport {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let p = &mut *self;
        let is_object = unmarshal_struct_fields(dec, "core.PluginImport", |name, dec| {
            match name {
                "name" => json_unmarshal_decode(dec, &mut p.name)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = PluginImport::default();
        }
        Ok(())
    }
}

impl MarshalerTo for CompilerOptionsJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let o = self.0;
        let first = &mut true;
        write_object_start(enc);
        marshal_tristate_omitzero(enc, first, "allowJs", o.allow_js)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "allowArbitraryExtensions",
            o.allow_arbitrary_extensions,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "allowImportingTsExtensions",
            o.allow_importing_ts_extensions,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "allowNonTsExtensions",
            o.allow_non_ts_extensions,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "allowUmdGlobalAccess",
            o.allow_umd_global_access,
        )?;
        marshal_tristate_omitzero(enc, first, "allowUnreachableCode", o.allow_unreachable_code)?;
        marshal_tristate_omitzero(enc, first, "allowUnusedLabels", o.allow_unused_labels)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "assumeChangesOnlyAffectDirectDependencies",
            o.assume_changes_only_affect_direct_dependencies,
        )?;
        marshal_tristate_omitzero(enc, first, "checkJs", o.check_js)?;
        marshal_opt_field(enc, first, "customConditions", &o.custom_conditions)?;
        marshal_tristate_omitzero(enc, first, "composite", o.composite)?;
        marshal_tristate_omitzero(enc, first, "emitDeclarationOnly", o.emit_declaration_only)?;
        marshal_tristate_omitzero(enc, first, "emitBOM", o.emit_bom)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "emitDecoratorMetadata",
            o.emit_decorator_metadata,
        )?;
        marshal_tristate_omitzero(enc, first, "declaration", o.declaration)?;
        marshal_field_omitzero(enc, first, "declarationDir", &o.declaration_dir)?;
        marshal_tristate_omitzero(enc, first, "declarationMap", o.declaration_map)?;
        marshal_tristate_omitzero(enc, first, "deduplicatePackages", o.deduplicate_packages)?;
        marshal_tristate_omitzero(enc, first, "disableSizeLimit", o.disable_size_limit)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "disableSourceOfProjectReferenceRedirect",
            o.disable_source_of_project_reference_redirect,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "disableSolutionSearching",
            o.disable_solution_searching,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "disableReferencedProjectLoad",
            o.disable_referenced_project_load,
        )?;
        marshal_tristate_omitzero(enc, first, "erasableSyntaxOnly", o.erasable_syntax_only)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "exactOptionalPropertyTypes",
            o.exact_optional_property_types,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "experimentalDecorators",
            o.experimental_decorators,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "forceConsistentCasingInFileNames",
            o.force_consistent_casing_in_file_names,
        )?;
        marshal_tristate_omitzero(enc, first, "isolatedModules", o.isolated_modules)?;
        marshal_tristate_omitzero(enc, first, "isolatedDeclarations", o.isolated_declarations)?;
        marshal_tristate_omitzero(enc, first, "ignoreConfig", o.ignore_config)?;
        marshal_field_omitzero(enc, first, "ignoreDeprecations", &o.ignore_deprecations)?;
        marshal_tristate_omitzero(enc, first, "importHelpers", o.import_helpers)?;
        marshal_tristate_omitzero(enc, first, "inlineSourceMap", o.inline_source_map)?;
        marshal_tristate_omitzero(enc, first, "inlineSources", o.inline_sources)?;
        marshal_tristate_omitzero(enc, first, "init", o.init)?;
        marshal_tristate_omitzero(enc, first, "incremental", o.incremental)?;
        marshal_field_omitzero(enc, first, "jsx", &o.jsx.0)?;
        marshal_field_omitzero(enc, first, "jsxFactory", &o.jsx_factory)?;
        marshal_field_omitzero(enc, first, "jsxFragmentFactory", &o.jsx_fragment_factory)?;
        marshal_field_omitzero(enc, first, "jsxImportSource", &o.jsx_import_source)?;
        marshal_opt_field(enc, first, "lib", &o.lib)?;
        marshal_tristate_omitzero(enc, first, "libReplacement", o.lib_replacement)?;
        marshal_field_omitzero(enc, first, "locale", &o.locale)?;
        marshal_field_omitzero(enc, first, "mapRoot", &o.map_root)?;
        marshal_field_omitzero(enc, first, "module", &o.module.0)?;
        marshal_field_omitzero(enc, first, "moduleResolution", &o.module_resolution.0)?;
        marshal_opt_field(enc, first, "moduleSuffixes", &o.module_suffixes)?;
        marshal_field_omitzero(enc, first, "moduleDetection", &o.module_detection.0)?;
        marshal_field_omitzero(enc, first, "newLine", &o.new_line.0)?;
        marshal_tristate_omitzero(enc, first, "noEmit", o.no_emit)?;
        marshal_tristate_omitzero(enc, first, "noCheck", o.no_check)?;
        marshal_tristate_omitzero(enc, first, "noErrorTruncation", o.no_error_truncation)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "noFallthroughCasesInSwitch",
            o.no_fallthrough_cases_in_switch,
        )?;
        marshal_tristate_omitzero(enc, first, "noImplicitAny", o.no_implicit_any)?;
        marshal_tristate_omitzero(enc, first, "noImplicitThis", o.no_implicit_this)?;
        marshal_tristate_omitzero(enc, first, "noImplicitReturns", o.no_implicit_returns)?;
        marshal_tristate_omitzero(enc, first, "noEmitHelpers", o.no_emit_helpers)?;
        marshal_tristate_omitzero(enc, first, "noLib", o.no_lib)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "noPropertyAccessFromIndexSignature",
            o.no_property_access_from_index_signature,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "noUncheckedIndexedAccess",
            o.no_unchecked_indexed_access,
        )?;
        marshal_tristate_omitzero(enc, first, "noEmitOnError", o.no_emit_on_error)?;
        marshal_tristate_omitzero(enc, first, "noUnusedLocals", o.no_unused_locals)?;
        marshal_tristate_omitzero(enc, first, "noUnusedParameters", o.no_unused_parameters)?;
        marshal_tristate_omitzero(enc, first, "noResolve", o.no_resolve)?;
        marshal_tristate_omitzero(enc, first, "noImplicitOverride", o.no_implicit_override)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "noUncheckedSideEffectImports",
            o.no_unchecked_side_effect_imports,
        )?;
        marshal_field_omitzero(enc, first, "outDir", &o.out_dir)?;
        marshal_opt_field(enc, first, "paths", &o.paths.as_ref().map(PathsJSON))?;
        // ts#64397: `plugins,omitzero` (a nil slice is omitted).
        marshal_opt_field(enc, first, "plugins", &o.plugins)?;
        marshal_tristate_omitzero(enc, first, "preserveConstEnums", o.preserve_const_enums)?;
        marshal_tristate_omitzero(enc, first, "preserveSymlinks", o.preserve_symlinks)?;
        marshal_field_omitzero(enc, first, "project", &o.project)?;
        marshal_tristate_omitzero(enc, first, "resolveJsonModule", o.resolve_json_module)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "resolvePackageJsonExports",
            o.resolve_package_json_exports,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "resolvePackageJsonImports",
            o.resolve_package_json_imports,
        )?;
        marshal_tristate_omitzero(enc, first, "removeComments", o.remove_comments)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "rewriteRelativeImportExtensions",
            o.rewrite_relative_import_extensions,
        )?;
        marshal_field_omitzero(enc, first, "reactNamespace", &o.react_namespace)?;
        marshal_field_omitzero(enc, first, "rootDir", &o.root_dir)?;
        marshal_opt_field(enc, first, "rootDirs", &o.root_dirs)?;
        marshal_tristate_omitzero(enc, first, "skipLibCheck", o.skip_lib_check)?;
        marshal_tristate_omitzero(enc, first, "stableTypeOrdering", o.stable_type_ordering)?;
        marshal_tristate_omitzero(enc, first, "strict", o.strict)?;
        marshal_tristate_omitzero(enc, first, "strictBindCallApply", o.strict_bind_call_apply)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "strictBuiltinIteratorReturn",
            o.strict_builtin_iterator_return,
        )?;
        marshal_tristate_omitzero(enc, first, "strictFunctionTypes", o.strict_function_types)?;
        marshal_tristate_omitzero(enc, first, "strictNullChecks", o.strict_null_checks)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "strictPropertyInitialization",
            o.strict_property_initialization,
        )?;
        marshal_tristate_omitzero(enc, first, "stripInternal", o.strip_internal)?;
        marshal_tristate_omitzero(enc, first, "skipDefaultLibCheck", o.skip_default_lib_check)?;
        marshal_tristate_omitzero(enc, first, "sourceMap", o.source_map)?;
        marshal_field_omitzero(enc, first, "sourceRoot", &o.source_root)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "suppressOutputPathCheck",
            o.suppress_output_path_check,
        )?;
        marshal_field_omitzero(enc, first, "target", &o.target.0)?;
        marshal_tristate_omitzero(enc, first, "traceResolution", o.trace_resolution)?;
        marshal_field_omitzero(enc, first, "tsBuildInfoFile", &o.ts_build_info_file)?;
        marshal_opt_field(enc, first, "typeRoots", &o.type_roots)?;
        marshal_opt_field(enc, first, "types", &o.types)?;
        marshal_tristate_omitzero(
            enc,
            first,
            "useDefineForClassFields",
            o.use_define_for_class_fields,
        )?;
        marshal_tristate_omitzero(
            enc,
            first,
            "useUnknownInCatchVariables",
            o.use_unknown_in_catch_variables,
        )?;
        marshal_tristate_omitzero(enc, first, "verbatimModuleSyntax", o.verbatim_module_syntax)?;
        marshal_opt_field(
            enc,
            first,
            "maxNodeModuleJsDepth",
            &o.max_node_module_js_depth,
        )?;

        // Deprecated: Do not use outside of options parsing and validation.
        marshal_tristate_omitzero(
            enc,
            first,
            "allowSyntheticDefaultImports",
            o.allow_synthetic_default_imports,
        )?;
        marshal_tristate_omitzero(enc, first, "alwaysStrict", o.always_strict)?;
        marshal_field_omitzero(enc, first, "baseUrl", &o.base_url)?;
        marshal_tristate_omitzero(enc, first, "downlevelIteration", o.downlevel_iteration)?;
        marshal_tristate_omitzero(enc, first, "esModuleInterop", o.es_module_interop)?;
        marshal_field_omitzero(enc, first, "outFile", &o.out_file)?;

        // Internal fields
        marshal_field_omitzero(enc, first, "configFilePath", &o.config_file_path)?;
        marshal_tristate_omitzero(enc, first, "noDtsResolution", o.no_dts_resolution)?;
        marshal_field_omitzero(enc, first, "pathsBasePath", &o.paths_base_path)?;
        marshal_tristate_omitzero(enc, first, "diagnostics", o.diagnostics)?;
        marshal_tristate_omitzero(enc, first, "extendedDiagnostics", o.extended_diagnostics)?;
        marshal_field_omitzero(enc, first, "generateCpuProfile", &o.generate_cpu_profile)?;
        marshal_field_omitzero(enc, first, "generateTrace", &o.generate_trace)?;
        marshal_tristate_omitzero(enc, first, "listEmittedFiles", o.list_emitted_files)?;
        marshal_tristate_omitzero(enc, first, "listFiles", o.list_files)?;
        marshal_tristate_omitzero(enc, first, "explainFiles", o.explain_files)?;
        marshal_tristate_omitzero(enc, first, "listFilesOnly", o.list_files_only)?;
        marshal_tristate_omitzero(enc, first, "noEmitForJsFiles", o.no_emit_for_js_files)?;
        marshal_tristate_omitzero(enc, first, "preserveWatchOutput", o.preserve_watch_output)?;
        marshal_tristate_omitzero(enc, first, "pretty", o.pretty)?;
        marshal_tristate_omitzero(enc, first, "version", o.version)?;
        marshal_tristate_omitzero(enc, first, "watch", o.watch)?;
        marshal_tristate_omitzero(enc, first, "showConfig", o.show_config)?;
        marshal_tristate_omitzero(enc, first, "build", o.build)?;
        marshal_tristate_omitzero(enc, first, "help", o.help)?;
        marshal_tristate_omitzero(enc, first, "all", o.all)?;
        // tsgo#4712
        marshal_tristate_omitzero(enc, first, "runExternalCode", o.run_external_code)?;

        marshal_field_omitzero(enc, first, "pprofDir", &o.pprof_dir)?;
        marshal_tristate_omitzero(enc, first, "singleThreaded", o.single_threaded)?;
        marshal_tristate_omitzero(enc, first, "quiet", o.quiet)?;
        marshal_opt_field(enc, first, "checkers", &o.checkers)?;
        write_object_end(enc);
        Ok(())
    }
}

/// Go v2 default unmarshal of `core.CompilerOptions`
/// (`TranspileOptions.CompilerOptions`, tsgo#4849).
/// PORT: Go decodes the struct by reflection over its `json` tags
/// (core/compileroptions.go:16). This matches the same member names, in Go
/// order, case-sensitive; unknown names are skipped and `null` sets the zero
/// struct. A `Tristate` calls its legacy `UnmarshalJSON` with the raw value
/// (core/tristate.go:43), the enum types are Go `int32` types, a `[]string`
/// is `None` for `null`, and `Paths` (`*collections.OrderedMap`) uses the
/// `IndexMap` impl of `OrderedMap.UnmarshalJSONFrom`.
impl UnmarshalerFrom for CompilerOptions {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let o = &mut *self;
        let is_object = unmarshal_struct_fields(dec, "core.CompilerOptions", |name, dec| {
            match name {
                "allowJs" => unmarshal_tristate(dec, &mut o.allow_js)?,
                "allowArbitraryExtensions" => {
                    unmarshal_tristate(dec, &mut o.allow_arbitrary_extensions)?
                }
                "allowImportingTsExtensions" => {
                    unmarshal_tristate(dec, &mut o.allow_importing_ts_extensions)?
                }
                "allowNonTsExtensions" => unmarshal_tristate(dec, &mut o.allow_non_ts_extensions)?,
                "allowUmdGlobalAccess" => unmarshal_tristate(dec, &mut o.allow_umd_global_access)?,
                "allowUnreachableCode" => unmarshal_tristate(dec, &mut o.allow_unreachable_code)?,
                "allowUnusedLabels" => unmarshal_tristate(dec, &mut o.allow_unused_labels)?,
                "assumeChangesOnlyAffectDirectDependencies" => {
                    unmarshal_tristate(dec, &mut o.assume_changes_only_affect_direct_dependencies)?
                }
                "checkJs" => unmarshal_tristate(dec, &mut o.check_js)?,
                "customConditions" => json_unmarshal_decode(dec, &mut o.custom_conditions)?,
                "composite" => unmarshal_tristate(dec, &mut o.composite)?,
                "emitDeclarationOnly" => unmarshal_tristate(dec, &mut o.emit_declaration_only)?,
                "emitBOM" => unmarshal_tristate(dec, &mut o.emit_bom)?,
                "emitDecoratorMetadata" => unmarshal_tristate(dec, &mut o.emit_decorator_metadata)?,
                "declaration" => unmarshal_tristate(dec, &mut o.declaration)?,
                "declarationDir" => json_unmarshal_decode(dec, &mut o.declaration_dir)?,
                "declarationMap" => unmarshal_tristate(dec, &mut o.declaration_map)?,
                "deduplicatePackages" => unmarshal_tristate(dec, &mut o.deduplicate_packages)?,
                "disableSizeLimit" => unmarshal_tristate(dec, &mut o.disable_size_limit)?,
                "disableSourceOfProjectReferenceRedirect" => {
                    unmarshal_tristate(dec, &mut o.disable_source_of_project_reference_redirect)?
                }
                "disableSolutionSearching" => {
                    unmarshal_tristate(dec, &mut o.disable_solution_searching)?
                }
                "disableReferencedProjectLoad" => {
                    unmarshal_tristate(dec, &mut o.disable_referenced_project_load)?
                }
                "erasableSyntaxOnly" => unmarshal_tristate(dec, &mut o.erasable_syntax_only)?,
                "exactOptionalPropertyTypes" => {
                    unmarshal_tristate(dec, &mut o.exact_optional_property_types)?
                }
                "experimentalDecorators" => {
                    unmarshal_tristate(dec, &mut o.experimental_decorators)?
                }
                "forceConsistentCasingInFileNames" => {
                    unmarshal_tristate(dec, &mut o.force_consistent_casing_in_file_names)?
                }
                "isolatedModules" => unmarshal_tristate(dec, &mut o.isolated_modules)?,
                "isolatedDeclarations" => unmarshal_tristate(dec, &mut o.isolated_declarations)?,
                "ignoreConfig" => unmarshal_tristate(dec, &mut o.ignore_config)?,
                "ignoreDeprecations" => json_unmarshal_decode(dec, &mut o.ignore_deprecations)?,
                "importHelpers" => unmarshal_tristate(dec, &mut o.import_helpers)?,
                "inlineSourceMap" => unmarshal_tristate(dec, &mut o.inline_source_map)?,
                "inlineSources" => unmarshal_tristate(dec, &mut o.inline_sources)?,
                "init" => unmarshal_tristate(dec, &mut o.init)?,
                "incremental" => unmarshal_tristate(dec, &mut o.incremental)?,
                "jsx" => unmarshal_core_int(dec, &mut o.jsx.0, "core.JsxEmit")?,
                "jsxFactory" => json_unmarshal_decode(dec, &mut o.jsx_factory)?,
                "jsxFragmentFactory" => json_unmarshal_decode(dec, &mut o.jsx_fragment_factory)?,
                "jsxImportSource" => json_unmarshal_decode(dec, &mut o.jsx_import_source)?,
                "lib" => json_unmarshal_decode(dec, &mut o.lib)?,
                "libReplacement" => unmarshal_tristate(dec, &mut o.lib_replacement)?,
                "locale" => json_unmarshal_decode(dec, &mut o.locale)?,
                "mapRoot" => json_unmarshal_decode(dec, &mut o.map_root)?,
                "module" => unmarshal_core_int(dec, &mut o.module.0, "core.ModuleKind")?,
                "moduleResolution" => unmarshal_core_int(
                    dec,
                    &mut o.module_resolution.0,
                    "core.ModuleResolutionKind",
                )?,
                "moduleSuffixes" => json_unmarshal_decode(dec, &mut o.module_suffixes)?,
                "moduleDetection" => {
                    unmarshal_core_int(dec, &mut o.module_detection.0, "core.ModuleDetectionKind")?
                }
                "newLine" => unmarshal_core_int(dec, &mut o.new_line.0, "core.NewLineKind")?,
                "noEmit" => unmarshal_tristate(dec, &mut o.no_emit)?,
                "noCheck" => unmarshal_tristate(dec, &mut o.no_check)?,
                "noErrorTruncation" => unmarshal_tristate(dec, &mut o.no_error_truncation)?,
                "noFallthroughCasesInSwitch" => {
                    unmarshal_tristate(dec, &mut o.no_fallthrough_cases_in_switch)?
                }
                "noImplicitAny" => unmarshal_tristate(dec, &mut o.no_implicit_any)?,
                "noImplicitThis" => unmarshal_tristate(dec, &mut o.no_implicit_this)?,
                "noImplicitReturns" => unmarshal_tristate(dec, &mut o.no_implicit_returns)?,
                "noEmitHelpers" => unmarshal_tristate(dec, &mut o.no_emit_helpers)?,
                "noLib" => unmarshal_tristate(dec, &mut o.no_lib)?,
                "noPropertyAccessFromIndexSignature" => {
                    unmarshal_tristate(dec, &mut o.no_property_access_from_index_signature)?
                }
                "noUncheckedIndexedAccess" => {
                    unmarshal_tristate(dec, &mut o.no_unchecked_indexed_access)?
                }
                "noEmitOnError" => unmarshal_tristate(dec, &mut o.no_emit_on_error)?,
                "noUnusedLocals" => unmarshal_tristate(dec, &mut o.no_unused_locals)?,
                "noUnusedParameters" => unmarshal_tristate(dec, &mut o.no_unused_parameters)?,
                "noResolve" => unmarshal_tristate(dec, &mut o.no_resolve)?,
                "noImplicitOverride" => unmarshal_tristate(dec, &mut o.no_implicit_override)?,
                "noUncheckedSideEffectImports" => {
                    unmarshal_tristate(dec, &mut o.no_unchecked_side_effect_imports)?
                }
                "outDir" => json_unmarshal_decode(dec, &mut o.out_dir)?,
                "paths" => json_unmarshal_decode(dec, &mut o.paths)?,
                // ts#64397: `null` leaves a nil slice.
                "plugins" => json_unmarshal_decode(dec, &mut o.plugins)?,
                "preserveConstEnums" => unmarshal_tristate(dec, &mut o.preserve_const_enums)?,
                "preserveSymlinks" => unmarshal_tristate(dec, &mut o.preserve_symlinks)?,
                "project" => json_unmarshal_decode(dec, &mut o.project)?,
                "resolveJsonModule" => unmarshal_tristate(dec, &mut o.resolve_json_module)?,
                "resolvePackageJsonExports" => {
                    unmarshal_tristate(dec, &mut o.resolve_package_json_exports)?
                }
                "resolvePackageJsonImports" => {
                    unmarshal_tristate(dec, &mut o.resolve_package_json_imports)?
                }
                "removeComments" => unmarshal_tristate(dec, &mut o.remove_comments)?,
                "rewriteRelativeImportExtensions" => {
                    unmarshal_tristate(dec, &mut o.rewrite_relative_import_extensions)?
                }
                "reactNamespace" => json_unmarshal_decode(dec, &mut o.react_namespace)?,
                "rootDir" => json_unmarshal_decode(dec, &mut o.root_dir)?,
                "rootDirs" => json_unmarshal_decode(dec, &mut o.root_dirs)?,
                "skipLibCheck" => unmarshal_tristate(dec, &mut o.skip_lib_check)?,
                "stableTypeOrdering" => unmarshal_tristate(dec, &mut o.stable_type_ordering)?,
                "strict" => unmarshal_tristate(dec, &mut o.strict)?,
                "strictBindCallApply" => unmarshal_tristate(dec, &mut o.strict_bind_call_apply)?,
                "strictBuiltinIteratorReturn" => {
                    unmarshal_tristate(dec, &mut o.strict_builtin_iterator_return)?
                }
                "strictFunctionTypes" => unmarshal_tristate(dec, &mut o.strict_function_types)?,
                "strictNullChecks" => unmarshal_tristate(dec, &mut o.strict_null_checks)?,
                "strictPropertyInitialization" => {
                    unmarshal_tristate(dec, &mut o.strict_property_initialization)?
                }
                "stripInternal" => unmarshal_tristate(dec, &mut o.strip_internal)?,
                "skipDefaultLibCheck" => unmarshal_tristate(dec, &mut o.skip_default_lib_check)?,
                "sourceMap" => unmarshal_tristate(dec, &mut o.source_map)?,
                "sourceRoot" => json_unmarshal_decode(dec, &mut o.source_root)?,
                "suppressOutputPathCheck" => {
                    unmarshal_tristate(dec, &mut o.suppress_output_path_check)?
                }
                "target" => unmarshal_core_int(dec, &mut o.target.0, "core.ScriptTarget")?,
                "traceResolution" => unmarshal_tristate(dec, &mut o.trace_resolution)?,
                "tsBuildInfoFile" => json_unmarshal_decode(dec, &mut o.ts_build_info_file)?,
                "typeRoots" => json_unmarshal_decode(dec, &mut o.type_roots)?,
                "types" => json_unmarshal_decode(dec, &mut o.types)?,
                "useDefineForClassFields" => {
                    unmarshal_tristate(dec, &mut o.use_define_for_class_fields)?
                }
                "useUnknownInCatchVariables" => {
                    unmarshal_tristate(dec, &mut o.use_unknown_in_catch_variables)?
                }
                "verbatimModuleSyntax" => unmarshal_tristate(dec, &mut o.verbatim_module_syntax)?,
                "maxNodeModuleJsDepth" => {
                    unmarshal_go_int_ptr(dec, &mut o.max_node_module_js_depth)?
                }
                "allowSyntheticDefaultImports" => {
                    unmarshal_tristate(dec, &mut o.allow_synthetic_default_imports)?
                }
                "alwaysStrict" => unmarshal_tristate(dec, &mut o.always_strict)?,
                "baseUrl" => json_unmarshal_decode(dec, &mut o.base_url)?,
                "downlevelIteration" => unmarshal_tristate(dec, &mut o.downlevel_iteration)?,
                "esModuleInterop" => unmarshal_tristate(dec, &mut o.es_module_interop)?,
                "outFile" => json_unmarshal_decode(dec, &mut o.out_file)?,
                "configFilePath" => json_unmarshal_decode(dec, &mut o.config_file_path)?,
                "noDtsResolution" => unmarshal_tristate(dec, &mut o.no_dts_resolution)?,
                "pathsBasePath" => json_unmarshal_decode(dec, &mut o.paths_base_path)?,
                "diagnostics" => unmarshal_tristate(dec, &mut o.diagnostics)?,
                "extendedDiagnostics" => unmarshal_tristate(dec, &mut o.extended_diagnostics)?,
                "generateCpuProfile" => json_unmarshal_decode(dec, &mut o.generate_cpu_profile)?,
                "generateTrace" => json_unmarshal_decode(dec, &mut o.generate_trace)?,
                "listEmittedFiles" => unmarshal_tristate(dec, &mut o.list_emitted_files)?,
                "listFiles" => unmarshal_tristate(dec, &mut o.list_files)?,
                "explainFiles" => unmarshal_tristate(dec, &mut o.explain_files)?,
                "listFilesOnly" => unmarshal_tristate(dec, &mut o.list_files_only)?,
                "noEmitForJsFiles" => unmarshal_tristate(dec, &mut o.no_emit_for_js_files)?,
                "preserveWatchOutput" => unmarshal_tristate(dec, &mut o.preserve_watch_output)?,
                "pretty" => unmarshal_tristate(dec, &mut o.pretty)?,
                "version" => unmarshal_tristate(dec, &mut o.version)?,
                "watch" => unmarshal_tristate(dec, &mut o.watch)?,
                "showConfig" => unmarshal_tristate(dec, &mut o.show_config)?,
                "build" => unmarshal_tristate(dec, &mut o.build)?,
                "help" => unmarshal_tristate(dec, &mut o.help)?,
                "all" => unmarshal_tristate(dec, &mut o.all)?,
                // tsgo#4712
                "runExternalCode" => unmarshal_tristate(dec, &mut o.run_external_code)?,
                "pprofDir" => json_unmarshal_decode(dec, &mut o.pprof_dir)?,
                "singleThreaded" => unmarshal_tristate(dec, &mut o.single_threaded)?,
                "quiet" => unmarshal_tristate(dec, &mut o.quiet)?,
                "checkers" => unmarshal_go_int_ptr(dec, &mut o.checkers)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = CompilerOptions::default();
        }
        Ok(())
    }
}

// Go v2 calls the legacy `Tristate.UnmarshalJSON` with the raw value, also
// for `null` (arshal_methods.go:286). It never fails.
fn unmarshal_tristate(dec: &mut JsonDecoder<'_>, t: &mut Tristate) -> Result<(), JsonError> {
    let val = dec.read_value()?;
    t.unmarshal_json(val);
    Ok(())
}

// Go v2 int arshaler for a Go named `int32` type (`core.ModuleKind`): the
// `int32` decode, with errors that name `go_type`.
fn unmarshal_core_int(
    dec: &mut JsonDecoder<'_>,
    v: &mut i32,
    go_type: &str,
) -> Result<(), JsonError> {
    v.unmarshal_json_from(dec)
        .map_err(|err| match SemanticError::of(&err) {
            Some(mut s) => {
                s.go_type = go_type.to_string();
                s.into_json_error()
            }
            None => err,
        })
}

// Go v2 pointer arshaler for a `*int` member: `null` sets nil. Go `int` is
// 64-bit; errors name Go `int`.
pub fn unmarshal_go_int_ptr(
    dec: &mut JsonDecoder<'_>,
    v: &mut Option<i64>,
) -> Result<(), JsonError> {
    if dec.peek_kind() == b'n' {
        dec.read_token()?;
        *v = None;
        return Ok(());
    }
    // Go allocates the int before it decodes into it.
    let n = v.get_or_insert(0);
    *n = crate::frontend::json_ext::unmarshal_int_as::<i64>(dec, "int")?;
    Ok(())
}
