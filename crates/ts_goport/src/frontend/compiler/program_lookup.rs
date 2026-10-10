//! Port of compiler/program.go lines 1788 to 2190: file lookup, include
//! explanations, resolution lookups, package name collection, the symlink
//! cache and the `plainJSErrors` set.

use crate::frontend::prelude::*;
use std::sync::{Arc, OnceLock};

// Go: program.go:91 packageNamesInfo
// PORT: U21 dropped the `packageNames` field and this type. It is here
// because only `collectPackageNames` uses it. Go `*collections.Set[string]`
// is an `FxHashSet<String>`.
#[derive(Clone, Debug, Default)]
pub struct PackageNamesInfo {
    pub resolved: FxHashSet<String>,
    pub unresolved: FxHashSet<String>,
    pub deep_import_packages: FxHashSet<String>,
}

/// Go callback `func(resolution T, moduleName string, mode core.ResolutionMode, filePath tspath.Path)`.
pub type ResolutionCallback<'f, T> = dyn FnMut(&T, &str, ResolutionMode, &Path) + 'f;

impl NewProgram {
    // Go: program.go:2067 (*Program).toPath (at 673a5f17d713; ts#64159 makes it PathKeyForFileName, compiler/program.go:2090)
    pub fn to_path(&self, filename: &str) -> Path {
        to_path(
            filename,
            &self.get_current_directory(),
            self.use_case_sensitive_file_names(),
        )
    }

    // Go: program.go:2094 (*Program).GetSourceFile
    pub fn get_source_file(&self, filename: &str) -> Option<Rc<ParsedSourceFile>> {
        let path = self.to_path(filename);
        self.get_source_file_by_path(&path)
    }

    // Go: program.go:2098 (*Program).GetSourceFileForResolvedModule
    pub fn get_source_file_for_resolved_module(
        &self,
        file_name: &str,
    ) -> Option<Rc<ParsedSourceFile>> {
        let file = self.get_source_file(file_name);
        if file.is_none() {
            let filename = self.get_parse_file_redirect(file_name);
            if !filename.is_empty() {
                return self.get_source_file(&filename);
            }
        }
        file
    }

    // Go: program.go:2113 (*Program).FilesByPath
    pub fn files_by_path(&self) -> &FxHashMap<Path, Rc<ParsedSourceFile>> {
        &self.processed_files.files_by_path
    }

    // Go: program.go:2117 (*Program).GetSourceFileByPath
    pub fn get_source_file_by_path(&self, path: &Path) -> Option<Rc<ParsedSourceFile>> {
        self.processed_files.files_by_path.get(path).cloned()
    }

