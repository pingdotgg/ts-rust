//! Go `internal/module/resolver.go` lines 2127 to 2366 (pin N): `Ending`,
//! `ResolvedEntrypoint`, `GetEntrypointsFromPackageJsonInfo`,
//! `createResolvedEntrypointHandlingSymlink`, `loadEntrypointsFromExportMap`
//! and `getMatchedStarForPatternEntrypoint`. Only `ls/autoimport` and
//! `modulespecifiers.ProcessEntrypointEnding` (language service) use them.
//!
//! PORT: shapes follow `resolver_p1.rs` and `resolver_p2.rs`: Go
//! `*packagejson.InfoCacheEntry` is `Option<Rc<InfoCacheEntry>>`, Go
//! `*resolved` is `Option<Resolved>`. Go `*collections.Set[string]` is
//! `Option<FxHashSet<String>>` (`None` is nil). Go `*ResolvedEntrypoint`
//! values are shared (`Rc<ResolvedEntrypoint>`).

use crate::frontend::prelude::*;

use crate::flags_macros::go_enum;

/// Go `(*resolved).isResolved` on a possibly nil `*resolved`.
fn is_resolved(resolved: &Option<Resolved>) -> bool {
    resolved.as_ref().is_some_and(|r| !r.path.is_empty())
}

// Go: module/resolver.go:2127 Ending
go_enum!(Ending, i32 {
    // EndingFixed indicates that the module specifier cannot be changed without changing its resolution.
    FIXED = 0; // EndingFixed
    // EndingExtensionChangeable indicates that the module specifier's extension portion was inferred from a
    // file on disk, so an interchangeable one could be used instead (e.g. replacing .d.ts with .js).
    EXTENSION_CHANGEABLE = 1; // EndingExtensionChangeable
    // EndingChangeable indicates that the module specifier's file name and extension portion were inferred
    // from a file on disk without being matched as part of an 'exports' pattern, so can be changed according
    // to the importer's module resolution rules (e.g. an /index.d.ts may be dropped entirely in CommonJS settings).
    CHANGEABLE = 2; // EndingChangeable
});

// Go: module/resolver.go:2389 ResolvedEntrypoint
#[derive(Clone, Debug, Default)]
pub struct ResolvedEntrypoint {
    // OriginalFileName is the symlink path if the entrypoint was discovered at a symlink. Empty otherwise.
    pub original_file_name: String,
    // ResolvedFileName is the real path to the entrypoint file.
    pub resolved_file_name: String,
    pub module_specifier: String,
    // Ending indicates whether the file name and extension portion of ModuleSpecifier is fixed or can be changed.
    pub ending: Ending,
    // IncludeConditions are the conditions that a resolver must have to reach this entrypoint.
    pub include_conditions: Option<FxHashSet<String>>,
    // ExcludeConditions are the conditions that a resolver must not have to reach this entrypoint.
    pub exclude_conditions: Option<FxHashSet<String>>,
}

impl ResolvedEntrypoint {
    // Go: module/resolver.go:2404 SymlinkOrRealpath
    #[must_use]
    pub fn symlink_or_realpath(&self) -> String {
        if !self.original_file_name.is_empty() {
            return self.original_file_name.clone();
        }
        self.resolved_file_name.clone()
    }
}

