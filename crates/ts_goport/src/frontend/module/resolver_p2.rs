//! Go `internal/module/resolver.go` lines 1153 to 2361 (unit U19).
//!
//! `resolutionState` methods from `createResolvedModuleHandlingSymlink` to
//! `getTraceFunc`, plus `GetConditions`, `getNodeResolutionFeatures`,
//! `ParsedPatterns`, `TryParsePatterns`, `MatchPatternOrExact`,
//! `normalizePathForCJSResolution`, `extensionIsOk`, `ResolveConfig` and
//! `GetAutomaticTypeDirectiveNames`.
//!
//! PORT: shapes shared with U17 (packagejson) and U18 (resolver_p1, types,
//! cache, util):
//! - Go `*resolved` is `Option<Resolved>`. `nil` (`continueSearching()`) is
//!   `None`. `shouldContinueSearching()` is `is_none()`. `isResolved()` is
//!   `is_some_and(|r| !r.path.is_empty())`.
//! - `resolutionState` is `ResolutionState<'_>` with `resolver: &Resolver`.
//!   Its methods take `&mut self`.
//! - Go `*tracer` is `Option<Rc<RefCell<Tracer>>>`. The resolver shares it
//!   with the state. `trace_write!` keeps the Go `if r.tracer != nil` form.
//! - Go `*packagejson.InfoCacheEntry` is `Option<Rc<InfoCacheEntry>>`.
//!   Go embedding is nesting (`contents.fields.path_fields.exports`).
//! - Go `*ResolvedModule` and `*ResolvedTypeReferenceDirective` results are
//!   owned values. The functions here never return nil for them.
//!
//! PORT: `Ending`, `ResolvedEntrypoint`, `GetEntrypointsFromPackageJsonInfo`,
//! `createResolvedEntrypointHandlingSymlink`, `loadEntrypointsFromExportMap`
//! and `getMatchedStarForPatternEntrypoint` (Go lines 2127 to 2366) are in
//! `entrypoints.rs`. Only `ls/autoimport` and
//! `modulespecifiers.ProcessEntrypointEnding` (language service) use them.

use crate::frontend::prelude::*;
use std::borrow::Cow;

/// Go `if r.tracer != nil { r.tracer.write(diag, args...) }`.
macro_rules! trace_write {
    ($state:expr, $msg:expr $(, $arg:expr)* $(,)?) => {
        if let Some(tracer) = &$state.tracer {
            tracer.borrow_mut().write($msg, args![$($arg),*]);
        }
    };
}

/// Go `(*resolved).isResolved` on a possibly nil `*resolved`.
fn is_resolved(resolved: &Option<Resolved>) -> bool {
    resolved.as_ref().is_some_and(|r| !r.path.is_empty())
}

/// Go `(*packagejson.InfoCacheEntry).Exists` on a possibly nil entry.
fn info_exists(info: &Option<Rc<InfoCacheEntry>>) -> bool {
    info.as_ref().is_some_and(|p| p.exists())
}