    // Go: program.go:2121 (*Program).HasSameFileNames
    // PORT: Go `maps.EqualFunc` treats a nil map as empty; a `None`
    // `redirect_files_by_path` is an empty map here.
    pub fn has_same_file_names(&self, other: &NewProgram) -> bool {
        fn equal_maps<V>(
            a: &FxHashMap<Path, V>,
            b: &FxHashMap<Path, V>,
            eq: impl Fn(&V, &V) -> bool,
        ) -> bool {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v1)| b.get(k).is_some_and(|v2| eq(v1, v2)))
        }
        let empty = FxHashMap::default();
        equal_maps(
            &self.processed_files.files_by_path,
            &other.processed_files.files_by_path,
            |a, b| {
                // checks for casing differences on case-insensitive file systems
                a.file_name() == b.file_name()
            },
        ) && equal_maps(
            self.processed_files
                .redirect_files_by_path
                .as_deref()
                .unwrap_or(&empty),
            other
                .processed_files
                .redirect_files_by_path
                .as_deref()
                .unwrap_or(&empty),
            |a, b| a.file_name() == b.file_name(),
        )
    }

    // Go: program.go:2130 (*Program).GetSourceFiles
    pub fn get_source_files(&self) -> &[Rc<ParsedSourceFile>] {
        &self.processed_files.files
    }

    // Go: program.go:2135 (*Program).GetIncludeReasons
    // Testing only
    pub fn get_include_reasons(&self) -> &FxHashMap<Path, Vec<Rc<FileIncludeReason>>> {
        &self.processed_files.include_processor.file_include_reasons
    }

    // Go: program.go:2140 (*Program).IsMissingPath
    // Testing only
    pub fn is_missing_path(&self, path: &Path) -> bool {
        self.processed_files
            .missing_files
            .iter()
            .any(|missing_path| self.to_path(missing_path) == *path)
    }

    // Go: program.go:2144 (*Program).ExplainFiles
    // Each line is one write, as Go's `fmt.Fprintln`; its errors are ignored.
    // Go `fmt.Fprintln(w, "  ", x)` puts one more space between the operands.
    // PORT: the Go `explainFile` closure increments `filesExplained`; here
    // the callers do it, so the loop condition can read the counter.
    // ts#64159: the names are relative to `current_directory` (the system
    // current directory, execute/tsc/emit.go:156), not to the program's.
    pub fn explain_files(
        &self,
        w: &mut dyn std::io::Write,
        locale: &crate::locale::Locale,
        current_directory: &str,
    ) {
        let to_relative_file_name = |file_name: &str| {
            super::file_include::relative_file_name_from_directory(
                current_directory,
                file_name,
                self,
            )
        };
        let explain_file = |w: &mut dyn std::io::Write, file: &dyn HasFileName| {
            let _ =
                w.write_all(format!("{}\n", to_relative_file_name(&file.file_name())).as_bytes());
            if let Some(reasons) = self
                .processed_files
                .include_processor
                .file_include_reasons
                .get(&file.path())
            {
                for reason in reasons {
                    let line = format!(
                        "   {}\n",
                        reason
                            .to_diagnostic(self, true, current_directory)
                            .localize(locale)
                    );
                    let _ = w.write_all(line.as_bytes());
                }
            }
            for diag in self
                .processed_files
                .include_processor
                .explain_redirect_and_implied_format(self, &file.path(), to_relative_file_name)
            {
                let _ = w.write_all(format!("   {}\n", diag.localize(locale)).as_bytes());
            }
        };
        let mut files_explained: i32 = 0;

        let mut redirect_files: Vec<&RedirectsFile> = self
            .processed_files
            .redirect_files_by_path
            .iter()
            .flat_map(|m| m.values())
            .collect();
        // Go: compiler/program.go:2063 slices.SortFunc(redirectFiles, a.index - b.index)
        crate::gostd::slices::sort_func(&mut redirect_files, |a, b| a.index.cmp(&b.index) as i32);

        let files = self.get_source_files();
        let mut source_file_index = 0;
        let mut explain_source_files =
            |w: &mut dyn std::io::Write, files_explained: &mut i32, end_index: i32| {
                while *files_explained < end_index {
                    explain_file(w, &*files[source_file_index]);
                    *files_explained += 1;
                    source_file_index += 1;
                }
            };

        for redirect_file in &redirect_files {
            // Explain all sourceFiles till we reach this redirectFile index
            explain_source_files(w, &mut files_explained, redirect_file.index);
            explain_file(w, *redirect_file);
            files_explained += 1;
        }

        // Explain any remaining sourceFiles
        explain_source_files(
            w,
            &mut files_explained,
            (files.len() + redirect_files.len()) as i32,
        );
    }

    // Go: program.go:2187 (*Program).GetLibFileFromReference
    pub fn get_lib_file_from_reference(
        &self,
        ref_: &FileReference,
    ) -> Option<Rc<ParsedSourceFile>> {
        let (name, ok) = get_lib_file_name(&ref_.file_name);
        if !ok {
            return None;
        }
        // ts#64159 (program.go:2187): the lib file of that name. N looked the
        // bare name ("lib.dom.d.ts") up as a path and found nothing.
        self.processed_files
            .lib_files
            .iter()
            .find(|(_, lib_file)| lib_file.name == name)
            .and_then(|(path, _)| self.processed_files.files_by_path.get(path).cloned())
    }

    // Go: program.go:2200 (*Program).GetResolvedTypeReferenceDirectiveFromTypeReferenceDirective
    pub fn get_resolved_type_reference_directive_from_type_reference_directive(
        &self,
        type_ref: &FileReference,
        source_file: &ParsedSourceFile,
    ) -> Option<Rc<ResolvedTypeReferenceDirective>> {
        self.get_resolved_type_reference_directive(
            source_file,
            &type_ref.file_name,
            self.get_mode_for_type_reference_directive_in_file(type_ref, source_file),
        )
    }

    // Go: program.go:2204 (*Program).GetResolvedTypeReferenceDirective (ts#64247)
    pub fn get_resolved_type_reference_directive(
        &self,
        file: &dyn HasFileName,
        type_directive_name: &str,
        mode: ResolutionMode,
    ) -> Option<Rc<ResolvedTypeReferenceDirective>> {
        let resolutions = self
            .processed_files
            .type_resolutions_in_file
            .get(&file.path())?;
        let key = ModeAwareCacheKey {
            name: type_directive_name.to_string(),
            mode,
        };
        resolutions.get(&key).cloned()
    }

    // Go: program.go:2213 (*Program).GetResolvedTypeReferenceDirectives
    pub fn get_resolved_type_reference_directives(
        &self,
    ) -> &FxHashMap<Path, ModeAwareCache<Rc<ResolvedTypeReferenceDirective>>> {
        &self.processed_files.type_resolutions_in_file
    }

    // Go: program.go:2217 (*Program).getModeForTypeReferenceDirectiveInFile
    pub fn get_mode_for_type_reference_directive_in_file(
        &self,
        ref_: &FileReference,
        source_file: &ParsedSourceFile,
    ) -> ResolutionMode {
        if ref_.resolution_mode != RESOLUTION_MODE_NONE {
            return ref_.resolution_mode;
        }
        self.get_default_resolution_mode_for_file(source_file)
    }

    // Go: program.go:2224 (*Program).IsSourceFileFromExternalLibrary
    pub fn is_source_file_from_external_library(&self, file: &ParsedSourceFile) -> bool {
        self.processed_files
            .source_files_found_searching_node_modules
            .contains(file.path())
    }

    // Go: program.go:2228 (*Program).GetJSXRuntimeImportSpecifier
    // PORT: a Go nil map is `None`.
    pub fn get_jsx_runtime_import_specifier(&self, path: &Path) -> (String, Node) {
        if let Some(result) = self
            .processed_files
            .jsx_runtime_import_specifiers
            .as_ref()
            .and_then(|m| m.get(path))
        {
            return (result.module_reference.clone(), result.specifier);
        }
        (String::new(), Node::NIL)
    }

    // Go: program.go:2235 (*Program).GetImportHelpersImportSpecifier
    // PORT: a Go nil map is `None`; a missing entry is `Node::NIL`.
    pub fn get_import_helpers_import_specifier(&self, path: &Path) -> Node {
        self.processed_files
            .import_helpers_import_specifiers
            .as_ref()
            .and_then(|m| m.get(path).copied())
            .unwrap_or(Node::NIL)
    }

    // Go: program.go:2239 (*Program).SourceFileMayBeEmitted
    pub fn source_file_may_be_emitted(
        &self,
        source_file: &ParsedSourceFile,
        force_dts_emit: bool,
    ) -> bool {
        // #4699: Go passes `false` for the new `forceJsEmit`.
        source_file_may_be_emitted(source_file, self, force_dts_emit, false)
    }

    // Go: program.go:2243 (*Program).ResolvedPackageNames
    pub fn resolved_package_names(&self) -> &FxHashSet<String> {
        &self.collect_package_names().resolved
    }

    // Go: program.go:2247 (*Program).UnresolvedPackageNames
    pub fn unresolved_package_names(&self) -> &FxHashSet<String> {
        &self.collect_package_names().unresolved
    }

    // Go: program.go:2251 (*Program).DeepImportPackageNames
    pub fn deep_import_package_names(&self) -> &FxHashSet<String> {
        &self.collect_package_names().deep_import_packages
    }

    // Go: program.go:2255 (*Program).collectPackageNames
    // PORT: `package_names` is Go `packageNames lazyValue[*packageNamesInfo]`.
    fn collect_package_names(&self) -> &PackageNamesInfo {
        self.package_names.get_value(|| {
            let mut package_names = PackageNamesInfo::default();
            let resolver = self
                .processed_files
                .resolver
                .as_ref()
                .expect("program has a resolver");
            for file in &self.processed_files.files {
                if self.is_source_file_default_library(file.path())
                    || self.is_source_file_from_external_library(file)
                    || file.file_name().contains("/node_modules/")
                {
                    // Checking for /node_modules/ is a little imprecise, but ATA treats locally installed typings
                    // as root files, which would not pass IsSourceFileFromExternalLibrary.
                    continue;
                }
                for &imp in &file.imports {
                    // ts#63915
                    if is_source_phase_import(imp.parent())
                        || is_external_module_name_relative(imp.text())
                    {
                        continue;
                    }
                    if let Some(resolved_modules) =
                        self.processed_files.resolved_modules.get(file.path())
                    {
                        let key = ModeAwareCacheKey {
                            name: imp.text().to_string(),
                            mode: self.get_mode_for_usage_location(&**file, imp),
                        };
                        if let Some(resolved_module) = resolved_modules.get(&key)
                            && resolved_module.is_resolved()
                        {
                            if !resolved_module.is_external_library_import {
                                continue;
                            }
                            // Priority order for getting package name:
                            // 1. PackageId.Name (requires both name and version in package.json)
                            let mut name = resolved_module.package_id.name.clone();
                            if name.is_empty() {
                                // 2. GetPackageScopeForPath - get name from package.json in the package directory
                                if let Some(package_scope) = resolver
                                    .get_package_scope_for_path(&resolved_module.resolved_file_name)
                                    && package_scope.exists()
                                {
                                    let (scope_name, ok) = package_scope
                                        .get_contents()
                                        .expect("package scope exists")
                                        .header_fields
                                        .name
                                        .get_value();
                                    if ok {
                                        name = scope_name;
                                    }
                                }
                            }
                            if name.is_empty() {
                                // 3. GetPackageNameFromDirectory - extract from node_modules path
                                name = get_package_name_from_directory(
                                    &resolved_module.resolved_file_name,
                                );
                            }
                            // 4. If all fail, don't add empty string
                            if !name.is_empty() {
                                package_names.resolved.insert(name.clone());
                                // Detect deep imports: subpath imports in packages without exports.
                                // These are imports like "lodash/fp" where the package has no exports
                                // map, so auto-import can only find them via recursive directory search.
                                let (_, rest) = parse_package_name(imp.text());
                                if !rest.is_empty()
                                    && let Some(scope) = resolver.get_package_scope_for_path(
                                        &resolved_module.resolved_file_name,
                                    )
                                    && scope.exists()
                                    && !scope
                                        .get_contents()
                                        .expect("package scope exists")
                                        .path_fields
                                        .exports
                                        .is_present()
                                {
                                    package_names
                                        .deep_import_packages
                                        .insert(get_package_name_from_types_package_name(&name));
                                }
                            }
                            continue;
                        }
                    }
                    package_names.unresolved.insert(imp.text().to_string());
                }
            }
            Rc::new(package_names)
        })
    }

    // Go: program.go:2313 (*Program).IsLibFile
    pub fn is_lib_file(&self, source_file: &ParsedSourceFile) -> bool {
        self.processed_files
            .lib_files
            .contains_key(source_file.path())
    }

    // Go: program.go:2318 (*Program).HasTSFile
    // PORT: Go `hasTSFileOnce` plus `hasTSFile` is `has_ts_file: OnceCell<bool>`.
    pub fn has_ts_file(&self) -> bool {
        *self.has_ts_file.get_or_init(|| {
            self.processed_files
                .files
                .iter()
                .any(|file| has_implementation_ts_file_extension(file.file_name()))
        })
    }

    // Go: program.go:2330 (*Program).GetSymlinkCache
    // PORT: `known_symlinks` is Go `knownSymlinks lazyValue[*symlinks.KnownSymlinks]`.
    pub fn get_symlink_cache(&self) -> Rc<KnownSymlinks> {
        self.known_symlinks
            .get_value(|| Rc::new(self.build_symlink_cache()))
            .clone()
    }

    /// The symlink cache when the program has computed it (Go
    /// `knownSymlinks.initialized`).
    pub fn symlink_cache_if_built(&self) -> Option<Rc<KnownSymlinks>> {
        self.known_symlinks.get().cloned()
    }

    /// The value that Go `GetSymlinkCache` computes on first use, without
    /// storing it in the program.
    // Go: program.go:2301 (the getValue callback)
    pub fn build_symlink_cache(&self) -> KnownSymlinks {
        let mut known_symlinks = KnownSymlinks::new(
            &self.get_current_directory(),
            self.use_case_sensitive_file_names(),
        );

        // Resolved modules store realpath information when they're resolved inside node_modules
        if !self.processed_files.resolved_modules.is_empty()
            || !self.processed_files.type_resolutions_in_file.is_empty()
        {
            known_symlinks.set_symlinks_from_resolutions(
                &|callback: &mut ResolutionCallback<'_, Arc<ResolvedModule>>,
                  file: Option<&ParsedSourceFile>| {
                    self.for_each_resolved_module(callback, file);
                },
                &|callback: &mut ResolutionCallback<'_, Rc<ResolvedTypeReferenceDirective>>,
                  file: Option<&ParsedSourceFile>| {
                    self.for_each_resolved_type_reference_directive(callback, file);
                },
            );
        }

        // Check other dependencies for symlinks
        let resolver = self
            .processed_files
            .resolver
            .as_ref()
            .expect("program has a resolver");
        let mut seen_package_jsons: FxHashSet<Path> = FxHashSet::default();
        for (file_path, meta) in self.processed_files.source_file_meta_datas.iter() {
            if meta.package_json_directory.is_empty() {
                continue;
            }
            // PORT: Go passes a possibly nil file to `SourceFileMayBeEmitted`.
            let source_file = self.get_source_file_by_path(file_path);
            if !source_file.is_some_and(|f| self.source_file_may_be_emitted(&f, false))
                || !seen_package_jsons.insert(self.to_path(&meta.package_json_directory))
            {
                continue;
            }
            let package_json_name = combine_paths(&meta.package_json_directory, &["package.json"]);
            let info = self.get_package_json_info(&package_json_name);
            let Some(contents) = info.as_ref().and_then(|info| info.get_contents()) else {
                continue;
            };

            for dep in contents.get_runtime_dependency_names() {
                // Skip work in common case: we already saved a symlink for this package directory
                // in the node_modules adjacent to this package.json
                let possible_directory_path = self.to_path(&combine_paths(
                    &meta.package_json_directory,
                    &["node_modules", &dep],
                ));
                if known_symlinks.has_directory(&possible_directory_path) {
                    continue;
                }
                if !dep.starts_with("@types") {
                    let possible_types_directory_path = self.to_path(&combine_paths(
                        &meta.package_json_directory,
                        &["node_modules", &get_types_package_name(&dep)],
                    ));
                    if known_symlinks.has_directory(&possible_types_directory_path) {
                        continue;
                    }
                }

                if let Some(package_resolution) = resolver.resolve_package_directory(
                    &dep,
                    &package_json_name,
                    RESOLUTION_MODE_COMMON_JS,
                    None,
                ) && package_resolution.is_resolved()
                    && !package_resolution.original_path.is_empty()
                {
                    known_symlinks.process_resolution(
                        &combine_paths(&package_resolution.original_path, &["package.json"]),
                        &combine_paths(&package_resolution.resolved_file_name, &["package.json"]),
                    );
                }
            }
        }
        known_symlinks
    }

    // Go: program.go:668 (*Program).ModuleResolutionError (ts#64299)
    // PORT: a nil Go `error` is `None`.
    pub fn module_resolution_error(&self) -> Option<crate::gostd::GoError> {
        self.module_resolution_error.clone()
    }

    // Go: program.go:2380 (*Program).ForEachResolvedModule
    pub fn for_each_resolved_module(
        &self,
        callback: &mut ResolutionCallback<'_, Arc<ResolvedModule>>,
        file: Option<&ParsedSourceFile>,
    ) {
        for_each_resolution(&self.processed_files.resolved_modules, callback, file);
    }

    // Go: program.go:2384 (*Program).ForEachResolvedTypeReferenceDirective
    pub fn for_each_resolved_type_reference_directive(
        &self,
        callback: &mut ResolutionCallback<'_, Rc<ResolvedTypeReferenceDirective>>,
        file: Option<&ParsedSourceFile>,
    ) {
        for_each_resolution(
            &self.processed_files.type_resolutions_in_file,
            callback,
            file,
        );
    }
}