impl DefaultResolver {
    // Go: module/resolver.go:2411 GetEntrypointsFromPackageJsonInfo
    // PORT: Go returns a nil slice for no entrypoints; that is an empty `Vec`.
    // Go `&resolutionState{resolver: r, extensions: ..., features: ...,
    // compilerOptions: r.compilerOptions}` spells out the zero fields here
    // (`ResolutionState::zero` is private to `resolver_p1.rs`).
    pub fn get_entrypoints_from_package_json_info(
        &self,
        package_json: &Option<Rc<InfoCacheEntry>>,
        package_name: &str,
        enable_directory_search: bool,
    ) -> Vec<Rc<ResolvedEntrypoint>> {
        let extensions = Extensions::TYPE_SCRIPT | Extensions::DECLARATION;
        let features = NodeResolutionFeatures::ALL;
        let mut state = ResolutionState {
            resolver: self,
            tracer: None,
            name: String::new(),
            containing_directory: String::new(),
            is_config_lookup: false,
            features,
            esm_mode: false,
            conditions: Vec::new(),
            extensions,
            compiler_options: self.compiler_options.clone(),
            resolve_package_directory_only: false,
            candidate_ending_is_from_config: false,
            resolved_package_directory: false,
            diagnostics: Vec::new(),
        };
        // ts#64544: a package in a dynamic directory names its entrypoints
        // with module specifier escapes.
        let dynamic_package = package_json
            .as_ref()
            .is_some_and(|p| is_encoded_dynamic_file_name(&p.package_directory));
        let source_package_name = if dynamic_package {
            dynamic_uri_path_to_module_specifier(package_name)
        } else {
            package_name.to_string()
        };
        if let Some(info) = package_json.as_ref().filter(|p| p.exists()) {
            let exports = &info.contents.as_ref().unwrap().fields.path_fields.exports;
            if exports.is_present() {
                let entrypoints = state.load_entrypoints_from_export_map(
                    package_json,
                    &source_package_name,
                    exports,
                );
                return entrypoints;
            }
        }

        let package_json_entry = package_json
            .as_ref()
            .expect("invalid memory address or nil pointer dereference");
        let mut result: Vec<Rc<ResolvedEntrypoint>> = Vec::new();
        let main_resolution = state.load_node_module_from_directory_worker(
            extensions,
            &package_json_entry.package_directory,
            package_json,
        );

        if is_resolved(&main_resolution) {
            result.push(self.create_resolved_entrypoint_handling_symlink(
                &main_resolution.as_ref().unwrap().path,
                &source_package_name,
                None,
                None,
                Ending::FIXED,
            ));
        }

        if enable_directory_search {
            let other_files = crate::frontend::vfs::vfsmatch::read_directory(
                self.host.fs(),
                self.host.get_current_directory(),
                &package_json_entry.package_directory,
                &extensions.array(),
                &["node_modules".to_string()],
                &["**/*".to_string()],
                crate::frontend::vfs::vfsmatch::UNLIMITED_DEPTH,
            );

            let use_case_sensitive_file_names = self.host.fs().use_case_sensitive_file_names();
            for file in &other_files {
                // ts#64159 (resolver.go:2454): CaseSensitivity.CompareFilePaths,
                // so dynamic names that differ only in case are two files.
                if is_resolved(&main_resolution)
                    && compare_rooted_text(
                        file,
                        &main_resolution.as_ref().unwrap().path,
                        use_case_sensitive_file_names,
                    ) == 0
                {
                    continue;
                }

                // ts#64159 (resolver.go:2458): RelativeFilePathFromDirectory,
                // joined to the package name with "/".
                let relative = relative_path_within_directory(
                    &package_json_entry.package_directory,
                    file,
                    use_case_sensitive_file_names,
                )
                .unwrap_or_default();
                let relative_specifier = if dynamic_package {
                    dynamic_uri_path_to_module_specifier(&relative)
                } else {
                    relative.into_owned()
                };
                result.push(self.create_resolved_entrypoint_handling_symlink(
                    file,
                    &format!("{source_package_name}/{relative_specifier}"),
                    None,
                    None,
                    Ending::CHANGEABLE,
                ));
            }
        }

        if !result.is_empty() {
            return result;
        }
        Vec::new()
    }

    // Go: module/resolver.go:2479 createResolvedEntrypointHandlingSymlink
    pub fn create_resolved_entrypoint_handling_symlink(
        &self,
        file_name: &str,
        module_specifier: &str,
        include_conditions: Option<FxHashSet<String>>,
        exclude_conditions: Option<FxHashSet<String>>,
        ending: Ending,
    ) -> Rc<ResolvedEntrypoint> {
        let mut original_file_name = String::new();
        let mut resolved_file_name = file_name.to_string();
        let real_path = self.host.fs().realpath(file_name);
        if real_path != file_name {
            original_file_name = file_name.to_string();
            resolved_file_name = real_path;
        }
        Rc::new(ResolvedEntrypoint {
            original_file_name,
            resolved_file_name,
            module_specifier: module_specifier.to_string(),
            include_conditions,
            exclude_conditions,
            ending,
        })
    }
}