impl ResolutionState<'_> {
    // Go: module/resolver.go:1355 createResolvedModuleHandlingSymlink
    pub fn create_resolved_module_handling_symlink(
        &mut self,
        mut resolved: Option<Resolved>,
    ) -> ResolvedModule {
        let is_external_library_import = resolved
            .as_ref()
            .is_some_and(|r| r.path.contains("/node_modules/"));
        if self.compiler_options.preserve_symlinks != Tristate::True
            && is_external_library_import
            && resolved
                .as_ref()
                .is_some_and(|r| r.original_path.is_empty())
            && !is_external_module_name_relative(&self.name)
        {
            let path = resolved
                .as_ref()
                .map(|r| r.path.clone())
                .unwrap_or_default();
            let (original_path, resolved_file_name) =
                self.get_original_and_resolved_file_name(&path);
            if !original_path.is_empty() {
                if let Some(r) = resolved.as_mut() {
                    r.path = resolved_file_name;
                    r.original_path = original_path;
                }
            }
        }
        self.create_resolved_module(resolved, is_external_library_import)
    }

    // Go: module/resolver.go:1370 createResolvedModule
    pub fn create_resolved_module(
        &self,
        resolved: Option<Resolved>,
        is_external_library_import: bool,
    ) -> ResolvedModule {
        let mut resolved_module = ResolvedModule::default();
        resolved_module.resolution_diagnostics = self.diagnostics.clone();

        if let Some(resolved) = resolved {
            resolved_module.resolved_file_name = resolved.path;
            resolved_module.original_path = resolved.original_path;
            resolved_module.is_external_library_import = is_external_library_import;
            resolved_module.resolved_using_ts_extension = resolved.resolved_using_ts_extension;
            resolved_module.resolved_using_extra_extensions =
                resolved.resolved_using_extra_extensions;
            resolved_module.extension = resolved.extension;
            resolved_module.package_id = resolved.package_id;
        }
        resolved_module
    }

    // Go: module/resolver.go:1391 createResolvedTypeReferenceDirective
    pub fn create_resolved_type_reference_directive(
        &mut self,
        resolved: Option<Resolved>,
        primary: bool,
    ) -> ResolvedTypeReferenceDirective {
        let mut resolved_type_reference_directive = ResolvedTypeReferenceDirective::default();
        resolved_type_reference_directive.resolution_diagnostics = self.diagnostics.clone();

        if is_resolved(&resolved) {
            let resolved = resolved.unwrap();
            if !extension_is_ts(&resolved.extension) {
                panic!("expected a TypeScript file extension");
            }
            resolved_type_reference_directive.resolved_file_name = resolved.path.clone();
            resolved_type_reference_directive.primary = primary;
            resolved_type_reference_directive.package_id = resolved.package_id;
            resolved_type_reference_directive.is_external_library_import =
                resolved.path.contains("/node_modules/");

            if self.compiler_options.preserve_symlinks != Tristate::True {
                let (original_path, resolved_file_name) =
                    self.get_original_and_resolved_file_name(&resolved.path);
                if !original_path.is_empty() {
                    resolved_type_reference_directive.resolved_file_name = resolved_file_name;
                    resolved_type_reference_directive.original_path = original_path;
                }
            }
        }
        resolved_type_reference_directive
    }

    // Go: module/resolver.go:1416 getOriginalAndResolvedFileName
    pub fn get_original_and_resolved_file_name(&mut self, file_name: &str) -> (String, String) {
        let resolved_file_name = self.real_path(file_name);
        let current_directory = self.resolver.host.get_current_directory();
        let compare_paths_options = ComparePathsOptions {
            use_case_sensitive_file_names: self.resolver.host.fs().use_case_sensitive_file_names(),
            current_directory: current_directory.to_string(),
        };
        if compare_paths(file_name, &resolved_file_name, &compare_paths_options) == 0 {
            // If the fileName and realpath are differing only in casing, prefer fileName
            // so that we can issue correct errors for casing under forceConsistentCasingInFileNames
            return (String::new(), file_name.to_string());
        }
        (file_name.to_string(), resolved_file_name)
    }

    // Go: module/resolver.go:1427 tryLoadModuleUsingOptionalResolutionSettings
    pub fn try_load_module_using_optional_resolution_settings(&mut self) -> Option<Resolved> {
        let resolved = self.try_load_module_using_paths_if_eligible();
        if resolved.is_some() {
            return resolved;
        }

        if !is_external_module_name_relative(&self.name) {
            // No more tryLoadModuleUsingBaseUrl.
            continue_searching()
        } else {
            self.try_load_module_using_root_dirs()
        }
    }

    // Go: module/resolver.go:1236 getParsedPatternsForPaths
    pub fn get_parsed_patterns_for_paths(&mut self) -> Rc<ParsedPatterns> {
        self.resolver
            .get_parsed_patterns_for_paths(&self.compiler_options)
    }

    // Go: module/resolver.go:1444 tryLoadModuleUsingPathsIfEligible
    pub fn try_load_module_using_paths_if_eligible(&mut self) -> Option<Resolved> {
        let compiler_options = self.compiler_options.clone();
        // Go `Paths.Size()` is 0 for a nil map.
        if compiler_options.paths.as_ref().map_or(0, IndexMap::len) > 0
            && !path_is_relative(&self.name)
        {
            trace_write!(
                self,
                diag::X_paths_option_is_specified_looking_for_a_pattern_to_match_module_name_0,
                self.name
            );
        } else {
            return continue_searching();
        }
        let current_directory = self.resolver.host.get_current_directory();
        let base_directory = compiler_options.get_paths_base_path(&current_directory);
        let path_patterns = self.get_parsed_patterns_for_paths();
        let extensions = self.extensions;
        let name = self.name.clone();
        self.try_load_module_using_paths(
            extensions,
            &name,
            &base_directory,
            compiler_options.paths.as_ref(),
            &path_patterns,
            &mut |state: &mut ResolutionState<'_>, extensions: Extensions, candidate: &str| {
                state.node_load_module_by_relative_name(
                    extensions, candidate, true, /*considerPackageJson*/
                )
            },
        )
    }

    // Go: module/resolver.go:1466 tryLoadModuleUsingPaths
    // PORT: Go `resolutionKindSpecificLoader` closures capture `r`. Here the
    // loader gets the state as its first argument.
    pub fn try_load_module_using_paths(
        &mut self,
        extensions: Extensions,
        module_name: &str,
        containing_directory: &str,
        paths: Option<&IndexMap<String, Option<Vec<String>>>>,
        path_patterns: &ParsedPatterns,
        loader: &mut dyn FnMut(&mut Self, Extensions, &str) -> Option<Resolved>,
    ) -> Option<Resolved> {
        let matched_pattern = match_pattern_or_exact(path_patterns, module_name);
        if matched_pattern.is_valid() {
            let matched_star = matched_pattern.matched_text(module_name);
            trace_write!(
                self,
                diag::Module_name_0_matched_pattern_1,
                module_name,
                matched_pattern.text
            );
            // Go `paths.GetOrZero(text)`: nil for a missing key or a nil map.
            // Go ranges over a nil slice as zero items.
            let substitutions = paths
                .and_then(|p| p.get(&matched_pattern.text))
                .cloned()
                .flatten()
                .unwrap_or_default();
            for subst in &substitutions {
                // PORT: Go joins the bytes (see `scanner_util::go_value`).
                let path = go_value_owned(subst.replacen('*', &matched_star, 1));
                let candidate = resolve_path_for_module(
                    containing_directory,
                    &path,
                    has_trailing_directory_separator(&path),
                );
                trace_write!(
                    self,
                    diag::Trying_substitution_0_candidate_module_location_Colon_1,
                    subst,
                    path
                );
                // A path mapping may have an extension
                let extension_from_subst = try_get_extension_from_path(subst);
                if !extension_from_subst.is_empty() {
                    let (path, ok) = self.try_file(&candidate);
                    if ok {
                        return Some(Resolved {
                            path,
                            extension: extension_from_subst.to_string(),
                            ..Default::default()
                        });
                    }
                }
                // When the substitution path has an explicit extension, the extension came from the
                // paths config, not the module specifier. Suppress resolvedUsingTsExtension in that case.
                let save_candidate_ending_is_from_config = self.candidate_ending_is_from_config;
                if !extension_from_subst.is_empty() {
                    self.candidate_ending_is_from_config = true;
                }
                let resolved = loader(self, extensions, &candidate);
                self.candidate_ending_is_from_config = save_candidate_ending_is_from_config;
                if resolved.is_some() {
                    return resolved;
                }
            }
        }
        continue_searching()
    }

    // Go: module/resolver.go:1504 tryLoadModuleUsingRootDirs
    pub fn try_load_module_using_root_dirs(&mut self) -> Option<Resolved> {
        if self
            .compiler_options
            .root_dirs
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            return continue_searching();
        }

        trace_write!(
            self,
            diag::X_rootDirs_option_is_set_using_it_to_resolve_relative_module_name_0,
            self.name
        );

        let candidate = resolve_path_for_module(
            &self.containing_directory,
            &self.name,
            has_trailing_directory_separator(&self.name),
        );

        let root_dirs = self.compiler_options.root_dirs.clone().unwrap_or_default();
        // ts#64159 (resolver.go:1518 HasDirectoryPrefix): the candidate is in a
        // root dir as a path (roots without case), and a directory candidate
        // is in its own root dir.
        let candidate_directory_only = has_trailing_directory_separator(&candidate);
        let candidate_path =
            if candidate_directory_only && candidate.len() > get_root_length(&candidate) {
                remove_trailing_directory_separator(&candidate)
            } else {
                candidate.as_str()
            };
        let mut matched_root_dir = String::new();
        let mut matched_normalized_root = String::new();
        let mut matched_relative = String::new();
        for root_dir in &root_dirs {
            // rootDirs are expected to be absolute
            // in case of tsconfig.json this will happen automatically - compiler will expand relative names
            // using location of tsconfig.json as base location
            let normalized_root = normalize_path(root_dir);
            let relative = relative_path_within_directory(&normalized_root, candidate_path, true)
                .filter(|relative| !relative.is_empty() || candidate_directory_only);
            let is_longest_matching_prefix = relative.is_some()
                && (matched_normalized_root.is_empty()
                    || matched_normalized_root.len() < normalized_root.len());

            trace_write!(
                self,
                diag::Checking_if_0_is_the_longest_matching_prefix_for_1_2,
                ensure_trailing_directory_separator(&normalized_root),
                candidate,
                is_longest_matching_prefix
            );

            if is_longest_matching_prefix {
                matched_relative = relative.unwrap().into_owned();
                matched_normalized_root = normalized_root;
                matched_root_dir = root_dir.clone();
            }
        }

        if !matched_normalized_root.is_empty() {
            let matched_normalized_prefix =
                ensure_trailing_directory_separator(&matched_normalized_root);
            trace_write!(
                self,
                diag::Longest_matching_prefix_for_0_is_1,
                candidate,
                matched_normalized_prefix
            );
            // Go: candidate.RelativeToDirectory(matchedRootDir)
            let suffix = if candidate_directory_only && !matched_relative.is_empty() {
                ensure_trailing_directory_separator(&matched_relative)
            } else {
                matched_relative
            };

            // first - try to load from a initial location
            trace_write!(
                self,
                diag::Loading_0_from_the_root_dir_1_candidate_location_2,
                suffix,
                matched_normalized_prefix,
                candidate
            );
            let loader = |state: &mut Self, extensions: Extensions, candidate: &str| {
                state.node_load_module_by_relative_name(
                    extensions, candidate, true, /*considerPackageJson*/
                )
            };
            let extensions = self.extensions;
            let resolved_file_name = loader(self, extensions, &candidate);
            if resolved_file_name.is_some() {
                return resolved_file_name;
            }

            trace_write!(self, diag::Trying_other_entries_in_rootDirs);
            // then try to resolve using remaining entries in rootDirs
            for root_dir in &root_dirs {
                if *root_dir == matched_root_dir {
                    // skip the initially matched entry
                    continue;
                }
                // ts#64544: a suffix and a root dir that are dynamic paths
                // map between their encoded and logical forms.
                let directory_only = suffix.is_empty() || has_trailing_directory_separator(&suffix);
                let logical_suffix = suffix.as_str();
                // ts#64159 (resolver.go:1556-1573): the suffix is relative to
                // the root dir, never rooted ("d:/generated" + "c:/dep"), and
                // an empty suffix names the root dir itself.
                let candidate = match (
                    is_encoded_dynamic_file_name(root_dir),
                    is_encoded_dynamic_file_name(&matched_root_dir),
                ) {
                    (true, true) => candidate_from_dynamic_logical_path(
                        &normalize_path(root_dir),
                        &decode_dynamic_uri_path(logical_suffix),
                    ),
                    (true, false) => candidate_from_dynamic_logical_path(
                        &normalize_path(root_dir),
                        logical_suffix,
                    ),
                    (false, true) => {
                        let Some(decoded) = decode_dynamic_uri_path_for_disk(logical_suffix) else {
                            continue;
                        };
                        candidate_from_relative_path(
                            &normalize_path(root_dir),
                            &decoded,
                            directory_only,
                        )
                    }
                    (false, false) => candidate_from_relative_path(
                        &normalize_path(root_dir),
                        logical_suffix,
                        directory_only,
                    ),
                };
                trace_write!(
                    self,
                    diag::Loading_0_from_the_root_dir_1_candidate_location_2,
                    suffix,
                    root_dir,
                    candidate
                );
                let extensions = self.extensions;
                let resolved_file_name = loader(self, extensions, &candidate);
                if resolved_file_name.is_some() {
                    return resolved_file_name;
                }
            }
            trace_write!(self, diag::Module_resolution_using_rootDirs_has_failed);
        }
        continue_searching()
    }

    // Go: module/resolver.go:1588 nodeLoadModuleByRelativeName
    pub fn node_load_module_by_relative_name(
        &mut self,
        extensions: Extensions,
        candidate: &str,
        consider_package_json: bool,
    ) -> Option<Resolved> {
        trace_write!(
            self,
            diag::Loading_module_as_file_Slash_folder_candidate_module_location_0_target_file_types_Colon_1,
            candidate,
            extensions.string()
        );
        if !has_trailing_directory_separator(candidate) {
            let parent_of_candidate = get_directory_path(candidate);
            if !self
                .resolver
                .host
                .fs()
                .directory_exists(&parent_of_candidate)
            {
                trace_write!(
                    self,
                    diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    parent_of_candidate
                );
                return continue_searching();
            }
            let resolved_from_file = self.load_module_from_file(extensions, candidate);
            if let Some(mut resolved_from_file) = resolved_from_file {
                if consider_package_json {
                    let package_directory =
                        node_module_package_root_for_file(&resolved_from_file.path);
                    if !package_directory.is_empty() {
                        let package_info = self.get_package_json_info(&package_directory);
                        let path = resolved_from_file.path.clone();
                        resolved_from_file.package_id = self.get_package_id(&path, &package_info);
                    }
                }
                return Some(resolved_from_file);
            }
        }
        // ts#64544: a dynamic candidate is looked up as a directory segment.
        // ts#64159 (resolver.go:1610): Go checks `candidate.AsDirectoryPath()`
        // and traces the candidate itself, not its directory spelling.
        let directory_candidate = candidate_directory_path(candidate);
        if !self
            .resolver
            .host
            .fs()
            .directory_exists(&directory_candidate)
        {
            trace_write!(
                self,
                diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                candidate
            );
            return continue_searching();
        }
        // esm mode relative imports shouldn't do any directory lookups (either inside `package.json`
        // files or implicit `index.js`es). This is a notable departure from cjs norms, where `./foo/pkg`
        // could have been redirected by `./foo/pkg/package.json` to an arbitrary location!
        if !self.esm_mode {
            return self.load_node_module_from_directory(
                extensions,
                &directory_candidate,
                consider_package_json,
            );
        }
        continue_searching()
    }

    // Go: module/resolver.go:1625 loadModuleFromFile
    pub fn load_module_from_file(
        &mut self,
        extensions: Extensions,
        candidate: &str,
    ) -> Option<Resolved> {
        // ./foo.js -> ./foo.ts
        let resolved_by_replacing_extension =
            self.load_module_from_file_no_implicit_extensions(extensions, candidate);
        if resolved_by_replacing_extension.is_some() {
            return resolved_by_replacing_extension;
        }

        // ./foo -> ./foo.ts
        if !self.esm_mode {
            // ts#64159: Go adds extensions to `candidate.path`, the name
            // without the trailing separator of a directory-only candidate.
            return self.try_adding_extensions(candidate_path(candidate), extensions, "");
        }

        continue_searching()
    }

    // Go: module/resolver.go:1640 loadModuleFromFileNoImplicitExtensions
    pub fn load_module_from_file_no_implicit_extensions(
        &mut self,
        extensions: Extensions,
        candidate: &str,
    ) -> Option<Resolved> {
        // ts#64159 (resolver.go:48 resolutionCandidate): a directory-only
        // candidate ("pkg/v.d/") splits its extension on the name without the
        // trailing separator (Go `candidate.path`, SplitExtension :208), and
        // the trace prints the candidate with it (Go `candidate.String()`).
        let path = candidate_path(candidate);
        let base = get_base_file_name(path);
        if !base.contains('.') {
            return continue_searching(); // extensionless import, no lookups performed, since we don't support extensionless files
        }
        let mut extensionless = remove_file_extension(path);
        if extensionless == path {
            // Once TS native extensions are handled, handle arbitrary extensions for declaration file mapping
            let mut extension =
                get_longest_extension_from_path(path, &self.resolver.extra_extensions, false);
            if extension.is_empty() {
                extension = path[path.rfind('.').unwrap()..].to_string();
            }
            extensionless = remove_extension(path, &extension);
        }

        let extension = &path[extensionless.len()..];
        trace_write!(
            self,
            diag::File_name_0_has_a_1_extension_stripping_it,
            candidate,
            extension
        );
        self.try_adding_extensions(extensionless, extensions, extension)
    }

    // Go: module/resolver.go:1652 tryAddingExtensions
    pub fn try_adding_extensions(
        &mut self,
        extensionless: &str,
        extensions: Extensions,
        original_extension: &str,
    ) -> Option<Resolved> {
        let directory = get_directory_path(extensionless);
        if !directory.is_empty() && !self.resolver.host.fs().directory_exists(&directory) {
            return continue_searching();
        }

        match original_extension {
            EXTENSION_MJS | EXTENSION_MTS | EXTENSION_DMTS => {
                let from_ts =
                    original_extension == EXTENSION_MTS || original_extension == EXTENSION_DMTS;
                if extensions.intersects(Extensions::TYPE_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_MTS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::DECLARATION) {
                    let resolved = self.try_extension(EXTENSION_DMTS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::JAVA_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_MJS, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
            EXTENSION_CJS | EXTENSION_CTS | EXTENSION_DCTS => {
                let from_ts =
                    original_extension == EXTENSION_CTS || original_extension == EXTENSION_DCTS;
                if extensions.intersects(Extensions::TYPE_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_CTS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::DECLARATION) {
                    let resolved = self.try_extension(EXTENSION_DCTS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::JAVA_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_CJS, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
            EXTENSION_JSON => {
                if extensions.intersects(Extensions::DECLARATION) {
                    let resolved = self.try_extension(".d.json.ts", extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::JSON) {
                    let resolved = self.try_extension(EXTENSION_JSON, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
            EXTENSION_TSX | EXTENSION_JSX => {
                // basically idendical to the ts/js case below, but prefers matching tsx and jsx files exactly before falling back to the ts or js file path
                // (historically, we disallow having both a a.ts and a.tsx file in the same compilation, since their outputs clash)
                // TODO: We should probably error if `"./a.tsx"` resolved to `"./a.ts"`, right?
                let from_tsx = original_extension == EXTENSION_TSX;
                if extensions.intersects(Extensions::TYPE_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_TSX, extensionless, from_tsx);
                    if resolved.is_some() {
                        return resolved;
                    }
                    let resolved = self.try_extension(EXTENSION_TS, extensionless, from_tsx);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::DECLARATION) {
                    let resolved = self.try_extension(EXTENSION_DTS, extensionless, from_tsx);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::JAVA_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_JSX, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                    let resolved = self.try_extension(EXTENSION_JS, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
            EXTENSION_TS | EXTENSION_DTS | EXTENSION_JS | "" => {
                let from_ts =
                    original_extension == EXTENSION_TS || original_extension == EXTENSION_DTS;
                if extensions.intersects(Extensions::TYPE_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_TS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                    let resolved = self.try_extension(EXTENSION_TSX, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::DECLARATION) {
                    let resolved = self.try_extension(EXTENSION_DTS, extensionless, from_ts);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if extensions.intersects(Extensions::JAVA_SCRIPT) {
                    let resolved = self.try_extension(EXTENSION_JS, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                    let resolved = self.try_extension(EXTENSION_JSX, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                if self.is_config_lookup {
                    let resolved = self.try_extension(EXTENSION_JSON, extensionless, false);
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
            _ => {
                if self
                    .resolver
                    .extra_extensions
                    .iter()
                    .any(|e| e == original_extension)
                {
                    // A fully specified import of an extraExtension resolves directly to the file.
                    let resolved = self.try_extension(original_extension, extensionless, false);
                    if let Some(mut resolved) = resolved {
                        resolved.resolved_using_extra_extensions = true;
                        return Some(resolved);
                    }
                }
                if extensions.intersects(Extensions::DECLARATION)
                    && !is_declaration_file_name(&format!("{extensionless}{original_extension}"))
                {
                    let resolved = self.try_extension(
                        &format!(".d{original_extension}.ts"),
                        extensionless,
                        false,
                    );
                    if resolved.is_some() {
                        return resolved;
                    }
                }
                continue_searching()
            }
        }
    }

    // Go: module/resolver.go:1775 tryExtension
    pub fn try_extension(
        &mut self,
        extension: &str,
        extensionless: &str,
        resolved_using_ts_extension: bool,
    ) -> Option<Resolved> {
        let file_name = format!("{extensionless}{extension}");
        let (path, ok) = self.try_file(&file_name);
        if ok {
            return Some(Resolved {
                path,
                extension: extension.to_string(),
                resolved_using_ts_extension: !self.candidate_ending_is_from_config
                    && resolved_using_ts_extension,
                ..Default::default()
            });
        }
        continue_searching()
    }

    // Go: module/resolver.go:1787 tryFile
    pub fn try_file(&mut self, file_name: &str) -> (String, bool) {
        // ts#64159: a directory-only candidate is never a file.
        if has_trailing_directory_separator(file_name) {
            return (String::new(), false);
        }
        if self
            .compiler_options
            .module_suffixes
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            return (file_name.to_string(), self.try_file_lookup(file_name));
        }

        let ext = try_get_extension_from_path(file_name);
        let file_name_no_extension = remove_extension(file_name, ext).to_string();
        let module_suffixes = self
            .compiler_options
            .module_suffixes
            .clone()
            .unwrap_or_default();
        for suffix in &module_suffixes {
            let path = format!("{file_name_no_extension}{suffix}{ext}");
            if self.try_file_lookup(&path) {
                return (path, true);
            }
        }
        (file_name.to_string(), false)
    }

    // Go: module/resolver.go:1810 tryFileLookup
    pub fn try_file_lookup(&mut self, file_name: &str) -> bool {
        if self.resolver.host.fs().file_exists(file_name) {
            trace_write!(
                self,
                diag::File_0_exists_use_it_as_a_name_resolution_result,
                file_name
            );
            return true;
        } else {
            trace_write!(self, diag::File_0_does_not_exist, file_name);
        }
        false
    }

    // Go: module/resolver.go:1822 loadNodeModuleFromDirectory
    pub fn load_node_module_from_directory(
        &mut self,
        extensions: Extensions,
        candidate: &str,
        consider_package_json: bool,
    ) -> Option<Resolved> {
        let mut package_info: Option<Rc<InfoCacheEntry>> = None;
        if consider_package_json {
            package_info = self.get_package_json_info(candidate);
        }

        self.load_node_module_from_directory_worker(extensions, candidate, &package_info)
    }

    // Go: module/resolver.go:1832 loadNodeModuleFromDirectoryWorker
    pub fn load_node_module_from_directory_worker(
        &mut self,
        ext: Extensions,
        candidate: &str,
        package_info: &Option<Rc<InfoCacheEntry>>,
    ) -> Option<Resolved> {
        let mut package_file = String::new();
        let mut version_paths = VersionPaths::default();
        if let Some(info) = package_info.as_ref().filter(|p| p.exists()) {
            let contents = info.contents.clone().unwrap();
            version_paths = contents.get_version_paths(self.get_trace_func());
            let options = ComparePathsOptions {
                use_case_sensitive_file_names: self
                    .resolver
                    .host
                    .fs()
                    .use_case_sensitive_file_names(),
                ..Default::default()
            };
            if compare_paths(candidate, &info.package_directory, &options) == 0 {
                let (file, ok) = self.get_package_file(ext, package_info);
                if ok {
                    package_file = file;
                }
            }
        }

        let package_is_module = package_info.as_ref().filter(|p| p.exists()).map(|p| {
            p.contents
                .as_ref()
                .unwrap()
                .fields
                .header_fields
                .type_
                .value
                == "module"
        });
        let mut loader = |state: &mut Self,
                          extensions: Extensions,
                          candidate: &str|
         -> Option<Resolved> {
            let from_file =
                state.load_file_name_from_package_json_field(extensions, candidate, &package_file);
            if from_file.is_some() {
                return from_file;
            }

            // Even if `extensions == extensionsDeclaration`, we can still look up a .ts file as a result of package.json "types"
            // !!! should we not set this before the filename lookup above?
            let mut expanded_extensions = extensions;
            if extensions == Extensions::DECLARATION {
                expanded_extensions = Extensions::TYPE_SCRIPT | Extensions::DECLARATION;
            }

            // Disable `esmMode` for the resolution of the package path for CJS-mode packages (so the `main` field can omit extensions)
            let save_esm_mode = state.esm_mode;
            let save_candidate_ending_is_from_config = state.candidate_ending_is_from_config;
            state.candidate_ending_is_from_config = true;
            // Go: `packageInfo.Exists() && packageInfo.Contents.Type.Value != "module"`.
            if package_is_module == Some(false) {
                state.esm_mode = false;
            }
            let result = state.node_load_module_by_relative_name(
                expanded_extensions,
                candidate,
                false, /*considerPackageJson*/
            );
            state.esm_mode = save_esm_mode;
            state.candidate_ending_is_from_config = save_candidate_ending_is_from_config;
            result
        };

        let index_path = if self.is_config_lookup {
            combine_paths(candidate, &["tsconfig"])
        } else {
            combine_paths(candidate, &["index"])
        };

        if version_paths.exists()
            && (package_file.is_empty()
                || contains_path(candidate, &package_file, &ComparePathsOptions::default()))
        {
            let module_name = if !package_file.is_empty() {
                get_relative_path_from_directory(
                    candidate,
                    &package_file,
                    &ComparePathsOptions::default(),
                )
            } else {
                get_relative_path_from_directory(
                    candidate,
                    &index_path,
                    &ComparePathsOptions::default(),
                )
            };
            // ts#64544: the typesVersions patterns match the logical name.
            let module_name = if is_encoded_dynamic_file_name(candidate) {
                decode_dynamic_uri_path(&module_name)
            } else {
                module_name
            };
            trace_write!(
                self,
                diag::X_package_json_has_a_typesVersions_entry_0_that_matches_compiler_version_1_looking_for_a_pattern_to_match_module_name_2,
                version_paths.version,
                version(),
                module_name
            );
            let paths = version_paths.get_paths();
            let path_patterns = try_parse_patterns(paths);
            let result = self.try_load_module_using_paths(
                ext,
                &module_name,
                candidate,
                paths,
                &path_patterns,
                &mut loader,
            );
            if let Some(result) = result {
                if !result.package_id.name.is_empty() {
                    // !!! are these asserts really necessary?
                    panic!("expected packageId to be empty");
                }
                return Some(result);
            }
        }

        if !package_file.is_empty() {
            let package_file_result = loader(self, ext, &package_file);
            if let Some(package_file_result) = package_file_result {
                if !package_file_result.package_id.name.is_empty() {
                    // !!! are these asserts really necessary?
                    panic!("expected packageId to be empty");
                }
                return Some(package_file_result);
            }
        }

        // ESM mode resolutions don't do package 'index' lookups
        if !self.esm_mode {
            if !self.resolver.host.fs().directory_exists(candidate) {
                return continue_searching();
            }
            return self.load_module_from_file(ext, &index_path);
        }
        continue_searching()
    }

    // Go: module/resolver.go:1928 loadFileNameFromPackageJSONField
    // This function is only ever called with paths written in package.json files - never
    // module specifiers written in source files - and so it always allows the
    // candidate to end with a TS extension (but will also try substituting a JS extension for a TS extension).
    pub fn load_file_name_from_package_json_field(
        &mut self,
        extensions: Extensions,
        candidate: &str,
        package_json_value: &str,
    ) -> Option<Resolved> {
        // ts#64159: a directory-only candidate is never a file.
        if has_trailing_directory_separator(candidate) {
            return continue_searching();
        }
        if extensions.intersects(Extensions::TYPE_SCRIPT)
            && has_implementation_ts_file_extension(candidate)
            || extensions.intersects(Extensions::DECLARATION) && is_declaration_file_name(candidate)
        {
            let (path, ok) = self.try_file(candidate);
            if ok {
                let extension = try_extract_ts_extension(&path);
                // resolvedUsingTsExtension should be true when the pattern ends with * and the
                // candidate file ends in a TS extension. This means the * matched a TS extension
                // from the module specifier. For example:
                // - import "pkg/foo.ts" with pattern "./*" -> true
                // - import "pkg/foo.ts.omg" with pattern "./*.omg" -> true (star matched .ts)
                // - import "pkg/foo" with pattern "./*.ts" -> false (extension in pattern, not specifier)
                let resolved_using_ts_extension =
                    package_json_value.ends_with('*') && !extension.is_empty();
                return Some(Resolved {
                    path,
                    extension: extension.to_string(),
                    resolved_using_ts_extension,
                    ..Default::default()
                });
            }
            return continue_searching();
        }

        if self.is_config_lookup
            && extensions.intersects(Extensions::JSON)
            && file_extension_is(candidate, EXTENSION_JSON)
        {
            let (path, ok) = self.try_file(candidate);
            if ok {
                return Some(Resolved {
                    path,
                    extension: EXTENSION_JSON.to_string(),
                    ..Default::default()
                });
            }
        }

        self.load_module_from_file_no_implicit_extensions(extensions, candidate)
    }

    // Go: module/resolver.go:1964 getPackageFile
    pub fn get_package_file(
        &mut self,
        extensions: Extensions,
        package_info: &Option<Rc<InfoCacheEntry>>,
    ) -> (String, bool) {
        if !info_exists(package_info) {
            return (String::new(), false);
        }
        let package_info = package_info.as_ref().unwrap();
        let contents = package_info.contents.clone().unwrap();
        let path_fields = &contents.fields.path_fields;
        if self.is_config_lookup {
            return self.get_package_json_path_field(
                "tsconfig",
                &path_fields.ts_config,
                &package_info.package_directory,
            );
        }
        if extensions.intersects(Extensions::DECLARATION) {
            let (package_file, ok) = self.get_package_json_path_field(
                "typings",
                &path_fields.typings,
                &package_info.package_directory,
            );
            if ok {
                return (package_file, ok);
            }
            let (package_file, ok) = self.get_package_json_path_field(
                "types",
                &path_fields.types,
                &package_info.package_directory,
            );
            if ok {
                return (package_file, ok);
            }
        }
        if extensions.intersects(Extensions::IMPLEMENTATION_FILES | Extensions::DECLARATION) {
            return self.get_package_json_path_field(
                "main",
                &path_fields.main,
                &package_info.package_directory,
            );
        }
        (String::new(), false)
    }

    // Go: module/resolver.go:1985 getPackageJsonInfo
    pub fn get_package_json_info(&mut self, package_directory: &str) -> Option<Rc<InfoCacheEntry>> {
        let package_json_path = combine_paths(package_directory, &["package.json"]);

        if let Some(existing) = self
            .resolver
            .caches
            .package_json_info_cache
            .get(&package_json_path)
        {
            self.resolver.caches.log_package_json(&existing);
            if existing.contents.is_some() {
                trace_write!(
                    self,
                    diag::File_0_exists_according_to_earlier_cached_lookups,
                    package_json_path
                );
                return Some(existing.with_package_directory(package_directory));
            } else {
                if existing.directory_exists {
                    trace_write!(
                        self,
                        diag::File_0_does_not_exist_according_to_earlier_cached_lookups,
                        package_json_path
                    );
                }
                return None;
            }
        }

        let directory_exists = self.resolver.host.fs().directory_exists(package_directory);
        if directory_exists && self.resolver.host.fs().file_exists(&package_json_path) {
            // Ignore error
            let (contents, _) = self.resolver.host.fs().read_file(&package_json_path);
            // PORT: Go `packagejson.Parse` returns zero `Fields` and an error for an invalid file.
            // `contents` is the port form of the Go text; Go parses its bytes.
            let parsed = crate::frontend::packagejson::parse(&go_string_bytes(&contents));
            let parseable = parsed.is_ok();
            let package_json_content = parsed.unwrap_or_default();
            trace_write!(self, diag::Found_package_json_at_0, package_json_path);
            let result = Rc::new(InfoCacheEntry {
                package_directory: package_directory.to_string(),
                directory_exists: true,
                contents: Some(Rc::new(PackageJson {
                    fields: package_json_content,
                    parseable,
                    ..Default::default()
                })),
            });
            let result = self
                .resolver
                .caches
                .package_json_info_cache
                .set(&package_json_path, result);
            self.resolver.caches.log_package_json(&result);
            return Some(result.with_package_directory(package_directory));
        } else {
            if directory_exists {
                trace_write!(self, diag::File_0_does_not_exist, package_json_path);
            }
            let stored = self.resolver.caches.package_json_info_cache.set(
                &package_json_path,
                Rc::new(InfoCacheEntry {
                    package_directory: package_directory.to_string(),
                    directory_exists,
                    contents: None,
                }),
            );
            self.resolver.caches.log_package_json(&stored);
        }
        None
    }

    // Go: module/resolver.go:2035 getPackageId
    pub fn get_package_id(
        &mut self,
        resolved_file_name: &str,
        package_info: &Option<Rc<InfoCacheEntry>>,
    ) -> PackageId {
        if info_exists(package_info) {
            let package_info = package_info.as_ref().unwrap();
            let package_json_content = package_info.contents.clone().unwrap();
            let (name, ok) = package_json_content.fields.header_fields.name.get_value();
            if ok {
                let (version, ok) = package_json_content
                    .fields
                    .header_fields
                    .version
                    .get_value();
                if ok {
                    let mut sub_module_name = String::new();
                    if resolved_file_name.len() > package_info.package_directory.len() {
                        sub_module_name = resolved_file_name
                            [package_info.package_directory.len() + 1..]
                            .to_string();
                    }
                    return PackageId {
                        name,
                        version,
                        sub_module_name,
                        peer_dependencies: self.read_package_json_peer_dependencies(package_info),
                    };
                }
            }
        }
        PackageId::default()
    }

    // Go: module/resolver.go:2056 readPackageJsonPeerDependencies
    pub fn read_package_json_peer_dependencies(
        &mut self,
        package_json_info: &Rc<InfoCacheEntry>,
    ) -> String {
        let contents = package_json_info.contents.clone().unwrap();
        let peer_dependencies = &contents.fields.dependency_fields.peer_dependencies;
        let ok = self.validate_package_json_field("peerDependencies", peer_dependencies);
        if !ok || peer_dependencies.value.is_empty() {
            return String::new();
        }
        trace_write!(self, diag::X_package_json_has_a_peerDependencies_field);
        let package_directory = self.real_path(&package_json_info.package_directory);
        let Some(node_modules_index) = package_directory.rfind("/node_modules") else {
            return String::new();
        };
        let node_modules = format!(
            "{}/",
            &package_directory[..node_modules_index + "/node_modules".len()]
        );
        let mut names: Vec<String> = peer_dependencies.value.keys().cloned().collect();
        // PORT: Go sorts by the bytes of the names (see `compare_go_bytes`).
        crate::gostd::slices::stable_sort_by(&mut names, |a, b| compare_go_bytes(a, b));
        let mut builder = String::new();
        for name in &names {
            let peer_package_json =
                self.get_package_json_info(&resolve_path_for_module(&node_modules, name, true));
            if let Some(peer_package_json) = peer_package_json.filter(|p| p.exists()) {
                let version = peer_package_json
                    .contents
                    .as_ref()
                    .unwrap()
                    .fields
                    .header_fields
                    .version
                    .value
                    .clone();
                builder.push('+');
                builder.push_str(name);
                builder.push('@');
                builder.push_str(&version);
                trace_write!(
                    self,
                    diag::Found_peerDependency_0_with_1_version,
                    name,
                    version
                );
            } else {
                trace_write!(self, diag::Failed_to_find_peerDependency_0, name);
            }
        }
        builder
    }

    // Go: module/resolver.go:2091 realPath
    pub fn real_path(&mut self, path: &str) -> String {
        let rp = normalize_path(&self.resolver.host.fs().realpath(path));
        trace_write!(self, diag::Resolving_real_path_for_0_result_1, path, rp);
        rp
    }

    // Go: module/resolver.go:2099 validatePackageJSONField
    pub fn validate_package_json_field(
        &mut self,
        field_name: &str,
        field: &dyn TypeValidatedField,
    ) -> bool {
        if field.is_present() {
            if field.is_valid() {
                return true;
            }
            trace_write!(
                self,
                diag::Expected_type_of_0_field_in_package_json_to_be_1_got_2,
                field_name,
                field.expected_json_type(),
                field.actual_json_type()
            );
        }
        trace_write!(
            self,
            diag::X_package_json_does_not_have_a_0_field,
            field_name
        );
        false
    }

    // Go: module/resolver.go:2114 getPackageJSONPathField
    pub fn get_package_json_path_field(
        &mut self,
        field_name: &str,
        field: &Expected<String>,
        directory: &str,
    ) -> (String, bool) {
        if !self.validate_package_json_field(field_name, field) {
            return (String::new(), false);
        }
        if field.value.is_empty() {
            trace_write!(self, diag::X_package_json_had_a_falsy_0_field, field_name);
            return (String::new(), false);
        }
        let path = resolve_path_for_module(
            directory,
            &field.value,
            has_trailing_directory_separator(&field.value),
        );
        trace_write!(
            self,
            diag::X_package_json_has_0_field_1_that_references_2,
            field_name,
            field.value,
            path
        );
        (path, true)
    }

    // Go: module/resolver.go:2131 conditionMatches
    pub fn condition_matches(&self, condition: &str) -> bool {
        if condition == "default" || self.conditions.iter().any(|c| c == condition) {
            return true;
        }
        if !self.conditions.iter().any(|c| c == "types") {
            return false; // only apply versioned types conditions if the types condition is applied
        }
        is_applicable_versioned_types_key(condition)
    }

    // Go: module/resolver.go:2141 getTraceFunc
    // PORT: the Go method value `r.tracer.write` is a closure over the shared tracer.
    pub fn get_trace_func(
        &self,
    ) -> Option<Box<dyn Fn(&'static crate::diagnostics::Message, Vec<String>)>> {
        if let Some(tracer) = &self.tracer {
            let tracer = tracer.clone();
            return Some(Box::new(move |m, args| tracer.borrow_mut().write(m, args)));
        }
        None
    }
}

// Go: module/resolver.go:2148 GetConditions
pub fn get_conditions(options: &CompilerOptions, resolution_mode: ResolutionMode) -> Vec<String> {
    let mut resolution_mode = resolution_mode;
    let module_resolution = options.get_module_resolution_kind();
    if resolution_mode == ModuleKind::NONE && module_resolution == ModuleResolutionKind::BUNDLER {
        resolution_mode = ModuleKind::ES_NEXT;
    }
    let custom_conditions = options.custom_conditions.as_deref().unwrap_or_default();
    let mut conditions: Vec<String> = Vec::with_capacity(3 + custom_conditions.len());
    if resolution_mode == ModuleKind::ES_NEXT {
        conditions.push("import".to_string());
    } else {
        conditions.push("require".to_string());
    }

    if options.no_dts_resolution != Tristate::True {
        conditions.push("types".to_string());
    }
    if module_resolution != ModuleResolutionKind::BUNDLER {
        conditions.push("node".to_string());
    }
    conditions.extend(custom_conditions.iter().cloned());
    conditions
}

// Go: module/resolver.go:2170 getNodeResolutionFeatures
pub fn get_node_resolution_features(options: &CompilerOptions) -> NodeResolutionFeatures {
    let mut features = NodeResolutionFeatures::NONE;

    match options.get_module_resolution_kind() {
        ModuleResolutionKind::NODE16 => features = NodeResolutionFeatures::NODE16_DEFAULT,
        ModuleResolutionKind::NODE_NEXT => features = NodeResolutionFeatures::NODE_NEXT_DEFAULT,
        ModuleResolutionKind::BUNDLER => features = NodeResolutionFeatures::BUNDLER_DEFAULT,
        _ => {}
    }
    if options.resolve_package_json_exports == Tristate::True {
        features |= NodeResolutionFeatures::EXPORTS;
    } else if options.resolve_package_json_exports == Tristate::False {
        features = features.without(NodeResolutionFeatures::EXPORTS);
    }
    if options.resolve_package_json_imports == Tristate::True {
        features |= NodeResolutionFeatures::IMPORTS;
    } else if options.resolve_package_json_imports == Tristate::False {
        features = features.without(NodeResolutionFeatures::IMPORTS);
    }
    features
}

// Go: module/resolver.go:2194 moveToNextDirectorySeparatorIfAvailable
pub fn move_to_next_directory_separator_if_available(
    path: &str,
    prev_separator_index: i32,
    is_folder: bool,
) -> i32 {
    let offset = (prev_separator_index + 1) as usize;
    // PORT: Go slices bytes. `offset` is one byte after the start of the
    // package name, which can be inside a multi-byte character.
    let next_separator_index = path
        .as_bytes()
        .get(offset..)
        .and_then(|rest| rest.iter().position(|&b| b == b'/'));
    match next_separator_index {
        None => {
            if is_folder {
                return path.len() as i32;
            }
            prev_separator_index
        }
        Some(next_separator_index) => (next_separator_index + offset) as i32,
    }
}

// Go: core/pattern.go:5 Pattern
// PORT: Go `core.Pattern` has no crate module in the frontend plan. Only the
// module resolver uses it on the frontend, so it lives here. binder_p1.rs keeps
// its own private copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pattern {
    pub text: String,
    pub star_index: i32, // -1 for exact match
}

impl Pattern {
    // Go: core/pattern.go:18 IsValid
    pub fn is_valid(&self) -> bool {
        self.star_index == -1 || (self.star_index as usize) < self.text.len()
    }

    // Go: core/pattern.go:22 Matches
    // PORT: Go compares bytes. `text` and `candidate` are port forms, so
    // this compares their Go bytes (see `scanner_util::GO_STRING_MARKER`).
    // `star_index` is the port offset of the star.
    pub fn matches(&self, candidate: &str) -> bool {
        if self.star_index == -1 {
            return self.text == candidate;
        }
        let star = self.star_index as usize;
        go_len(candidate) + 1 >= go_len(&self.text)
            && go_has_prefix(candidate, &self.text[..star])
            && go_has_suffix(candidate, &self.text[star + 1..])
    }

    // Go: core/pattern.go:31 MatchedText
    // PORT: Go slices the candidate bytes (see `matches`).
    pub fn matched_text<'c>(&self, candidate: &'c str) -> Cow<'c, str> {
        if !self.matches(candidate) {
            panic!("candidate does not match pattern");
        }
        if self.star_index == -1 {
            return Cow::Borrowed("");
        }
        let star = self.star_index as usize;
        let prefix = go_len(&self.text[..star]);
        let suffix = go_len(&self.text[star + 1..]);
        go_slice(candidate, prefix, go_len(candidate) - suffix)
    }

    /// Go `StarIndex`: the Go byte offset of the star, or -1.
    fn go_star_index(&self) -> i32 {
        if self.star_index == -1 {
            return -1;
        }
        go_len(&self.text[..self.star_index as usize]) as i32
    }
}

// Go: core/pattern.go:10 TryParsePattern
pub fn try_parse_pattern(pattern: &str) -> Pattern {
    let star_index = pattern.find('*');
    match star_index {
        None => Pattern {
            text: pattern.to_string(),
            star_index: -1,
        },
        Some(i) if !pattern[i + 1..].contains('*') => Pattern {
            text: pattern.to_string(),
            star_index: i as i32,
        },
        Some(_) => Pattern::default(),
    }
}

// Go: core/pattern.go:41 FindBestPatternMatch
// PORT: only the `core.Identity` form is used, so this takes the patterns directly.
pub fn find_best_pattern_match(values: &[Pattern], candidate: &str) -> Pattern {
    let mut best_pattern = Pattern::default();
    let mut longest_match_prefix_length: i32 = -1;
    for pattern in values {
        let star_index = pattern.go_star_index();
        if (star_index == -1 || star_index > longest_match_prefix_length)
            && pattern.matches(candidate)
        {
            best_pattern = pattern.clone();
            longest_match_prefix_length = star_index;
        }
    }
    best_pattern
}

// Go: module/resolver.go:2209 ParsedPatterns
#[derive(Clone, Debug, Default)]
pub struct ParsedPatterns {
    matchable_string_set: FxHashSet<String>,
    patterns: Vec<Pattern>,
}

impl DefaultResolver {
    // Go: module/resolver.go:1991 getParsedPatternsForPaths
    pub fn get_parsed_patterns_for_paths(
        &self,
        compiler_options: &Rc<CompilerOptions>,
    ) -> Rc<ParsedPatterns> {
        self.caches.parsed_patterns_for_paths.get(compiler_options)
    }
}

// Go: module/resolver.go:2218 TryParsePatterns
// PORT: a nil `*OrderedMap` is `None`.
pub fn try_parse_patterns(
    path_mappings: Option<&IndexMap<String, Option<Vec<String>>>>,
) -> ParsedPatterns {
    let empty = IndexMap::new();
    let path_mappings = path_mappings.unwrap_or(&empty);
    let paths = path_mappings.keys();

    let mut num_patterns = 0;
    let mut num_matchables = 0;
    for path in paths.clone() {
        let pattern = try_parse_pattern(path);
        if pattern.is_valid() {
            if pattern.star_index == -1 {
                num_matchables += 1;
            } else {
                num_patterns += 1;
            }
        }
    }

    let mut patterns: Vec<Pattern> = Vec::new();
    let mut matchable_string_set: FxHashSet<String> = FxHashSet::default();
    if num_patterns != 0 {
        patterns.reserve(num_patterns);
    }
    if num_matchables != 0 {
        matchable_string_set.reserve(num_matchables);
    }

    for path in paths {
        let pattern = try_parse_pattern(path);
        if pattern.is_valid() {
            if pattern.star_index == -1 {
                matchable_string_set.insert(path.clone());
            } else {
                patterns.push(pattern);
            }
        }
    }
    ParsedPatterns {
        matchable_string_set,
        patterns,
    }
}

// Go: module/resolver.go:2257 MatchPatternOrExact
pub fn match_pattern_or_exact(patterns: &ParsedPatterns, candidate: &str) -> Pattern {
    if patterns.matchable_string_set.contains(candidate) {
        return Pattern {
            text: candidate.to_string(),
            star_index: -1,
        };
    }
    if patterns.patterns.is_empty() {
        return Pattern::default();
    }
    find_best_pattern_match(&patterns.patterns, candidate)
}

// Go: module/resolver.go:2275 normalizePathForCJSResolution
// If you import from "." inside a containing directory "/foo", the result of `tspath.NormalizePath`
// would be "/foo", but this loses the information that `foo` is a directory and we intended
// to look inside of it. The Node CommonJS resolution algorithm doesn't call this out
// (https://nodejs.org/api/modules.html#all-together), but it seems that module paths ending
// in `.` are actually normalized to `./` before proceeding with the resolution algorithm.
pub fn normalize_path_for_cjs_resolution(containing_directory: &str, module_name: &str) -> String {
    // ts#64159: a name with a trailing separator is encoded as a directory
    // (Go resolutionCandidateFromDirectoryPath).
    let combined = combine_paths(
        containing_directory,
        &[&path_for_dynamic_resolution(
            containing_directory,
            module_name,
            has_trailing_directory_separator(module_name),
        )],
    );
    // PORT: Go builds `GetPathComponents(combined, "")` only to read its last
    // entry. `last_path_component` reads that entry in place.
    let last_part = last_path_component(&combined);
    debug_assert_eq!(
        Some(last_part),
        get_path_components(&combined, "")
            .last()
            .map(String::as_str)
    );
    if last_part == "." || last_part == ".." {
        return ensure_trailing_directory_separator(&normalize_path(&combined));
    }
    normalize_path(&combined)
}

// PORT: the last entry of `get_path_components(path, "")` with no allocation.
// `path` must already have normalized slashes (`combine_paths` output), so the
// combine step in `get_path_components` returns it unchanged. Same rules as
// `path_components`: the root is the first entry, one trailing separator is
// removed, and a path that is only a root (or is empty) ends with the root.
fn last_path_component(path: &str) -> &str {
    let root_length = get_root_length(path);
    let rest = &path[root_length..];
    if rest.is_empty() {
        return &path[..root_length];
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    match memchr::memrchr(b'/', rest.as_bytes()) {
        Some(index) => &rest[index + 1..],
        None => rest,
    }
}

// Go: module/resolver.go:2288 matchesPatternWithTrailer
// PORT: compares the Go bytes of the port forms (see
// `scanner_util::GO_STRING_MARKER`).
pub fn matches_pattern_with_trailer(target: &str, name: &str) -> bool {
    if target.ends_with('*') {
        return false;
    }
    let Some((before, after)) = target.split_once('*') else {
        return false;
    };
    go_has_prefix(name, before) && go_has_suffix(name, after)
}

// Go: module/resolver.go:2063 extensionIsOk
/** True if `extension` is one of the supported `extensions`. */
pub fn extension_is_ok(extensions: Extensions, extension: &str) -> bool {
    extensions.intersects(Extensions::JAVA_SCRIPT)
        && (extension == EXTENSION_JS
            || extension == EXTENSION_JSX
            || extension == EXTENSION_MJS
            || extension == EXTENSION_CJS)
        || (extensions.intersects(Extensions::TYPE_SCRIPT)
            && (extension == EXTENSION_TS
                || extension == EXTENSION_TSX
                || extension == EXTENSION_MTS
                || extension == EXTENSION_CTS))
        || (extensions.intersects(Extensions::DECLARATION)
            && (extension == EXTENSION_DTS
                || extension == EXTENSION_DMTS
                || extension == EXTENSION_DCTS))
        || (extensions.intersects(Extensions::JSON) && extension == EXTENSION_JSON)
}

// Go: module/resolver.go:2320 ResolveConfig
pub fn resolve_config(
    module_name: &str,
    containing_file: &str,
    host: Rc<dyn ResolutionHost>,
) -> ResolvedModule {
    let resolver = new_resolver(ResolverOptions {
        host: Some(host),
        compiler_options: Some(Rc::new(CompilerOptions {
            module_resolution: ModuleResolutionKind::NODE_NEXT,
            ..Default::default()
        })),
        ..Default::default()
    });
    resolver.resolve_config(module_name, containing_file)
}

// Go: module/resolver.go:2328 GetAutomaticTypeDirectiveNames
// PORT: Go returns `[]string{}` for nil `Types`; `unwrap_or_default` does the same.
pub fn get_automatic_type_directive_names(
    options: &CompilerOptions,
    host: &dyn ResolutionHost,
) -> Vec<String> {
    if !options.uses_wildcard_types() {
        return options.types.clone().unwrap_or_default();
    }

    // Walk the primary type lookup locations
    let mut wildcard_matches: Vec<String> = Vec::new();
    let current_directory = host.get_current_directory();
    let (type_roots, _) = options.get_effective_type_roots(&current_directory);
    for root in &type_roots {
        if host.fs().directory_exists(root) {
            for type_directive_path in &host.fs().get_accessible_entries(root).directories {
                let normalized = normalize_path(type_directive_path);
                let package_json_path = combine_paths(root, &[&normalized, "package.json"]);
                let mut is_not_needed_package = false;
                if host.fs().file_exists(&package_json_path) {
                    let (contents, _) = host.fs().read_file(&package_json_path);
                    let package_json_content =
                        crate::frontend::packagejson::parse(&go_string_bytes(&contents))
                            .unwrap_or_default();
                    // `types-publisher` sometimes creates packages with `"typings": null` for packages that don't provide their own types.
                    // See `createNotNeededPackageJSON` in the types-publisher` repo.
                    is_not_needed_package = package_json_content.path_fields.typings.null;
                }
                if !is_not_needed_package {
                    let base_file_name = get_base_file_name(&normalized);
                    if !base_file_name.starts_with('.') {
                        wildcard_matches.push(base_file_name);
                    }
                }
            }
        }
    }

    // Order potentially matters in program construction, so substitute
    // in the wildcard in the position it was specified in the types array
    let mut result: Vec<String> = Vec::new();
    for t in options.types.iter().flatten() {
        if t == "*" {
            result.extend(wildcard_matches.iter().cloned());
        } else {
            result.push(t.clone());
        }
    }
    deduplicate(result)
}