// Go: program.go:2388 forEachResolution
pub fn for_each_resolution<T>(
    resolution_cache: &FxHashMap<Path, ModeAwareCache<T>>,
    callback: &mut ResolutionCallback<'_, T>,
    file: Option<&ParsedSourceFile>,
) {
    if let Some(file) = file {
        if let Some(resolutions) = resolution_cache.get(file.path()) {
            for (key, resolution) in resolutions {
                callback(resolution, &key.name, key.mode, file.path());
            }
        }
    } else {
        for (file_path, resolutions) in resolution_cache {
            for (key, resolution) in resolutions {
                callback(resolution, &key.name, key.mode, file_path);
            }
        }
    }
}

// Go: program.go:2404 plainJSErrors
// PORT: Go package-level set; built once on first use.
pub fn plain_js_errors() -> &'static FxHashSet<i32> {
    static PLAIN_JS_ERRORS: OnceLock<FxHashSet<i32>> = OnceLock::new();
    PLAIN_JS_ERRORS.get_or_init(|| {
        [
            // binder errors
            diag::Cannot_redeclare_block_scoped_variable_0.code() as i32,
            diag::A_module_cannot_have_multiple_default_exports.code() as i32,
            diag::Another_export_default_is_here.code() as i32,
            diag::The_first_export_default_is_here.code() as i32,
            diag::Identifier_expected_0_is_a_reserved_word_at_the_top_level_of_a_module.code() as i32,
            diag::Identifier_expected_0_is_a_reserved_word_in_strict_mode_Modules_are_automatically_in_strict_mode.code() as i32,
            diag::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here.code() as i32,
            diag::X_constructor_is_a_reserved_word.code() as i32,
            diag::X_delete_cannot_be_called_on_an_identifier_in_strict_mode.code() as i32,
            diag::Code_contained_in_a_class_is_evaluated_in_JavaScript_s_strict_mode_which_does_not_allow_this_use_of_0_For_more_information_see_https_Colon_Slash_Slashdeveloper_mozilla_org_Slashen_US_Slashdocs_SlashWeb_SlashJavaScript_SlashReference_SlashStrict_mode.code() as i32,
            diag::Invalid_use_of_0_Modules_are_automatically_in_strict_mode.code() as i32,
            diag::Invalid_use_of_0_in_strict_mode.code() as i32,
            diag::A_label_is_not_allowed_here.code() as i32,
            diag::X_with_statements_are_not_allowed_in_strict_mode.code() as i32,
            // grammar errors
            diag::A_break_statement_can_only_be_used_within_an_enclosing_iteration_or_switch_statement.code() as i32,
            diag::A_break_statement_can_only_jump_to_a_label_of_an_enclosing_statement.code() as i32,
            diag::A_class_declaration_without_the_default_modifier_must_have_a_name.code() as i32,
            diag::A_class_member_cannot_have_the_0_keyword.code() as i32,
            diag::A_comma_expression_is_not_allowed_in_a_computed_property_name.code() as i32,
            diag::A_continue_statement_can_only_be_used_within_an_enclosing_iteration_statement.code() as i32,
            diag::A_continue_statement_can_only_jump_to_a_label_of_an_enclosing_iteration_statement.code() as i32,
            diag::A_default_clause_cannot_appear_more_than_once_in_a_switch_statement.code() as i32,
            diag::A_default_export_must_be_at_the_top_level_of_a_file_or_module_declaration.code() as i32,
            // ts#64640
            diag::A_deferred_import_must_specify_a_namespace_binding.code() as i32,
            diag::A_definite_assignment_assertion_is_not_permitted_in_this_context.code() as i32,
            diag::A_destructuring_declaration_must_have_an_initializer.code() as i32,
            diag::A_get_accessor_cannot_have_parameters.code() as i32,
            diag::A_rest_element_cannot_contain_a_binding_pattern.code() as i32,
            diag::A_rest_element_cannot_have_a_property_name.code() as i32,
            diag::A_rest_element_cannot_have_an_initializer.code() as i32,
            diag::A_rest_element_must_be_last_in_a_destructuring_pattern.code() as i32,
            diag::A_rest_parameter_cannot_have_an_initializer.code() as i32,
            diag::A_rest_parameter_must_be_last_in_a_parameter_list.code() as i32,
            diag::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma.code() as i32,
            diag::A_return_statement_cannot_be_used_inside_a_class_static_block.code() as i32,
            diag::A_set_accessor_cannot_have_rest_parameter.code() as i32,
            diag::A_set_accessor_must_have_exactly_one_parameter.code() as i32,
            // ts#63915
            diag::A_source_phase_import_must_specify_a_local_binding.code() as i32,
            diag::An_export_declaration_can_only_be_used_at_the_top_level_of_a_module.code() as i32,
            diag::An_export_declaration_cannot_have_modifiers.code() as i32,
            diag::An_import_declaration_can_only_be_used_at_the_top_level_of_a_module.code() as i32,
            diag::An_import_declaration_cannot_have_modifiers.code() as i32,
            diag::An_object_member_cannot_be_declared_optional.code() as i32,
            diag::Argument_of_dynamic_import_cannot_be_spread_element.code() as i32,
            diag::Cannot_assign_to_private_method_0_Private_methods_are_not_writable.code() as i32,
            diag::Cannot_redeclare_identifier_0_in_catch_clause.code() as i32,
            diag::Catch_clause_variable_cannot_have_an_initializer.code() as i32,
            diag::Class_decorators_can_t_be_used_with_static_private_identifier_Consider_removing_the_experimental_decorator.code() as i32,
            diag::Classes_can_only_extend_a_single_class.code() as i32,
            diag::Classes_may_not_have_a_field_named_constructor.code() as i32,
            diag::Did_you_mean_to_use_a_Colon_An_can_only_follow_a_property_name_when_the_containing_object_literal_is_part_of_a_destructuring_pattern.code() as i32,
            diag::Duplicate_label_0.code() as i32,
            diag::Dynamic_imports_can_only_accept_a_module_specifier_and_an_optional_set_of_attributes_as_arguments.code() as i32,
            diag::X_for_await_loops_cannot_be_used_inside_a_class_static_block.code() as i32,
            diag::JSX_attributes_must_only_be_assigned_a_non_empty_expression.code() as i32,
            diag::JSX_elements_cannot_have_multiple_attributes_with_the_same_name.code() as i32,
            diag::JSX_expressions_may_not_use_the_comma_operator_Did_you_mean_to_write_an_array.code() as i32,
            diag::JSX_property_access_expressions_cannot_include_JSX_namespace_names.code() as i32,
            diag::Jump_target_cannot_cross_function_boundary.code() as i32,
            diag::Line_terminator_not_permitted_before_arrow.code() as i32,
            diag::Modifiers_cannot_appear_here.code() as i32,
            // ts#63915
            diag::Named_and_namespace_imports_are_not_allowed_in_a_source_phase_import.code() as i32,
            diag::Only_a_single_variable_declaration_is_allowed_in_a_for_in_statement.code() as i32,
            diag::Only_a_single_variable_declaration_is_allowed_in_a_for_of_statement.code() as i32,
            // ts#63915
            diag::Optional_chaining_cannot_be_used_with_import_source.code() as i32,
            diag::Private_identifiers_are_not_allowed_outside_class_bodies.code() as i32,
            diag::Private_identifiers_are_only_allowed_in_class_bodies_and_may_only_be_used_as_part_of_a_class_member_declaration_property_access_or_on_the_left_hand_side_of_an_in_expression.code() as i32,
            diag::Property_0_is_not_accessible_outside_class_1_because_it_has_a_private_identifier.code() as i32,
            // ts#63915
            diag::Source_phase_imports_are_not_allowed_on_statements_that_compile_to_CommonJS_require_calls.code() as i32,
            // ts#63915
            diag::Source_phase_imports_are_only_supported_when_the_module_option_is_set_to_esnext_nodenext_or_preserve.code() as i32,
            diag::Tagged_template_expressions_are_not_permitted_in_an_optional_chain.code() as i32,
            diag::The_left_hand_side_of_a_for_of_statement_may_not_be_async.code() as i32,
            diag::The_variable_declaration_of_a_for_in_statement_cannot_have_an_initializer.code() as i32,
            diag::The_variable_declaration_of_a_for_of_statement_cannot_have_an_initializer.code() as i32,
            diag::Trailing_comma_not_allowed.code() as i32,
            diag::Variable_declaration_list_cannot_be_empty.code() as i32,
            diag::X_0_and_1_operations_cannot_be_mixed_without_parentheses.code() as i32,
            diag::X_0_expected.code() as i32,
            // ts#63915
            diag::X_0_is_not_a_valid_meta_property_for_keyword_import_Did_you_mean_meta_defer_or_source.code() as i32,
            diag::X_0_is_not_a_valid_meta_property_for_keyword_1_Did_you_mean_2.code() as i32,
            diag::X_0_list_cannot_be_empty.code() as i32,
            diag::X_0_modifier_already_seen.code() as i32,
            diag::X_0_modifier_cannot_appear_on_a_constructor_declaration.code() as i32,
            diag::X_0_modifier_cannot_appear_on_a_module_or_namespace_element.code() as i32,
            diag::X_0_modifier_cannot_appear_on_a_parameter.code() as i32,
            diag::X_0_modifier_cannot_appear_on_class_elements_of_this_kind.code() as i32,
            diag::X_0_modifier_cannot_be_used_here.code() as i32,
            diag::X_0_modifier_must_precede_1_modifier.code() as i32,
            diag::X_0_declarations_can_only_be_declared_inside_a_block.code() as i32,
            diag::X_0_declarations_must_be_initialized.code() as i32,
            diag::X_extends_clause_already_seen.code() as i32,
            diag::X_let_is_not_allowed_to_be_used_as_a_name_in_let_or_const_declarations.code() as i32,
            diag::Class_constructor_may_not_be_a_generator.code() as i32,
            diag::Class_constructor_may_not_be_an_accessor.code() as i32,
            diag::X_await_expressions_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules.code() as i32,
            diag::X_await_using_statements_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules.code() as i32,
            diag::Private_field_0_must_be_declared_in_an_enclosing_class.code() as i32,
            // Type errors
            diag::This_condition_will_always_return_0_since_JavaScript_compares_objects_by_reference_not_value.code() as i32,
        ]
        .into_iter()
        .collect()
    })
}