impl ResolutionState<'_> {
    // Go: module/resolver.go:2497 loadEntrypointsFromExportMap
    pub fn load_entrypoints_from_export_map(
        &mut self,
        package_json: &Option<Rc<InfoCacheEntry>>,
        package_name: &str,
        exports: &ExportsOrImports,
    ) -> Vec<Rc<ResolvedEntrypoint>> {
        let package_json = package_json
            .as_ref()
            .expect("invalid memory address or nil pointer dereference");
        let mut entrypoints: Vec<Rc<ResolvedEntrypoint>> = Vec::new();

        match exports.type_ {
            JSONValueType::ARRAY => {
                for element in exports.as_array() {
                    self.load_entrypoints_from_target_exports(
                        package_json,
                        package_name,
                        &mut entrypoints,
                        ".",
                        None,
                        None,
                        element,
                    );
                }
            }
            JSONValueType::OBJECT => {
                if exports.is_subpaths() {
                    for (subpath, export) in exports.as_object() {
                        self.load_entrypoints_from_target_exports(
                            package_json,
                            package_name,
                            &mut entrypoints,
                            subpath,
                            None,
                            None,
                            export,
                        );
                    }
                } else {
                    self.load_entrypoints_from_target_exports(
                        package_json,
                        package_name,
                        &mut entrypoints,
                        ".",
                        None,
                        None,
                        exports,
                    );
                }
            }
            _ => {
                self.load_entrypoints_from_target_exports(
                    package_json,
                    package_name,
                    &mut entrypoints,
                    ".",
                    None,
                    None,
                    exports,
                );
            }
        }

        entrypoints
    }

    // Go: module/resolver.go:2246 loadEntrypointsFromTargetExports (closure in loadEntrypointsFromExportMap)
    // PORT: the Go closure is a method; its captures (`packageJson`,
    // `packageName`, `entrypoints`) are parameters. Go passes
    // `*collections.Set` pointers and clones a set before every change, so
    // passing set values gives the same sets.
    fn load_entrypoints_from_target_exports(
        &mut self,
        package_json: &InfoCacheEntry,
        package_name: &str,
        entrypoints: &mut Vec<Rc<ResolvedEntrypoint>>,
        subpath: &str,
        include_conditions: Option<FxHashSet<String>>,
        mut exclude_conditions: Option<FxHashSet<String>>,
        exports: &ExportsOrImports,
    ) {
        if exports.type_ == JSONValueType::STRING && exports.as_string().starts_with("./") {
            let exports_string = exports.as_string();
            if exports_string.contains('*') {
                if exports_string.find('*') != exports_string.rfind('*') {
                    return;
                }
                // ts#64544: the pattern matches the path of each file relative
                // to the package directory (decoded in a dynamic package).
                let dynamic_package = is_encoded_dynamic_file_name(&package_json.package_directory);
                let include_patterns = if dynamic_package {
                    vec!["**/*".to_string()]
                } else {
                    vec![crate::frontend::tspath::change_full_extension(
                        &exports_string.replacen('*', "**/*", 1),
                        ".*",
                    )]
                };
                let pattern_path = exports_string.strip_prefix("./").unwrap_or(exports_string);
                // Go: strings.Cut(patternPath, "*")
                let (leading_slice, trailing_slice) = match pattern_path.split_once('*') {
                    Some((before, after)) => (before.to_string(), after.to_string()),
                    None => (pattern_path.to_string(), String::new()),
                };
                let case_sensitive =
                    dynamic_package || self.resolver.host.fs().use_case_sensitive_file_names();
                let files = crate::frontend::vfs::vfsmatch::read_directory(
                    self.resolver.host.fs(),
                    self.resolver.host.get_current_directory(),
                    &package_json.package_directory,
                    &self.extensions.array(),
                    &[],
                    &include_patterns,
                    crate::frontend::vfs::vfsmatch::UNLIMITED_DEPTH,
                );
                for file in &files {
                    let mut logical_file = get_relative_path_from_directory(
                        &package_json.package_directory,
                        file,
                        &ComparePathsOptions {
                            use_case_sensitive_file_names: case_sensitive,
                            ..Default::default()
                        },
                    );
                    if dynamic_package {
                        logical_file = decode_dynamic_uri_path(&logical_file);
                    }
                    let (mut matched_star, ok) = self.get_matched_star_for_pattern_entrypoint(
                        &logical_file,
                        &leading_slice,
                        &trailing_slice,
                        case_sensitive,
                    );
                    if !ok {
                        continue;
                    }
                    if dynamic_package {
                        matched_star = encode_dynamic_logical_module_specifier(&matched_star);
                    }
                    let replaced = subpath.replacen('*', &matched_star, 1);
                    let resolved_subpath = replaced.strip_prefix("./").unwrap_or(&replaced);
                    if resolved_subpath.is_empty() {
                        continue;
                    }
                    let module_specifier =
                        crate::frontend::tspath::resolve_path(package_name, &[resolved_subpath]);
                    entrypoints.push(self.resolver.create_resolved_entrypoint_handling_symlink(
                        file,
                        &module_specifier,
                        include_conditions.clone(),
                        exclude_conditions.clone(),
                        if exports_string.ends_with('*') {
                            Ending::EXTENSION_CHANGEABLE
                        } else {
                            Ending::FIXED
                        },
                    ));
                }
            } else {
                let path_components =
                    crate::frontend::tspath::get_path_components(exports_string, "");
                let parts_after_first = &path_components[2..];
                if parts_after_first.iter().any(|p| p == "..")
                    || parts_after_first.iter().any(|p| p == ".")
                    || parts_after_first.iter().any(|p| p == "node_modules")
                {
                    return;
                }
                let resolved_target = crate::frontend::tspath::resolve_path(
                    &package_json.package_directory,
                    &[exports_string],
                );
                let result = self.load_file_name_from_package_json_field(
                    self.extensions,
                    &resolved_target,
                    exports_string,
                );
                if is_resolved(&result) {
                    entrypoints.push(self.resolver.create_resolved_entrypoint_handling_symlink(
                        &result.as_ref().unwrap().path,
                        &crate::frontend::tspath::resolve_path(package_name, &[subpath]),
                        include_conditions.clone(),
                        exclude_conditions.clone(),
                        if exports_string.ends_with('*') {
                            Ending::EXTENSION_CHANGEABLE
                        } else {
                            Ending::FIXED
                        },
                    ));
                }
            }
        } else if exports.type_ == JSONValueType::ARRAY {
            for element in exports.as_array() {
                self.load_entrypoints_from_target_exports(
                    package_json,
                    package_name,
                    entrypoints,
                    subpath,
                    include_conditions.clone(),
                    exclude_conditions.clone(),
                    element,
                );
            }
        } else if exports.type_ == JSONValueType::OBJECT {
            let mut prev_conditions: Vec<String> = Vec::new();
            for (condition, export) in exports.as_object() {
                if exclude_conditions
                    .as_ref()
                    .is_some_and(|e| e.contains(condition))
                {
                    continue;
                }

                let condition_always_matches = condition == "default"
                    || condition == "types"
                    || is_applicable_versioned_types_key(condition);
                let mut new_include_conditions = include_conditions.clone();
                if !condition_always_matches {
                    // Go: newIncludeConditions = includeConditions.Clone()
                    new_include_conditions = include_conditions.clone();
                    // Go: excludeConditions = excludeConditions.Clone() (the value is already a copy)
                    if new_include_conditions.is_none() {
                        new_include_conditions = Some(FxHashSet::default());
                    }
                    new_include_conditions
                        .as_mut()
                        .unwrap()
                        .insert(condition.clone());
                    for prev_condition in &prev_conditions {
                        if exclude_conditions.is_none() {
                            exclude_conditions = Some(FxHashSet::default());
                        }
                        exclude_conditions
                            .as_mut()
                            .unwrap()
                            .insert(prev_condition.clone());
                    }
                }

                prev_conditions.push(condition.clone());
                self.load_entrypoints_from_target_exports(
                    package_json,
                    package_name,
                    entrypoints,
                    subpath,
                    new_include_conditions,
                    exclude_conditions.clone(),
                    export,
                );
                if condition_always_matches {
                    break;
                }
            }
        }
    }

    // Go: module/resolver.go:2632 getMatchedStarForPatternEntrypoint
    // PORT: Go slices bytes; a case-insensitive match can end inside a
    // character, so the bytes are copied (`from_utf8_lossy`) instead of
    // slicing the `&str`.
    pub fn get_matched_star_for_pattern_entrypoint(
        &self,
        file: &str,
        leading_slice: &str,
        trailing_slice: &str,
        case_sensitive: bool,
    ) -> (String, bool) {
        if crate::frontend::stringutil_ls::has_prefix_and_suffix_without_overlap(
            file,
            leading_slice,
            trailing_slice,
            case_sensitive,
        ) {
            return (
                String::from_utf8_lossy(
                    &file.as_bytes()[leading_slice.len()..file.len() - trailing_slice.len()],
                )
                .into_owned(),
                true,
            );
        }

        let js_extension = try_get_js_extension_for_file(file, &self.compiler_options);
        if !js_extension.is_empty() {
            let swapped = crate::frontend::tspath::change_full_extension(file, js_extension);
            if crate::frontend::stringutil_ls::has_prefix_and_suffix_without_overlap(
                &swapped,
                leading_slice,
                trailing_slice,
                case_sensitive,
            ) {
                return (
                    String::from_utf8_lossy(
                        &swapped.as_bytes()
                            [leading_slice.len()..swapped.len() - trailing_slice.len()],
                    )
                    .into_owned(),
                    true,
                );
            }
        }

        (String::new(), false)
    }
}