// Go: symlinks/knownsymlinks.go:91 (*KnownSymlinks).SetSymlinksFromResolutions
// PORT: the method lives here because it needs frontend resolution types.
impl KnownSymlinks {
    pub fn set_symlinks_from_resolutions(
        &mut self,
        for_each_resolved_module: &dyn Fn(
            &mut ResolutionCallback<'_, Arc<ResolvedModule>>,
            Option<&ParsedSourceFile>,
        ),
        for_each_resolved_type_reference_directive: &dyn Fn(
            &mut ResolutionCallback<'_, Rc<ResolvedTypeReferenceDirective>>,
            Option<&ParsedSourceFile>,
        ),
    ) {
        for_each_resolved_module(
            &mut |resolution: &Arc<ResolvedModule>,
                  _module_name: &str,
                  _mode: ResolutionMode,
                  _file_path: &Path| {
                self.process_resolution(&resolution.original_path, &resolution.resolved_file_name);
            },
            None,
        );
        for_each_resolved_type_reference_directive(
            &mut |resolution: &Rc<ResolvedTypeReferenceDirective>,
                  _module_name: &str,
                  _mode: ResolutionMode,
                  _file_path: &Path| {
                self.process_resolution(&resolution.original_path, &resolution.resolved_file_name);
            },
            None,
        );
    }
}

impl NewProgram {
    // Go: program.go:644 (*Program).GetResolvedModule
    pub fn get_resolved_module(
        &self,
        file: &dyn HasFileName,
        module_reference: &str,
        mode: ResolutionMode,
    ) -> Option<Arc<ResolvedModule>> {
        if let Some(resolutions) = self.processed_files.resolved_modules.get(&file.path()) {
            if let Some(resolved) = resolutions.get(&ModeAwareCacheKey {
                name: module_reference.to_string(),
                mode,
            }) {
                return Some(resolved.clone());
            }
        }
        None
    }

    // Go: program.go:653 (*Program).GetResolvedModuleFromModuleSpecifier
    pub fn get_resolved_module_from_module_specifier(
        &self,
        file: &dyn HasFileName,
        module_specifier: Node,
    ) -> Option<Arc<ResolvedModule>> {
        if !is_string_literal_like(module_specifier) {
            panic!("moduleSpecifier must be a StringLiteralLike");
        }
        // ts#63915
        if is_source_phase_import(module_specifier.parent()) {
            return None;
        }
        let mode = self.get_mode_for_usage_location(file, module_specifier);
        self.get_resolved_module(file, module_specifier.text(), mode)
    }

    // Go: program.go:1763 (*Program).GetSourceFileMetaData
    // PORT: a missing Go map entry is the zero value.
    pub fn get_source_file_meta_data(&self, path: &Path) -> SourceFileMetaData {
        self.processed_files
            .source_file_meta_datas
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    // Go: program.go:1767 (*Program).GetEmitModuleFormatOfFile
    pub fn get_emit_module_format_of_file(&self, source_file: &dyn HasFileName) -> ModuleKind {
        get_emit_module_format_of_file_worker(
            &source_file.file_name(),
            &self.mapper().get_compiler_options_for_file(source_file),
            &self.get_source_file_meta_data(&source_file.path()),
        )
    }

    // Go: program.go:1771 (*Program).GetEmitSyntaxForUsageLocation
    pub fn get_emit_syntax_for_usage_location(
        &self,
        source_file: &dyn HasFileName,
        location: Node,
    ) -> ResolutionMode {
        super::file_loader::get_emit_syntax_for_usage_location_worker(
            &source_file.file_name(),
            &self.get_source_file_meta_data(&source_file.path()),
            location,
            &self.mapper().get_compiler_options_for_file(source_file),
        )
    }

    // Go: program.go:1775 (*Program).GetImpliedNodeFormatForEmit
    pub fn get_implied_node_format_for_emit(
        &self,
        source_file: &dyn HasFileName,
    ) -> ResolutionMode {
        get_implied_node_format_for_emit_worker(
            &source_file.file_name(),
            self.mapper()
                .get_compiler_options_for_file(source_file)
                .get_emit_module_kind(),
            &self.get_source_file_meta_data(&source_file.path()),
        )
    }

    // Go: program.go:1779 (*Program).GetModeForUsageLocation
    pub fn get_mode_for_usage_location(
        &self,
        source_file: &dyn HasFileName,
        location: Node,
    ) -> ResolutionMode {
        super::file_loader::get_mode_for_usage_location(
            &source_file.file_name(),
            &self.get_source_file_meta_data(&source_file.path()),
            location,
            Some(&self.mapper().get_compiler_options_for_file(source_file)),
        )
    }

    // Go: program.go:1783 (*Program).GetModeForResolutionAtIndex (ts#64292)
    // PORT: Go `*ast.SourceFile` is the file root `Node` (the api passes a
    // file of this program); its `ast.HasFileName` view copies the file name
    // and path. Go `index int` is `usize`.
    pub fn get_mode_for_resolution_at_index(
        &self,
        source_file: Node,
        index: usize,
    ) -> ResolutionMode {
        let info = source_file_info(source_file);
        let file = new_has_file_name(source_file_file_name(source_file), &info.path);
        let imports = &info.imports;
        if index < imports.len() {
            return self.get_mode_for_usage_location(&file, imports[index]);
        }
        let mut index = index - imports.len();
        for &augmentation in &info.module_augmentations {
            if augmentation.kind() == SyntaxKind::StringLiteral {
                if index == 0 {
                    return self.get_mode_for_usage_location(&file, augmentation);
                }
                index -= 1;
            }
        }
        panic!("resolution index out of range")
    }

    // Go: program.go:1800 (*Program).GetDefaultResolutionModeForFile
    pub fn get_default_resolution_mode_for_file(
        &self,
        source_file: &dyn HasFileName,
    ) -> ResolutionMode {
        super::file_loader::get_default_resolution_mode_for_file(
            &source_file.file_name(),
            &self.get_source_file_meta_data(&source_file.path()),
            &self.mapper().get_compiler_options_for_file(source_file),
        )
    }

    // Go: program.go:1804 (*Program).IsSourceFileDefaultLibrary
    pub fn is_source_file_default_library(&self, path: &Path) -> bool {
        self.processed_files.lib_files.contains_key(path)
    }

    // Go: program.go:1823 (*Program).CommonSourceDirectory
    // PORT: Go `checkSourceFilesBelongToPath` adds processing diagnostics
    // through the include processor, which uses interior mutability here.
    pub fn common_source_directory(&self) -> String {
        self.common_source_directory
            .get_or_init(|| {
                let files = || -> Vec<String> {
                    self.processed_files
                        .files
                        .iter()
                        .filter(|file| {
                            // #4699: Go `sourceFileMayBeEmitted(file, p, false
                            // /*forceDtsEmit*/, false /*forceJsEmit*/)`.
                            source_file_may_be_emitted(file, self, false, false)
                                && !file.is_declaration_file
                        })
                        .map(|file| file.file_name().to_string())
                        .collect()
                };
                let mut check = |source_files: &[String], root_directory: &str| -> bool {
                    self.check_source_files_belong_to_path(source_files, root_directory)
                };
                get_common_source_directory(
                    self.options(),
                    files,
                    &self.get_current_directory(),
                    self.use_case_sensitive_file_names(),
                    Some(&mut check),
                )
            })
            .clone()
    }

    // Go: program.go:899 (*Program).getSourceFilesToEmit
    // PORT: Go nil `targetSourceFiles` is `None`; an empty slice is
    // `Some(&[])` (#4699: a slice of targets and `forceJsEmit`).
    pub fn get_source_files_to_emit(
        &self,
        target_source_files: Option<&[Rc<ParsedSourceFile>]>,
        force_dts_emit: bool,
        force_js_emit: bool,
    ) -> Vec<Rc<ParsedSourceFile>> {
        if target_source_files.is_none() && !force_dts_emit && !force_js_emit {
            return self
                .source_files_to_emit
                .get_or_init(|| get_source_files_to_emit(self, None, false, false))
                .clone();
        }
        get_source_files_to_emit(self, target_source_files, force_dts_emit, force_js_emit)
    }
}

// Go: emitter.go:476 sourceFileMayBeEmitted
// PORT: the Go host is `SourceFileMayBeEmittedHost`; the program is the only
// host the frontend uses.
pub fn source_file_may_be_emitted(
    source_file: &ParsedSourceFile,
    host: &NewProgram,
    force_dts_emit: bool,
    force_js_emit: bool,
) -> bool {
    // TODO: move this to outputpaths?

    let options = host.options();
    // Js files are emitted only if option is enabled
    // #4699: a forced JS emit keeps JS files.
    if !force_js_emit && options.no_emit_for_js_files.is_true() && source_file.is_js() {
        return false;
    }

    // Declaration files are not emitted
    if source_file.is_declaration_file {
        return false;
    }

    // Runtime output for content-mapped files is owned by the external content mapper or build tool. Only
    // include them in the emit set when their transformed TypeScript can produce declarations.
    // tsgo#4712
    if !source_file.content_mapper().is_empty()
        && !force_dts_emit
        && !options.get_emit_declarations()
    {
        return false;
    }

    // Source file from node_modules are not emitted
    if host.is_source_file_from_external_library(source_file) {
        return false;
    }

    // forcing dts emit => file needs to be emitted
    // #4699: a forced JS emit too.
    if force_dts_emit || force_js_emit {
        return true;
    }

    // Check other conditions for file emit
    // Source files from referenced projects are not emitted
    if host
        .get_project_reference_from_source(source_file.path())
        .is_some()
    {
        return false;
    }

    // Any non json file should be emitted
    if source_file.script_kind != ScriptKind::JSON {
        return true;
    }

    // Json file is not emitted if outDir is not specified
    if options.out_dir.is_empty() {
        return false;
    }

    // Otherwise, if rootDir is specified or a config file exists, we know the common source directory and can check if the file would be emitted in the same location
    if !options.root_dir.is_empty() || !options.config_file_path.is_empty() {
        let current_directory = host.get_current_directory();
        let common_dir = get_normalized_absolute_path(
            &get_common_source_directory(
                options,
                Vec::new,
                &current_directory,
                host.use_case_sensitive_file_names(),
                None,
            ),
            &current_directory,
        );
        let output_path = get_source_file_path_in_new_dir_worker(
            source_file.file_name(),
            &options.out_dir,
            &current_directory,
            &common_dir,
            host.use_case_sensitive_file_names(),
        );
        if compare_paths(
            source_file.file_name(),
            &output_path,
            &ComparePathsOptions {
                use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
                current_directory: current_directory,
            },
        ) == 0
        {
            return false;
        }
    }

    true
}

// Go: emitter.go:538 getSourceFilesToEmit
// PORT: Go nil `targetSourceFiles` is `None` (#4699: a slice of targets and
// `forceJsEmit`).
pub fn get_source_files_to_emit(
    host: &NewProgram,
    target_source_files: Option<&[Rc<ParsedSourceFile>]>,
    force_dts_emit: bool,
    force_js_emit: bool,
) -> Vec<Rc<ParsedSourceFile>> {
    let target_source_files: &[Rc<ParsedSourceFile>] = match target_source_files {
        Some(target_source_files) => target_source_files,
        None => &host.processed_files.files,
    };
    target_source_files
        .iter()
        .filter(|source_file| {
            source_file_may_be_emitted(source_file, host, force_dts_emit, force_js_emit)
        })
        .cloned()
        .collect()
}

// Go: `*Program` implements `outputpaths.OutputPathsHost`.
impl OutputPathsHost for NewProgram {
    fn common_source_directory(&self) -> String {
        NewProgram::common_source_directory(self)
    }
    fn get_current_directory(&self) -> String {
        NewProgram::get_current_directory(self)
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        NewProgram::use_case_sensitive_file_names(self)
    }
    // tsgo#4712
    fn content_mapper_extensions(&self) -> Vec<String> {
        NewProgram::content_mapper_extensions(self)
    }
}

impl NewProgram {
    // Go: program.go:1841 (*Program).checkSourceFilesBelongToPath
    // PERF: Go makes the canonical absolute path of every file before the
    // `ContainsPath` test, but only the diagnostic of a file outside
    // `rootDirectory` reads it. It is a pure function of the file name, so
    // it is made in that branch only (effect: 11 M cycles of serial loader
    // work). Same result and diagnostics.
    pub fn check_source_files_belong_to_path(
        &self,
        source_files: &[String],
        root_directory: &str,
    ) -> bool {
        let mut all_files_belong_to_path = true;
        for file in source_files {
            if !contains_path(root_directory, file, &self.compare_paths_options) {
                let absolute_source_file_path = get_canonical_file_name(
                    &get_normalized_absolute_path(file, &self.get_current_directory()),
                    self.use_case_sensitive_file_names(),
                );
                self.include_processor.late_processing_diagnostics.borrow_mut().push(Rc::new(ProcessingDiagnostic {
                    kind: ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE,
                    data: ProcessingDiagnosticData::IncludeExplaining(IncludeExplainingDiagnostic {
                        file: Path(absolute_source_file_path),
                        diagnostic_reason: None,
                        message: diag::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files,
                        args: vec![file.clone(), root_directory.to_string()],
                    }),
                }));
                all_files_belong_to_path = false;
            }
        }

        all_files_belong_to_path
    }
}
