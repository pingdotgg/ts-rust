//! Port of modulespecifiers/specifiers.go.

use crate::prelude::*;

use super::compare::count_path_components;
use super::deps;
use super::packagejson::{ExportsOrImports, JSONValue, VersionPaths};
use super::preferences::{ModuleSpecifierPreferences, get_module_specifier_preferences};
use super::tspath;
use super::types::*;
use super::util::*;

// Go: modulespecifiers/specifiers.go:19 GetModuleSpecifiers
// PORT: `checker` is `&mut` because the Rust checker methods take `&mut self`.
#[allow(clippy::too_many_arguments)]
pub fn get_module_specifiers(
    module_symbol: SymbolId,
    checker: &mut dyn CheckerShape,
    compiler_options: &CompilerOptions,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    host: &dyn ModuleSpecifierGenerationHost,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
    for_auto_imports: bool,
) -> ModuleSpecifiersResult {
    get_module_specifiers_with_info(
        module_symbol,
        checker,
        compiler_options,
        importing_source_file,
        host,
        user_preferences,
        options,
        for_auto_imports,
    )
}

// Go: modulespecifiers/specifiers.go:41 GetModuleSpecifiersWithInfo
#[allow(clippy::too_many_arguments)]
pub fn get_module_specifiers_with_info(
    module_symbol: SymbolId,
    checker: &mut dyn CheckerShape,
    compiler_options: &CompilerOptions,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    host: &dyn ModuleSpecifierGenerationHost,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
    for_auto_imports: bool,
) -> ModuleSpecifiersResult {
    let ambient = try_get_module_name_from_ambient_module(module_symbol, checker);
    if !ambient.name.is_empty() {
        if for_auto_imports
            && is_excluded_by_regex(
                &ambient.name,
                &user_preferences.auto_import_specifier_exclude_regexes,
            )
        {
            return ModuleSpecifiersResult {
                kind: ResultKind::Ambient,
                ambient_module_symbol: ambient.symbol,
                ..Default::default()
            };
        }
        return ModuleSpecifiersResult {
            specifiers: vec![ambient.name],
            kind: ResultKind::Ambient,
            ambient_module_symbol: ambient.symbol,
        };
    }

    let module_source_file = get_source_file_of_module(checker.symbols(), module_symbol);
    if module_source_file.is_nil() {
        return ModuleSpecifiersResult::default();
    }

    // Use original source file name when file is from project reference output
    let module_file_name =
        host.get_source_of_project_reference_if_output_included(module_source_file);

    let (specifiers, kind) = get_module_specifiers_for_file_with_info(
        importing_source_file,
        &module_file_name,
        compiler_options,
        host,
        user_preferences,
        options,
        for_auto_imports,
    );
    ModuleSpecifiersResult {
        specifiers,
        kind,
        ..Default::default()
    }
}

// Go: modulespecifiers/specifiers.go:79 GetModuleSpecifiersForFileWithInfo
pub fn get_module_specifiers_for_file_with_info(
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    module_file_name: &str,
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
    for_auto_imports: bool,
) -> (Vec<String>, ResultKind) {
    let module_paths = get_all_module_paths_worker(
        &get_info(
            &host.get_source_of_project_reference_if_output_included(importing_source_file.node()),
            host,
        ),
        module_file_name,
        host,
        compiler_options,
        options,
    );

    compute_module_specifiers(
        &module_paths,
        compiler_options,
        importing_source_file,
        host,
        user_preferences,
        options,
        for_auto_imports,
    )
}

// Go: modulespecifiers/specifiers.go:107 ambientModuleInfo (ts#63931)
#[derive(Default)]
struct AmbientModuleInfo {
    name: String,
    symbol: SymbolId,
}

// Go: modulespecifiers/specifiers.go:112 tryGetModuleNameFromAmbientModule
fn try_get_module_name_from_ambient_module(
    module_symbol: SymbolId,
    checker: &mut dyn CheckerShape,
) -> AmbientModuleInfo {
    let declarations = checker.symbols().sym(module_symbol).declarations.clone();
    for decl in &declarations {
        if is_module_with_string_literal_name(*decl)
            && (!is_module_augmentation_external(*decl)
                || !tspath::is_external_module_name_relative(decl.name().text()))
        {
            return AmbientModuleInfo {
                name: decl.name().text().to_string(),
                symbol: module_symbol,
            };
        }
    }

    // the module could be a namespace, which is export through "export=" from an ambient module.
    /*
     * declare module "m" {
     *     namespace ns {
     *         class c {}
     *     }
     *     export = ns;
     * }
     */
    // `import {c} from "m";` is valid, in which case, `moduleSymbol` is "ns", but the module name should be "m"
    for d in declarations {
        if !is_module_declaration(d) {
            continue;
        }

        let possible_container = find_ancestor(d, is_module_with_string_literal_name);
        if possible_container.is_nil()
            || possible_container.parent().is_nil()
            || !is_source_file(possible_container.parent())
        {
            continue;
        }

        let symbols = checker.symbols();
        let sym = symbols.get(
            symbols.sym(possible_container.symbol()).exports,
            INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
        );
        if sym.is_nil() {
            continue;
        }
        let export_assignment_decl = symbols.sym(sym).value_declaration;
        if export_assignment_decl.is_nil()
            || export_assignment_decl.kind() != SyntaxKind::ExportAssignment
        {
            continue;
        }
        let mut export_symbol = checker.get_symbol_at_location(export_assignment_decl.expression());
        if export_symbol.is_nil() {
            continue;
        }
        if checker
            .symbols()
            .sym(export_symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
        {
            export_symbol = checker.get_aliased_symbol(export_symbol);
        }
        // TODO: Possible strada bug - isn't this insufficient in the presence of merge symbols?
        if export_symbol == d.symbol() {
            return AmbientModuleInfo {
                name: possible_container.name().text().to_string(),
                symbol: possible_container.symbol(),
            };
        }
    }
    AmbientModuleInfo::default()
}

// Go: modulespecifiers/specifiers.go:162 Info
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub use_case_sensitive_file_names: bool,
    pub importing_source_file_name: String,
    pub source_directory: String,
}

// Go: modulespecifiers/specifiers.go:168 getInfo
pub(crate) fn get_info(
    importing_source_file_name: &str,
    host: &dyn ModuleSpecifierGenerationHost,
) -> Info {
    let source_directory = tspath::get_directory_path(importing_source_file_name);
    Info {
        importing_source_file_name: importing_source_file_name.to_string(),
        source_directory,
        use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
    }
}

// Go: modulespecifiers/specifiers.go:179 getAllModulePaths
pub(crate) fn get_all_module_paths(
    info: &Info,
    imported_file_name: &str,
    host: &dyn ModuleSpecifierGenerationHost,
    compiler_options: &CompilerOptions,
    preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
) -> Vec<ModulePath> {
    // !!! use new cache model
    let _ = preferences;
    get_all_module_paths_worker(info, imported_file_name, host, compiler_options, options)
}

// Go: modulespecifiers/specifiers.go:202 getAllModulePathsWorker
// PORT: Go collects paths in a map with random iteration order and sorts
// each group. The IndexMap keeps insertion order; the sort result is the
// same because `compare_paths_by_redirect` ends with a path comparison.
fn get_all_module_paths_worker(
    info: &Info,
    imported_file_name: &str,
    host: &dyn ModuleSpecifierGenerationHost,
    compiler_options: &CompilerOptions,
    options: ModuleSpecifierOptions,
) -> Vec<ModulePath> {
    let _ = (compiler_options, options);
    let mut all_file_names: IndexMap<String, ModulePath> = IndexMap::default();
    let paths = get_each_file_name_of_module(
        &info.importing_source_file_name,
        imported_file_name,
        host,
        true,
    );
    for p in &paths {
        all_file_names.insert(p.file_name.clone(), p.clone());
    }

    let use_case_sensitive_file_names = info.use_case_sensitive_file_names;
    let compare_paths = |a: &ModulePath, b: &ModulePath| {
        compare_paths_by_redirect(a, b, use_case_sensitive_file_names)
    };

    // Sort by paths closest to importing file Name directory
    let mut sorted_paths: Vec<ModulePath> = Vec::with_capacity(paths.len());
    let mut directory = info.source_directory.clone();
    while !all_file_names.is_empty() {
        let mut paths_in_directory: Vec<ModulePath> = Vec::new();
        all_file_names.retain(|file_name, p| {
            // ts#64159 (specifiers.go:225): CaseSensitivity.StartsWithDirectory,
            // without case on a case-insensitive file system (N: a
            // case-sensitive text prefix).
            if tspath::relative_path_within_directory(
                &directory,
                file_name,
                use_case_sensitive_file_names,
            )
            .is_some_and(|relative| !relative.is_empty())
            {
                paths_in_directory.push(p.clone());
                return false;
            }
            true
        });
        if !paths_in_directory.is_empty() {
            // Go: modulespecifiers/specifiers.go:228 slices.SortFunc(pathsInDirectory, comparePaths)
            crate::gostd::slices::sort_func(&mut paths_in_directory, compare_paths);
            sorted_paths.extend(paths_in_directory);
        }
        let new_directory = tspath::get_directory_path(&directory);
        if new_directory == directory {
            break;
        }
        directory = new_directory;
    }
    if !all_file_names.is_empty() {
        let mut remaining_paths: Vec<ModulePath> = all_file_names.into_values().collect();
        // Go: modulespecifiers/specifiers.go:239 slices.SortFunc(remainingPaths, comparePaths)
        crate::gostd::slices::sort_func(&mut remaining_paths, compare_paths);
        sorted_paths.extend(remaining_paths);
    }
    sorted_paths
}

// Go: modulespecifiers/specifiers.go:250 containsIgnoredPath
// containsIgnoredPath checks if a path contains patterns that should be ignored.
// This is a local helper that duplicates tspath.ContainsIgnoredPath for performance.
fn contains_ignored_path(s: &str) -> bool {
    s.contains("/node_modules/.") || s.contains("/.git") || s.contains(".#")
}

// Go: modulespecifiers/specifiers.go:259 ContainsNodeModules (at 673a5f17d713;
// ts#64159 renames it moduleSpecifierContainsNodeModules, modulespecifiers/specifiers.go:256)
/// Checks if a path contains the node_modules directory.
pub fn contains_node_modules(s: &str) -> bool {
    s.contains("/node_modules/")
}

// Go: modulespecifiers/specifiers.go:262 GetEachFileNameOfModule
/// Returns all possible file paths for a module, including symlink alternatives.
pub fn get_each_file_name_of_module(
    importing_file_name: &str,
    imported_file_name: &str,
    host: &dyn ModuleSpecifierGenerationHost,
    prefer_symlinks: bool,
) -> Vec<ModulePath> {
    let cwd = host.get_current_directory();
    let imported_path = tspath::to_path(
        imported_file_name,
        &cwd,
        host.use_case_sensitive_file_names(),
    );
    let mut reference_redirect = String::new();
    let output_and_reference = host.get_project_reference_from_source(&imported_path);
    if let Some(output_and_reference) = output_and_reference {
        if !output_and_reference.output_dts.is_empty() {
            reference_redirect = output_and_reference.output_dts.clone();
        }
    }

    let redirects = host.get_redirect_targets(&imported_path);
    let mut imported_file_names: Vec<String> = Vec::with_capacity(2 + redirects.len());
    if !reference_redirect.is_empty() {
        imported_file_names.push(reference_redirect.clone());
    }
    imported_file_names.push(imported_file_name.to_string());
    imported_file_names.extend(redirects);
    let targets: Vec<String> = imported_file_names
        .iter()
        .map(|f| tspath::get_normalized_absolute_path(f, &cwd))
        .collect();
    let mut should_filter_ignored_paths = !targets.iter().all(|t| contains_ignored_path(t));

    let mut results: Vec<ModulePath> = Vec::with_capacity(2);
    if !prefer_symlinks {
        for p in &targets {
            if !(should_filter_ignored_paths && contains_ignored_path(p)) {
                results.push(ModulePath {
                    file_name: p.clone(),
                    is_in_node_modules: contains_node_modules(p),
                    is_redirect: reference_redirect == *p,
                });
            }
        }
    }

    let symlink_cache = host.get_symlink_cache();
    let full_imported_file_name = tspath::get_normalized_absolute_path(imported_file_name, &cwd);
    if let Some(symlink_cache) = symlink_cache {
        tspath::for_each_ancestor_directory_stopping_at_global_cache(
            &host.get_global_typings_cache_location(),
            &tspath::get_directory_path(&full_imported_file_name),
            |real_path_directory| -> (bool, bool) {
                let key = tspath::to_path(
                    real_path_directory,
                    &cwd,
                    host.use_case_sensitive_file_names(),
                )
                .ensure_trailing_directory_separator();
                let Some(symlink_set) = symlink_cache.directories_by_realpath().get(&key) else {
                    return (false, false);
                }; // Continue to ancestor directory

                // Don't want to a package to globally import from itself (importNameCodeFix_symlink_own_package.ts)
                if tspath::starts_with_directory(
                    importing_file_name,
                    real_path_directory,
                    host.use_case_sensitive_file_names(),
                ) {
                    return (false, true); // Stop search, each ancestor directory will also hit this condition
                }

                for target in &targets {
                    if !tspath::starts_with_directory(
                        target,
                        real_path_directory,
                        host.use_case_sensitive_file_names(),
                    ) {
                        continue;
                    }

                    let relative = tspath::get_relative_path_from_directory(
                        real_path_directory,
                        target,
                        &tspath::ComparePathsOptions {
                            use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
                            current_directory: cwd.clone(),
                        },
                    );
                    for symlink_directory in symlink_set {
                        let option = tspath::resolve_path(symlink_directory, &[&relative]);
                        results.push(ModulePath {
                            is_in_node_modules: contains_node_modules(&option),
                            is_redirect: *target == reference_redirect,
                            file_name: option,
                        });
                        should_filter_ignored_paths = true; // We found a non-ignored path in symlinks, so we can reject ignored-path realpaths
                    }
                }

                (false, false)
            },
        );
    }

    if prefer_symlinks {
        for p in &targets {
            if !(should_filter_ignored_paths && contains_ignored_path(p)) {
                results.push(ModulePath {
                    file_name: p.clone(),
                    is_in_node_modules: contains_node_modules(p),
                    is_redirect: reference_redirect == *p,
                });
            }
        }
    }

    results
}

// Go: modulespecifiers/specifiers.go:352 computeModuleSpecifiers
fn compute_module_specifiers(
    module_paths: &[ModulePath],
    compiler_options: &CompilerOptions,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    host: &dyn ModuleSpecifierGenerationHost,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
    for_auto_import: bool,
) -> (Vec<String>, ResultKind) {
    let info = get_info(&importing_source_file.file_name(), host);
    let preferences = get_module_specifier_preferences(
        user_preferences,
        host,
        compiler_options,
        importing_source_file,
        "",
    );

    let mut existing_specifier = String::new();
    let imports = importing_source_file.imports();
    for module_path in module_paths {
        let target_path = tspath::to_path(
            &module_path.file_name,
            &host.get_current_directory(),
            info.use_case_sensitive_file_names,
        );
        let mut existing_import = Node::NIL;
        for import_specifier in &imports {
            let resolved_module = host.get_resolved_module_from_module_specifier(
                importing_source_file.node(),
                *import_specifier,
            );
            if let Some(resolved_module) = resolved_module.filter(|r| r.is_resolved()) {
                if tspath::to_path(
                    &resolved_module.resolved_file_name,
                    &host.get_current_directory(),
                    info.use_case_sensitive_file_names,
                ) == target_path
                {
                    existing_import = *import_specifier;
                    break;
                }
            }
        }
        if existing_import.is_some() {
            if preferences.relative_preference == RelativePreferenceKind::NonRelative
                && tspath::path_is_relative(existing_import.text())
            {
                // If the preference is for non-relative and the module specifier is relative, ignore it
                continue;
            }
            let existing_mode =
                host.get_mode_for_usage_location(importing_source_file.node(), existing_import);
            let mut target_mode = options.override_import_mode;
            if target_mode == RESOLUTION_MODE_NONE {
                target_mode =
                    host.get_default_resolution_mode_for_file(importing_source_file.node());
            }
            if existing_mode != target_mode
                && existing_mode != RESOLUTION_MODE_NONE
                && target_mode != RESOLUTION_MODE_NONE
            {
                // If the candidate import mode doesn't match the mode we're generating for, don't consider it
                continue;
            }
            existing_specifier = existing_import.text().to_string();
            break;
        }
    }

    if !existing_specifier.is_empty() {
        return (vec![existing_specifier], ResultKind::None);
    }

    let imported_file_is_in_node_modules = module_paths.iter().any(|p| p.is_in_node_modules);

    // Module specifier priority:
    //   1. "Bare package specifiers" (e.g. "@foo/bar") resulting from a path through node_modules to a package.json's "types" entry
    //   2. Specifiers generated using "paths" from tsconfig
    //   3. Non-relative specfiers resulting from a path through node_modules (e.g. "@foo/bar/path/to/file")
    //   4. Relative paths
    let mut paths_specifiers: Vec<String> = Vec::new();
    let mut redirect_paths_specifiers: Vec<String> = Vec::new();
    let mut node_modules_specifiers: Vec<String> = Vec::new();
    let mut relative_specifiers: Vec<String> = Vec::new();

    for module_path in module_paths {
        let mut specifier = String::new();
        if module_path.is_in_node_modules {
            specifier = try_get_module_name_as_node_module(
                module_path,
                &info,
                importing_source_file,
                host,
                compiler_options,
                user_preferences,
                /*packageNameOnly*/ false,
                options.override_import_mode,
            );
        }
        if !specifier.is_empty()
            && !(for_auto_import && is_excluded_by_regex(&specifier, &preferences.exclude_regexes))
        {
            node_modules_specifiers.push(specifier.clone());
            if module_path.is_redirect {
                // If we got a specifier for a redirect, it was a bare package specifier (e.g. "@foo/bar",
                // not "@foo/bar/path/to/file"). No other specifier will be this good, so stop looking.
                return (node_modules_specifiers, ResultKind::NodeModules);
            }
        }

        let mut import_mode = options.override_import_mode;
        if import_mode == RESOLUTION_MODE_NONE {
            import_mode = host.get_default_resolution_mode_for_file(importing_source_file.node());
        }
        let local = get_local_module_specifier(
            &module_path.file_name,
            &info,
            compiler_options,
            host,
            import_mode,
            &preferences,
            /*pathsOnly*/ module_path.is_redirect || !specifier.is_empty(),
        );
        if local.is_empty()
            || for_auto_import && is_excluded_by_regex(&local, &preferences.exclude_regexes)
        {
            continue;
        }
        if module_path.is_redirect {
            redirect_paths_specifiers.push(local);
        } else if path_is_bare_specifier(&local) {
            if contains_node_modules(&local) {
                // We could be in this branch due to inappropriate use of `baseUrl`, not intentional `paths`
                // usage. It's impossible to reason about where to prioritize baseUrl-generated module
                // specifiers, but if they contain `/node_modules/`, they're going to trigger a portability
                // error, so *at least* don't prioritize those.
                relative_specifiers.push(local);
            } else {
                paths_specifiers.push(local);
            }
        } else if for_auto_import
            || !imported_file_is_in_node_modules
            || module_path.is_in_node_modules
        {
            // Why this extra conditional, not just an `else`? If some path to the file contained
            // 'node_modules', but we can't create a non-relative specifier (e.g. "@foo/bar/path/to/file"),
            // that means we had to go through a *sibling's* node_modules, not one we can access directly.
            // If some path to the file was in node_modules but another was not, this likely indicates that
            // we have a monorepo structure with symlinks. In this case, the non-nodeModules path is
            // probably the realpath, e.g. "../bar/path/to/file", but a relative path to another package
            // in a monorepo is probably not portable. So, the module specifier we actually go with will be
            // the relative path through node_modules, so that the declaration emitter can produce a
            // portability error. (See declarationEmitReexportedSymlinkReference3)
            relative_specifiers.push(local);
        }
    }

    if !paths_specifiers.is_empty() {
        return (paths_specifiers, ResultKind::Paths);
    }
    if !redirect_paths_specifiers.is_empty() {
        return (redirect_paths_specifiers, ResultKind::Redirect);
    }
    if !node_modules_specifiers.is_empty() {
        return (node_modules_specifiers, ResultKind::NodeModules);
    }
    (relative_specifiers, ResultKind::Relative)
}

// Go: modulespecifiers/specifiers.go:479 getLocalModuleSpecifier
fn get_local_module_specifier(
    module_file_name: &str,
    info: &Info,
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    import_mode: ResolutionMode,
    preferences: &ModuleSpecifierPreferences<'_>,
    paths_only: bool,
) -> String {
    let paths = compiler_options.paths.as_ref();
    let root_dirs = compiler_options.root_dirs.as_deref().unwrap_or_default();

    if paths_only && paths.is_none() {
        return String::new();
    }

    let source_directory = info.source_directory.as_str();

    let allowed_endings = (preferences.get_allowed_endings_in_preferred_order)(import_mode);
    let mut relative_path = String::new();
    if !root_dirs.is_empty() {
        relative_path = try_get_module_name_from_root_dirs(
            root_dirs,
            module_file_name,
            source_directory,
            &allowed_endings,
            compiler_options,
            host,
        );
    }
    if relative_path.is_empty() {
        relative_path = process_ending(
            &ensure_path_is_non_module_name(&tspath::get_relative_path_from_directory(
                source_directory,
                module_file_name,
                &tspath::ComparePathsOptions {
                    use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
                    current_directory: host.get_current_directory(),
                },
            )),
            module_file_name,
            &allowed_endings,
            compiler_options,
            host,
        );
    }

    if (paths.is_none() && !compiler_options.get_resolve_package_json_imports())
        || preferences.relative_preference == RelativePreferenceKind::Relative
    {
        if paths_only {
            return String::new();
        }
        return relative_path;
    }

    // ts#64159 (specifiers.go:524, R1): the host's base directory, not its
    // current directory.
    let host_base_directory = host.base_directory();
    let root = compiler_options.get_paths_base_path(&host_base_directory);
    let base_directory = tspath::get_normalized_absolute_path(&root, &host_base_directory);
    let relative_to_base_url = get_relative_path_if_in_same_volume(
        module_file_name,
        &base_directory,
        host.use_case_sensitive_file_names(),
    );
    if relative_to_base_url.is_empty() {
        if paths_only {
            return String::new();
        }
        return relative_path;
    }

    let mut from_package_json_imports = String::new();
    if !paths_only {
        from_package_json_imports = try_get_module_name_from_package_json_imports(
            module_file_name,
            source_directory,
            compiler_options,
            host,
            import_mode,
            prefers_ts_extension(&allowed_endings),
        );
    }

    let mut from_paths = String::new();
    if paths_only || from_package_json_imports.is_empty() {
        if let Some(paths) = paths {
            from_paths = try_get_module_name_from_paths(
                &relative_to_base_url,
                module_file_name,
                paths,
                &allowed_endings,
                &base_directory,
                host,
                compiler_options,
            );
        }
    }

    if paths_only {
        return from_paths;
    }

    let maybe_non_relative = if !from_package_json_imports.is_empty() {
        from_package_json_imports
    } else {
        from_paths
    };
    if maybe_non_relative.is_empty() {
        return relative_path;
    }

    let relative_is_excluded = is_excluded_by_regex(&relative_path, &preferences.exclude_regexes);
    let non_relative_is_excluded =
        is_excluded_by_regex(&maybe_non_relative, &preferences.exclude_regexes);
    if !relative_is_excluded && non_relative_is_excluded {
        return relative_path;
    }
    if relative_is_excluded && !non_relative_is_excluded {
        return maybe_non_relative;
    }

    if preferences.relative_preference == RelativePreferenceKind::NonRelative
        && !tspath::path_is_relative(&maybe_non_relative)
    {
        return maybe_non_relative;
    }

    if preferences.relative_preference == RelativePreferenceKind::ExternalNonRelative
        && !tspath::path_is_relative(&maybe_non_relative)
    {
        let cwd = host.get_current_directory();
        let case = host.use_case_sensitive_file_names();
        let project_directory = if !compiler_options.config_file_path.is_empty() {
            tspath::to_path(
                &tspath::get_directory_path(&compiler_options.config_file_path),
                &cwd,
                case,
            )
        } else {
            // ts#64159 (specifiers.go:593, R1): the host's base directory.
            tspath::to_path(&host.base_directory(), &cwd, case)
        };
        let canonical_source_directory = tspath::to_path(source_directory, &cwd, case);
        let module_path = tspath::to_path(module_file_name, &project_directory, case);

        let source_is_internal = project_directory.contains_path(&canonical_source_directory);
        let target_is_internal = project_directory.contains_path(&module_path);
        if source_is_internal && !target_is_internal || !source_is_internal && target_is_internal {
            // 1. The import path crosses the boundary of the tsconfig.json-containing directory.
            //
            //      src/
            //        tsconfig.json
            //        index.ts -------
            //      lib/              | (path crosses tsconfig.json)
            //        imported.ts <---
            //
            return maybe_non_relative;
        }

        let nearest_target_package_json = host.get_nearest_ancestor_directory_with_package_json(
            &tspath::get_directory_path(&module_path),
        );
        let nearest_source_package_json =
            host.get_nearest_ancestor_directory_with_package_json(source_directory);

        if !package_json_paths_are_equal(
            &nearest_target_package_json,
            &nearest_source_package_json,
            &tspath::ComparePathsOptions {
                use_case_sensitive_file_names: case,
                current_directory: cwd.clone(),
            },
        ) {
            // 2. The importing and imported files are part of different packages.
            //
            //      packages/a/
            //        package.json
            //        index.ts --------
            //      packages/b/        | (path crosses package.json)
            //        package.json     |
            //        component.ts <---
            //
            return maybe_non_relative;
        }

        return relative_path;
    }

    // Prefer a relative import over a baseUrl import if it has fewer components.
    // Go: modulespecifiers/specifiers.go:633 (at 673a5f17d713 :635). ts#64159
    // keeps `strings.HasPrefix(maybeNonRelative, "..")` here: a `paths`
    // result such as "..lib/thing" also prefers the relative path. Only
    // isPathRelativeToParent (util.go:214) needs a ".." segment.
    if maybe_non_relative.starts_with("..")
        || count_path_components(&relative_path) < count_path_components(&maybe_non_relative)
    {
        return relative_path;
    }
    maybe_non_relative
}

// Go: modulespecifiers/specifiers.go:639 processEnding
// ts#64159: `source_file_name` is the target file (Go `sourceFileName`).
// PORT: Go checks `host != nil` before probing the file system. Every Go
// caller passes a host, so `host` is not optional here.
fn process_ending(
    file_name: &str,
    source_file_name: &str,
    allowed_endings: &[ModuleSpecifierEnding],
    options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
) -> String {
    if tspath::file_extension_is_one_of(
        file_name,
        &[
            tspath::EXTENSION_JSON,
            tspath::EXTENSION_MJS,
            tspath::EXTENSION_CJS,
        ],
    ) {
        return file_name.to_string();
    }

    let no_extension = tspath::remove_file_extension(file_name);
    if file_name == no_extension {
        return file_name.to_string();
    }

    let js_priority = index_of(allowed_endings, ModuleSpecifierEnding::JsExtension);
    let ts_priority = index_of(allowed_endings, ModuleSpecifierEnding::TsExtension);
    if tspath::file_extension_is_one_of(file_name, &[tspath::EXTENSION_MTS, tspath::EXTENSION_CTS])
        && ts_priority != -1
        && ts_priority < js_priority
    {
        return file_name.to_string();
    }
    if tspath::file_extension_is_one_of(
        file_name,
        &[tspath::EXTENSION_DMTS, tspath::EXTENSION_DCTS],
    ) {
        let input_ext = tspath::get_declaration_file_extension(file_name);
        let ext = get_js_extension_for_declaration_file_extension(&input_ext);
        return format!("{}{}", tspath::remove_extension(file_name, &input_ext), ext);
    }
    if tspath::file_extension_is_one_of(file_name, &[tspath::EXTENSION_MTS, tspath::EXTENSION_CTS])
    {
        return format!(
            "{}{}",
            no_extension,
            get_js_extension_for_file(file_name, options)
        );
    }
    if !tspath::file_extension_is_one_of(file_name, &[tspath::EXTENSION_DTS])
        && tspath::file_extension_is_one_of(file_name, &[tspath::EXTENSION_TS])
        && file_name.contains(".d.")
    {
        // `foo.d.json.ts` and the like - remap back to `foo.json`
        let result = try_get_real_file_name_for_non_js_declaration_file_name(file_name);
        if !result.is_empty() {
            return result;
        }
    }

    match allowed_endings[0] {
        ModuleSpecifierEnding::Minimal => {
            let without_index = no_extension.strip_suffix("/index").unwrap_or(no_extension);
            // ts#64159 (specifiers.go:679): the file next to the index file is
            // looked up from the target file's directory (`sourceFileName`), not
            // from the specifier resolved against the current directory.
            if without_index != no_extension
                && try_get_any_file_from_path(host, &tspath::get_directory_path(source_file_name))
            {
                // Can't remove index if there's a file by the same name as the directory.
                // Probably more callers should pass `host` so we can determine this?
                return no_extension.to_string();
            }
            without_index.to_string()
        }
        ModuleSpecifierEnding::Index => no_extension.to_string(),
        ModuleSpecifierEnding::JsExtension => {
            format!(
                "{}{}",
                no_extension,
                get_js_extension_for_file(file_name, options)
            )
        }
        ModuleSpecifierEnding::TsExtension => {
            // For now, we don't know if this import is going to be type-only, which means we don't
            // know if a .d.ts extension is valid, so use no extension or a .js extension
            if tspath::is_declaration_file_name(file_name) {
                let mut extensionless_priority: isize = -1;
                for (i, e) in allowed_endings.iter().enumerate() {
                    if *e == ModuleSpecifierEnding::Minimal || *e == ModuleSpecifierEnding::Index {
                        extensionless_priority = i as isize;
                        break;
                    }
                }
                if extensionless_priority != -1 && extensionless_priority < js_priority {
                    return no_extension.to_string();
                }
                return format!(
                    "{}{}",
                    no_extension,
                    get_js_extension_for_file(file_name, options)
                );
            }
            file_name.to_string()
        }
    }
}

// Go: modulespecifiers/specifiers.go:712 tryGetModuleNameFromRootDirs
fn try_get_module_name_from_root_dirs(
    root_dirs: &[String],
    module_file_name: &str,
    source_directory: &str,
    allowed_endings: &[ModuleSpecifierEnding],
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
) -> String {
    let normalized_target_paths = get_paths_relative_to_root_dirs(
        module_file_name,
        root_dirs,
        host.use_case_sensitive_file_names(),
    );
    if normalized_target_paths.is_empty() {
        return String::new();
    }

    let normalized_source_paths = get_paths_relative_to_root_dirs(
        source_directory,
        root_dirs,
        host.use_case_sensitive_file_names(),
    );
    let mut shortest = String::new();
    let mut shortest_sep_count = 0usize;
    for source_path in &normalized_source_paths {
        for target_path in &normalized_target_paths {
            let candidate =
                ensure_path_is_non_module_name(&tspath::get_relative_path_from_directory(
                    source_path,
                    target_path,
                    &tspath::ComparePathsOptions {
                        use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
                        current_directory: host.get_current_directory(),
                    },
                ));
            let candidate_sep_count = candidate.matches('/').count();
            if shortest.is_empty() || candidate_sep_count < shortest_sep_count {
                shortest = candidate;
                shortest_sep_count = candidate_sep_count;
            }
        }
    }

    if shortest.is_empty() {
        return String::new();
    }
    process_ending(
        &shortest,
        module_file_name,
        allowed_endings,
        compiler_options,
        host,
    )
}

// Go: modulespecifiers/specifiers.go:745 tryGetModuleNameAsNodeModule
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_get_module_name_as_node_module(
    path_obj: &ModulePath,
    info: &Info,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    host: &dyn ModuleSpecifierGenerationHost,
    options: &CompilerOptions,
    user_preferences: &UserPreferences,
    package_name_only: bool,
    override_mode: ResolutionMode,
) -> String {
    let Some(parts) = get_node_module_path_parts(&path_obj.file_name) else {
        return String::new();
    };

    // Simplify the full file path to something that can be resolved by Node.
    let preferences = get_module_specifier_preferences(
        user_preferences,
        host,
        options,
        importing_source_file,
        "",
    );
    let allowed_endings =
        (preferences.get_allowed_endings_in_preferred_order)(RESOLUTION_MODE_NONE);

    let case_sensitive = host.use_case_sensitive_file_names();
    let mut module_specifier = path_obj.file_name.clone();
    let mut is_package_root_path = false;
    // ts#64159 (specifiers.go:768): a file directly in node_modules
    // (`parts.IsDirectNodeModulesFile`, no package root) only gets its
    // ending processed (N tried package.json files at every "/" of the path).
    if !package_name_only && parts.package_root_index == -1 {
        module_specifier = process_ending(
            &path_obj.file_name,
            &path_obj.file_name,
            &allowed_endings,
            options,
            host,
        );
    } else if !package_name_only {
        let mut package_root_index = parts.package_root_index;
        let mut module_file_name = String::new();
        loop {
            // ts#64544: each pass tries the directory at the current
            // `packageRootIndex`.
            let mut current_parts = parts;
            current_parts.package_root_index = package_root_index;
            // If the module could be imported by a directory name, use that directory's name
            let pkg_json_results = try_directory_with_package_json(
                current_parts,
                parts.package_root_index,
                path_obj,
                importing_source_file,
                host,
                override_mode,
                options,
                &allowed_endings,
            );
            let module_file_to_try = pkg_json_results.module_file_to_try;
            let package_root_path = pkg_json_results.package_root_path;
            let blocked_by_exports = pkg_json_results.blocked_by_exports;
            let verbatim_from_exports = pkg_json_results.verbatim_from_exports;
            if blocked_by_exports {
                return String::new(); // File is under this package.json, but is not publicly exported - there's no way to name it via `node_modules` resolution
            }
            if verbatim_from_exports {
                return module_file_to_try;
            }
            if !package_root_path.is_empty() {
                module_specifier = package_root_path;
                is_package_root_path = true;
                break;
            }
            if module_file_name.is_empty() {
                module_file_name = module_file_to_try;
            }
            // try with next level of directory
            package_root_index =
                deps::index_after(&path_obj.file_name, "/", (package_root_index + 1) as usize);
            if package_root_index == -1 {
                module_specifier = process_ending(
                    &module_file_name,
                    &module_file_name,
                    &allowed_endings,
                    options,
                    host,
                );
                break;
            }
        }
    }

    if path_obj.is_redirect && !is_package_root_path {
        return String::new();
    }

    let global_typings_cache_location = host.get_global_typings_cache_location();
    // Get a path that's relative to node_modules or the importing file's path
    // if node_modules folder is in this folder or any of its parent folders, no need to keep it.
    // ts#64159 (specifiers.go:825): the directory that holds the top-level
    // node_modules (`parts.TopLevelNodeModulesSearchRoot`, a root at least)
    // must contain the importing directory as a path; N tested a string
    // prefix, so "/a" matched "/ab/src".
    let search_root_end = (parts.top_level_node_modules_index as usize)
        .max(tspath::get_root_length(&path_obj.file_name));
    let search_root = &path_obj.file_name[..search_root_end];
    let compare_options = tspath::ComparePathsOptions {
        use_case_sensitive_file_names: case_sensitive,
        current_directory: String::new(),
    };
    if !tspath::contains_path(search_root, &info.source_directory, &compare_options)
        || !global_typings_cache_location.is_empty()
            && tspath::contains_path(
                search_root,
                &global_typings_cache_location,
                &compare_options,
            )
    {
        return String::new();
    }

    // If the module was found in @types, get the actual Node package name
    // ts#64159 (specifiers.go:831): a specifier outside the top-level
    // node_modules directory gives no name.
    let node_modules_prefix =
        &path_obj.file_name[..(parts.top_level_package_name_index + 1) as usize];
    let Some(node_modules_directory_name) = module_specifier.strip_prefix(node_modules_prefix)
    else {
        return String::new();
    };
    deps::get_package_name_from_types_package_name(node_modules_directory_name)
}

// Go: modulespecifiers/specifiers.go:839 pkgJsonDirAttemptResult
#[derive(Clone, Debug, Default)]
struct PkgJsonDirAttemptResult {
    module_file_to_try: String,
    package_root_path: String,
    blocked_by_exports: bool,
    verbatim_from_exports: bool,
}

// Go: modulespecifiers/specifiers.go:846 tryDirectoryWithPackageJson
// ts#64544: `package_base_root_index` is the package root of the module
// (`parts.PackageRootIndex` before the loop), for the index file name check.
fn try_directory_with_package_json(
    parts: NodeModulePathParts,
    package_base_root_index: isize,
    path_obj: &ModulePath,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    host: &dyn ModuleSpecifierGenerationHost,
    override_mode: ResolutionMode,
    options: &CompilerOptions,
    allowed_endings: &[ModuleSpecifierEnding],
) -> PkgJsonDirAttemptResult {
    let mut root_idx = parts.package_root_index;
    if root_idx == -1 {
        root_idx = path_obj.file_name.len() as isize; // TODO: possible strada bug? -1 in js slice removes characters from the end, in go it panics - js behavior seems unwanted here?
    }
    let package_root_path = path_obj.file_name[0..root_idx as usize].to_string();
    let package_json_path = tspath::combine_paths(&package_root_path, &["package.json"]);
    let mut module_file_to_try = path_obj.file_name.clone();
    let mut maybe_blocked_by_types_versions = false;
    let Some(package_json) = host.get_package_json_info(&package_json_path) else {
        // No package.json exists; an index.js will still resolve as the package name
        let file_name = &module_file_to_try[(package_base_root_index + 1) as usize..];
        if file_name == "index.d.ts"
            || file_name == "index.js"
            || file_name == "index.ts"
            || file_name == "index.tsx"
        {
            return PkgJsonDirAttemptResult {
                module_file_to_try,
                package_root_path,
                ..Default::default()
            };
        }
        return PkgJsonDirAttemptResult {
            module_file_to_try,
            ..Default::default()
        };
    };

    let mut import_mode = override_mode;
    if import_mode == RESOLUTION_MODE_NONE {
        import_mode = host.get_default_resolution_mode_for_file(importing_source_file.node());
    }

    let package_json_content = package_json.get_contents();
    if options.get_resolve_package_json_exports() {
        // The package name that we found in node_modules could be different from the package
        // name in the package.json content via url/filepath dependency specifiers. We need to
        // use the actual directory name, so don't look at `packageJsonContent.name` here.
        let node_modules_directory_name =
            &package_root_path[(parts.top_level_package_name_index + 1) as usize..];
        let package_name =
            deps::get_package_name_from_types_package_name(node_modules_directory_name);

        // Determine resolution mode for package.json exports condition matching.
        // TypeScript's tryDirectoryWithPackageJson uses the importing file's mode (moduleSpecifiers.ts:1257),
        // but this causes incorrect exports resolution. We fix this by checking the target file's extension
        // using the logic from getImpliedNodeFormatForEmitWorker (program.ts:4827-4838).
        // .cjs/.cts/.d.cts → CommonJS → "require" condition
        // .mjs/.mts/.d.mts → ESM → "import" condition
        if tspath::file_extension_is_one_of(
            &path_obj.file_name,
            &[
                tspath::EXTENSION_CJS,
                tspath::EXTENSION_CTS,
                tspath::EXTENSION_DCTS,
            ],
        ) {
            import_mode = RESOLUTION_MODE_COMMON_JS;
        } else if tspath::file_extension_is_one_of(
            &path_obj.file_name,
            &[
                tspath::EXTENSION_MJS,
                tspath::EXTENSION_MTS,
                tspath::EXTENSION_DMTS,
            ],
        ) {
            import_mode = RESOLUTION_MODE_ESM;
        }

        let conditions = deps::get_conditions(options, import_mode);

        let mut from_exports = String::new();
        if let Some(content) = package_json_content.filter(|c| c.fields.exports.is_present()) {
            from_exports = try_get_module_name_from_exports(
                options,
                host,
                &path_obj.file_name,
                &package_root_path,
                &package_name,
                &content.fields.exports,
                &conditions,
            );
        }
        if !from_exports.is_empty() {
            return PkgJsonDirAttemptResult {
                module_file_to_try: from_exports,
                verbatim_from_exports: true,
                ..Default::default()
            };
        }
        if package_json_content.is_some_and(|c| c.fields.exports.is_present()) {
            return PkgJsonDirAttemptResult {
                module_file_to_try: path_obj.file_name.clone(),
                blocked_by_exports: true,
                ..Default::default()
            };
        }
    }

    let mut version_paths = VersionPaths::default();
    if let Some(content) = package_json_content {
        if matches!(content.fields.types_versions, JSONValue::Object(_)) {
            version_paths = content.get_version_paths().clone();
        }
    }
    let version_paths_paths = version_paths.get_paths();
    if let Some(paths) = &version_paths_paths {
        let sub_module_name = &path_obj.file_name[package_root_path.len() + 1..];
        let from_paths = try_get_module_name_from_paths(
            sub_module_name,
            &module_file_to_try,
            paths,
            allowed_endings,
            &package_root_path,
            host,
            options,
        );
        if from_paths.is_empty() {
            maybe_blocked_by_types_versions = true;
        } else {
            // ts#64159 (specifiers.go:947): Go resolves the file
            // (`packageRootDirectory.ResolveFile(fromPaths)`), so "." and ".."
            // segments are reduced (N: CombinePaths).
            module_file_to_try = tspath::resolve_path_without_trailing_directory_separator(
                &package_root_path,
                &[&from_paths],
            );
        }
    }
    // If the file is the main module, it can be imported by the package name
    let mut main_file_relative = "index.js".to_string();
    if let Some(content) = package_json_content {
        if content.fields.typings.valid {
            main_file_relative = content.fields.typings.value.clone();
        } else if content.fields.types.valid {
            main_file_relative = content.fields.types.value.clone();
        } else if content.fields.main.valid {
            main_file_relative = content.fields.main.value.clone();
        }
    }

    if !main_file_relative.is_empty()
        && !(maybe_blocked_by_types_versions
            && deps::match_pattern_or_exact(
                &deps::try_parse_patterns(version_paths_paths.as_ref()),
                &main_file_relative,
            ) != deps::Pattern::default())
    {
        // The 'main' file is also subject to mapping through typesVersions, and we couldn't come up with a path
        // explicitly through typesVersions, so if it matches a key in typesVersions now, it's not reachable.
        // (The only way this can happen is if some file in a package that's not resolvable from outside the
        // package got pulled into the program anyway, e.g. transitively through a file that *is* reachable. It
        // happens very easily in fourslash tests though, since every test file listed gets included. See
        // importNameCodeFix_typesVersions.ts for an example.)
        let package_type = package_json_content.map_or("", |c| c.fields.type_.value.as_str());
        if is_package_main_file(
            &module_file_to_try,
            &package_root_path,
            &main_file_relative,
            package_type,
            host.use_case_sensitive_file_names(),
        ) {
            return PkgJsonDirAttemptResult {
                package_root_path,
                module_file_to_try,
                ..Default::default()
            };
        }
    }

    PkgJsonDirAttemptResult {
        module_file_to_try,
        ..Default::default()
    }
}

// Go: modulespecifiers/specifiers.go:982 isPackageMainFile (ts#64159)
// ts#64159 takes this check out of tryDirectoryWithPackageJson and changes it:
// - a main entry with directory intent ("./types/") does not name the sibling
//   file "types.d.ts";
// - a nil package.json content no longer makes every file the main file
//   (N: `packageJsonContent == nil ||`);
// - the `HasPrefix(moduleFileToTry, mainExportFile)` test is gone; the
//   directory compare covers it.
fn is_package_main_file(
    module_file_name: &str,
    package_root_directory: &str,
    main_file_relative: &str,
    package_type: &str,
    use_case_sensitive_file_names: bool,
) -> bool {
    let main_is_directory = tspath::has_trailing_directory_separator(main_file_relative);
    // Go `packageRootDirectory.ResolveFile(mainFileRelative)`.
    let main_export_file = tspath::resolve_path_without_trailing_directory_separator(
        package_root_directory,
        &[main_file_relative],
    );

    if !main_is_directory
        && tspath::compare_rooted_text(
            tspath::remove_file_extension(&main_export_file),
            tspath::remove_file_extension(module_file_name),
            use_case_sensitive_file_names,
        ) == 0
    {
        // An arbitrary removal of file extension for this comparison is almost certainly wrong.
        return true;
    }
    // if mainExportFile is a directory, which contains moduleFileToTry, we just try index file
    // example mainExportFile: `pkg/lib` and moduleFileToTry: `pkg/lib/index`, we can use packageRootPath
    // but this behavior is deprecated for packages with "type": "module", so we only do this for packages without "type": "module"
    // and make sure that the extension on index.{???} is something that supports omitting the extension
    package_type != "module"
        && !tspath::file_extension_is_one_of(
            module_file_name,
            tspath::EXTENSIONS_NOT_SUPPORTING_EXTENSIONLESS_RESOLUTION,
        )
        && tspath::compare_rooted_text(
            &tspath::get_directory_path(module_file_name),
            &main_export_file,
            use_case_sensitive_file_names,
        ) == 0
        && tspath::remove_file_extension(&tspath::get_base_file_name(module_file_name)) == "index"
}

// Go: modulespecifiers/specifiers.go:1003 tryGetModuleNameFromExports
fn try_get_module_name_from_exports(
    options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    target_file_path: &str,
    package_directory: &str,
    package_name: &str,
    exports: &ExportsOrImports,
    conditions: &[String],
) -> String {
    if exports.is_subpaths() {
        // sub-mappings
        // 3 cases:
        // * directory mappings (legacyish, key ends with / (technically allows index/extension resolution under cjs mode))
        // * pattern mappings (contains a *)
        // * exact mappings (no *, does not end with /)
        for (k, subk) in exports.as_object() {
            // ts#64159 (specifiers.go:1019): ResolvePathWithoutTrailingDirectorySeparator.
            let sub_package_name =
                tspath::resolve_path_without_trailing_directory_separator(package_name, &[k]);
            let mut mode = MatchingMode::Exact;
            if k.ends_with('/') {
                mode = MatchingMode::Directory;
            } else if k.contains('*') {
                mode = MatchingMode::Pattern;
            }
            let result = try_get_module_name_from_exports_or_imports(
                options,
                host,
                target_file_path,
                package_directory,
                &sub_package_name,
                subk,
                conditions,
                mode,
                /*isImports*/ false,
                /*preferTsExtension*/ false,
            );
            if !result.is_empty() {
                return result;
            }
        }
    }
    try_get_module_name_from_exports_or_imports(
        options,
        host,
        target_file_path,
        package_directory,
        package_name,
        exports,
        conditions,
        MatchingMode::Exact,
        /*isImports*/ false,
        /*preferTsExtension*/ false,
    )
}

// Go: modulespecifiers/specifiers.go:1046 tryGetModuleNameFromPackageJsonImports
fn try_get_module_name_from_package_json_imports(
    module_file_name: &str,
    source_directory: &str,
    options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    import_mode: ResolutionMode,
    prefer_ts_extension: bool,
) -> String {
    if !options.get_resolve_package_json_imports() {
        return String::new();
    }

    let ancestor_directory_with_package_json =
        host.get_nearest_ancestor_directory_with_package_json(source_directory);
    if ancestor_directory_with_package_json.is_empty() {
        return String::new();
    }
    let package_json_path =
        tspath::combine_paths(&ancestor_directory_with_package_json, &["package.json"]);

    let Some(info) = host.get_package_json_info(&package_json_path) else {
        return String::new();
    };

    // PORT: Go dereferences `GetContents()` without a nil check. The host
    // only returns entries whose package.json was read.
    let imports = &info
        .get_contents()
        .expect("package.json contents")
        .fields
        .imports;
    match imports {
        JSONValue::NotPresent | JSONValue::Array(_) | JSONValue::String(_) => {
            return String::new(); // not present or invalid for imports
        }
        JSONValue::Object(top) => {
            let conditions = deps::get_conditions(options, import_mode);
            for (k, value) in top {
                if k == "#" || k == "#/" || !k.starts_with('#') {
                    continue; // invalid imports entry
                }
                if k.starts_with("#/")
                    && options.get_module_resolution_kind() != ModuleResolutionKind::NODE_NEXT
                    && options.get_module_resolution_kind() != ModuleResolutionKind::BUNDLER
                {
                    continue; // "#/" imports keys are only valid in nodenext/bundler
                }
                let mut mode = MatchingMode::Exact;
                if k.ends_with('/') {
                    mode = MatchingMode::Directory;
                } else if k.contains('*') {
                    mode = MatchingMode::Pattern;
                }
                let result = try_get_module_name_from_exports_or_imports(
                    options,
                    host,
                    module_file_name,
                    &ancestor_directory_with_package_json,
                    k,
                    value,
                    &conditions,
                    mode,
                    true,
                    prefer_ts_extension,
                );
                if !result.is_empty() {
                    return result;
                }
            }
        }
        _ => {}
    }

    String::new()
}

// Go: modulespecifiers/specifiers.go:1111 specPair
#[derive(Clone, Debug)]
struct SpecPair {
    ending: ModuleSpecifierEnding,
    value: String,
}

// Go: modulespecifiers/specifiers.go:1116 tryGetModuleNameFromPaths
fn try_get_module_name_from_paths(
    relative_to_base_url: &str,
    file_name: &str,
    paths: &IndexMap<String, Option<Vec<String>>>,
    allowed_endings: &[ModuleSpecifierEnding],
    base_directory: &str,
    host: &dyn ModuleSpecifierGenerationHost,
    compiler_options: &CompilerOptions,
) -> String {
    let case_sensitive = host.use_case_sensitive_file_names();
    for (key, values) in paths {
        // Go ranges over a nil slice as zero items.
        for pattern_text in values.iter().flatten() {
            let normalized = tspath::normalize_path(pattern_text);
            let mut pattern =
                get_relative_path_if_in_same_volume(&normalized, base_directory, case_sensitive);
            if pattern.is_empty() {
                pattern = normalized;
            }
            let star = pattern.split_once('*');

            // In module resolution, if `pattern` itself has an extension, a file with that extension is looked up directly,
            // meaning a '.ts' or '.d.ts' extension is allowed to resolve. This is distinct from the case where a '*' substitution
            // causes a module specifier to have an extension, i.e. the extension comes from the module specifier in a JS/TS file
            // and matches the '*'. See the Go source for the full table of cases.

            let mut candidates: Vec<SpecPair> = Vec::new();
            for ending in allowed_endings {
                let result = process_ending(
                    relative_to_base_url,
                    file_name,
                    &[*ending],
                    compiler_options,
                    host,
                );
                candidates.push(SpecPair {
                    ending: *ending,
                    value: result,
                });
            }
            if !tspath::try_get_extension_from_path(&pattern).is_empty() {
                candidates.push(SpecPair {
                    ending: ModuleSpecifierEnding::JsExtension,
                    value: relative_to_base_url.to_string(),
                });
            }

            if let Some((prefix, suffix)) = star {
                for c in &candidates {
                    let value = &c.value;
                    // PORT: Go byte lengths and slices (see
                    // `scanner_util::GO_STRING_MARKER`).
                    if go_len(value) >= go_len(prefix) + go_len(suffix)
                        && deps::has_prefix(value, prefix, case_sensitive) // TODO: possible strada bug: these are not case-switched in strada
                        && deps::has_suffix(value, suffix, case_sensitive)
                        && validate_ending(c, relative_to_base_url, file_name, compiler_options, host)
                    {
                        let matched_star =
                            go_slice(value, go_len(prefix), go_len(value) - go_len(suffix));
                        if !tspath::path_is_relative(&matched_star) {
                            return replace_first_star(key, &matched_star);
                        }
                    }
                }
            } else if candidates
                .iter()
                .any(|c| c.ending != ModuleSpecifierEnding::Minimal && pattern == c.value)
                || candidates.iter().any(|c| {
                    c.ending == ModuleSpecifierEnding::Minimal
                        && pattern == c.value
                        && validate_ending(
                            c,
                            relative_to_base_url,
                            file_name,
                            compiler_options,
                            host,
                        )
                })
            {
                return key.clone();
            }
        }
    }
    String::new()
}

// Go: modulespecifiers/specifiers.go:1219 validateEnding
fn validate_ending(
    c: &SpecPair,
    relative_to_base_url: &str,
    file_name: &str,
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
) -> bool {
    // Optimization: `removeExtensionAndIndexPostFix` can query the file system (a good bit) if `ending` is `Minimal`, the basename
    // is 'index', and a `host` is provided. To avoid that until it's unavoidable, we ran the function with no `host` above. Only
    // here, after we've checked that the minimal ending is indeed a match (via the length and prefix/suffix checks / `some` calls),
    // do we check that the host-validated result is consistent with the answer we got before. If it's not, it falls back to the
    // `ModuleSpecifierEnding.Index` result, which should already be in the list of candidates if `Minimal` was. (Note: the assumption here is
    // that every module resolution mode that supports dropping extensions also supports dropping `/index`. Like literally
    // everything else in this file, this logic needs to be updated if that's not true in some future module resolution mode.)
    c.ending != ModuleSpecifierEnding::Minimal
        || c.value
            == process_ending(
                relative_to_base_url,
                file_name,
                &[c.ending],
                compiler_options,
                host,
            )
}

// Go: modulespecifiers/specifiers.go:1230 tryGetModuleNameFromExportsOrImports
#[allow(clippy::too_many_arguments)]
fn try_get_module_name_from_exports_or_imports(
    options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    target_file_path: &str,
    package_directory: &str,
    package_name: &str,
    exports: &ExportsOrImports,
    conditions: &[String],
    mode: MatchingMode,
    is_imports: bool,
    prefer_ts_extension: bool,
) -> String {
    match exports {
        JSONValue::NotPresent => String::new(),
        JSONValue::String(str_value) => {
            // possible strada bug? Always uses compilerOptions of the host project, not those applicable to the targeted package.json!
            let mut output_file = String::new();
            let mut declaration_file = String::new();
            if is_imports {
                output_file = deps::get_output_js_file_name_worker(
                    target_file_path,
                    options,
                    host.as_output_paths_host(),
                );
                declaration_file = deps::get_output_declaration_file_name_worker(
                    target_file_path,
                    options,
                    host.as_output_paths_host(),
                );
            }

            let mut extension_swapped_target = String::new();
            if tspath::has_ts_file_extension(target_file_path) {
                extension_swapped_target = format!(
                    "{}{}",
                    tspath::remove_file_extension(target_file_path),
                    deps::try_get_js_extension_for_file(target_file_path, options)
                );
            }
            let can_try_ts_extension = prefer_ts_extension
                && tspath::has_implementation_ts_file_extension(target_file_path);

            let case_sensitive = host.use_case_sensitive_file_names();
            let compare_opts = tspath::ComparePathsOptions {
                use_case_sensitive_file_names: case_sensitive,
                current_directory: host.get_current_directory(),
            };

            match mode {
                MatchingMode::Exact => {
                    // ts#64159 (specifiers.go:1267): an exact target with a
                    // trailing separator names a directory, never this file.
                    if tspath::has_trailing_directory_separator(str_value) {
                        return String::new();
                    }
                    // Go: packageDirectory.ResolveFile(strValue), compared as
                    // rooted text (CaseSensitivity.CompareFilePaths, R3).
                    let resolved_target = tspath::get_normalized_absolute_path(
                        &tspath::combine_paths(package_directory, &[str_value]),
                        "",
                    );
                    let same = |file: &str| {
                        tspath::compare_rooted_text(file, &resolved_target, case_sensitive) == 0
                    };
                    if !extension_swapped_target.is_empty() && same(&extension_swapped_target)
                        || same(target_file_path)
                        || !output_file.is_empty() && same(&output_file)
                        || !declaration_file.is_empty() && same(&declaration_file)
                    {
                        return package_name.to_string();
                    }
                }
                MatchingMode::Directory => {
                    // Go: packageDirectory.ResolveDirectory(RemoveTrailingDirectorySeparator(strValue)).
                    let path_or_pattern = tspath::get_normalized_absolute_path(
                        &tspath::combine_paths(
                            package_directory,
                            &[tspath::remove_trailing_directory_separator(str_value)],
                        ),
                        "",
                    );
                    // Go: packageSpecifier.Resolve(strValue, fragment) (ts#64159).
                    let from_fragment = |fragment: &str| {
                        tspath::resolve_path_without_trailing_directory_separator(
                            package_name,
                            &[str_value, fragment],
                        )
                    };
                    // PORT: Go passes the arguments in this order
                    // (`targetFilePath` as the parent).
                    if can_try_ts_extension
                        && tspath::contains_path(target_file_path, &path_or_pattern, &compare_opts)
                    {
                        let fragment = tspath::get_relative_path_from_directory(
                            &path_or_pattern,
                            target_file_path,
                            &compare_opts,
                        );
                        return from_fragment(&fragment);
                    }
                    if !extension_swapped_target.is_empty()
                        && tspath::contains_path(
                            &path_or_pattern,
                            &extension_swapped_target,
                            &compare_opts,
                        )
                    {
                        let fragment = tspath::get_relative_path_from_directory(
                            &path_or_pattern,
                            &extension_swapped_target,
                            &compare_opts,
                        );
                        return from_fragment(&fragment);
                    }
                    if !can_try_ts_extension
                        && tspath::contains_path(&path_or_pattern, target_file_path, &compare_opts)
                    {
                        let fragment = tspath::get_relative_path_from_directory(
                            &path_or_pattern,
                            target_file_path,
                            &compare_opts,
                        );
                        return from_fragment(&fragment);
                    }
                    if !output_file.is_empty()
                        && tspath::contains_path(&path_or_pattern, &output_file, &compare_opts)
                    {
                        let fragment = tspath::get_relative_path_from_directory(
                            &path_or_pattern,
                            &output_file,
                            &compare_opts,
                        );
                        return tspath::combine_paths(package_name, &[&fragment]);
                    }
                    if !declaration_file.is_empty()
                        && tspath::contains_path(&path_or_pattern, &declaration_file, &compare_opts)
                    {
                        let fragment = tspath::get_relative_path_from_directory(
                            &path_or_pattern,
                            &declaration_file,
                            &compare_opts,
                        );
                        let js_extension = get_js_extension_for_file(&declaration_file, options);
                        let fragment_with_js_extension =
                            tspath::change_extension(&fragment, js_extension);
                        return tspath::combine_paths(package_name, &[&fragment_with_js_extension]);
                    }
                }
                MatchingMode::Pattern => {
                    // ts#64159 (specifiers.go:1306): ResolvePath keeps a
                    // trailing separator of the pattern.
                    let path_or_pattern = tspath::resolve_path(package_directory, &[str_value]);
                    let (leading_slice, trailing_slice) = path_or_pattern
                        .split_once('*')
                        .unwrap_or((path_or_pattern.as_str(), ""));
                    // PORT: Go slices bytes (see `scanner_util::go_slice`).
                    fn star_replacement<'s>(
                        s: &'s str,
                        leading: &str,
                        trailing: &str,
                    ) -> std::borrow::Cow<'s, str> {
                        go_slice(s, go_len(leading), go_len(s) - go_len(trailing))
                    }
                    if can_try_ts_extension
                        && deps::has_prefix_and_suffix_without_overlap(
                            target_file_path,
                            leading_slice,
                            trailing_slice,
                            case_sensitive,
                        )
                    {
                        return replace_first_star(
                            package_name,
                            &star_replacement(target_file_path, leading_slice, trailing_slice),
                        );
                    }
                    if !extension_swapped_target.is_empty()
                        && deps::has_prefix_and_suffix_without_overlap(
                            &extension_swapped_target,
                            leading_slice,
                            trailing_slice,
                            case_sensitive,
                        )
                    {
                        return replace_first_star(
                            package_name,
                            &star_replacement(
                                &extension_swapped_target,
                                leading_slice,
                                trailing_slice,
                            ),
                        );
                    }
                    if !can_try_ts_extension
                        && deps::has_prefix_and_suffix_without_overlap(
                            target_file_path,
                            leading_slice,
                            trailing_slice,
                            case_sensitive,
                        )
                    {
                        return replace_first_star(
                            package_name,
                            &star_replacement(target_file_path, leading_slice, trailing_slice),
                        );
                    }
                    if !output_file.is_empty()
                        && deps::has_prefix_and_suffix_without_overlap(
                            &output_file,
                            leading_slice,
                            trailing_slice,
                            case_sensitive,
                        )
                    {
                        return replace_first_star(
                            package_name,
                            &star_replacement(&output_file, leading_slice, trailing_slice),
                        );
                    }
                    if !declaration_file.is_empty()
                        && deps::has_prefix_and_suffix_without_overlap(
                            &declaration_file,
                            leading_slice,
                            trailing_slice,
                            case_sensitive,
                        )
                    {
                        let substituted = replace_first_star(
                            package_name,
                            &star_replacement(&declaration_file, leading_slice, trailing_slice),
                        );
                        let js_extension =
                            deps::try_get_js_extension_for_file(&declaration_file, options);
                        if !js_extension.is_empty() {
                            return tspath::change_full_extension(&substituted, js_extension);
                        }
                    }
                }
            }
            String::new()
        }
        JSONValue::Array(arr) => {
            for e in arr {
                let result = try_get_module_name_from_exports_or_imports(
                    options,
                    host,
                    target_file_path,
                    package_directory,
                    package_name,
                    e,
                    conditions,
                    mode,
                    is_imports,
                    prefer_ts_extension,
                );
                if !result.is_empty() {
                    return result;
                }
            }
            String::new()
        }
        JSONValue::Object(obj) => {
            // conditional mapping
            for (key, value) in obj {
                if key == "default"
                    || conditions.iter().any(|c| c == key)
                    || conditions.iter().any(|c| c == "types")
                        && deps::is_applicable_versioned_types_key(key)
                {
                    let result = try_get_module_name_from_exports_or_imports(
                        options,
                        host,
                        target_file_path,
                        package_directory,
                        package_name,
                        value,
                        conditions,
                        mode,
                        is_imports,
                        prefer_ts_extension,
                    );
                    if !result.is_empty() {
                        return result;
                    }
                }
            }
            String::new()
        }
        JSONValue::Null => String::new(),
        _ => String::new(),
    }
}

// Go: modulespecifiers/specifiers.go:1367 GetModuleSpecifier
// `importingSourceFile` and `importingSourceFileName`? Why not just use `importingSourceFile.path`?
// Because when this is called by the declaration emitter, `importingSourceFile` is the implementation
// file, but `importingSourceFileName` and `toFileName` refer to declaration files (the former to the
// one currently being produced; the latter to the one being imported). We need an implementation file
// just to get its `impliedNodeFormat` and to detect certain preferences from existing import module
// specifiers.
pub fn get_module_specifier(
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    importing_source_file: Node, // !!! | FutureSourceFile
    importing_source_file_name: &str,
    old_import_specifier: &str, // used only in updatingModuleSpecifier
    to_file_name: &str,
    options: ModuleSpecifierOptions,
) -> String {
    get_module_specifier_with_preferences(
        compiler_options,
        host,
        importing_source_file,
        importing_source_file_name,
        old_import_specifier,
        to_file_name,
        &UserPreferences::default(),
        options,
    )
}

// Go: modulespecifiers/specifiers.go:1388 UpdateModuleSpecifier
#[allow(clippy::too_many_arguments)]
pub fn update_module_specifier(
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    importing_source_file: Node,
    importing_source_file_name: &str,
    old_import_specifier: &str,
    to_file_name: &str,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
) -> String {
    get_module_specifier_with_preferences(
        compiler_options,
        host,
        importing_source_file,
        importing_source_file_name,
        old_import_specifier,
        to_file_name,
        user_preferences,
        options,
    )
}

// Go: modulespecifiers/specifiers.go:1410 getModuleSpecifierWithPreferences
#[allow(clippy::too_many_arguments)]
fn get_module_specifier_with_preferences(
    compiler_options: &CompilerOptions,
    host: &dyn ModuleSpecifierGenerationHost,
    importing_source_file: Node, // !!! | FutureSourceFile
    importing_source_file_name: &str,
    old_import_specifier: &str, // used only in updatingModuleSpecifier
    to_file_name: &str,
    user_preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
) -> String {
    let info = get_info(importing_source_file_name, host);
    let module_paths = get_all_module_paths(
        &info,
        to_file_name,
        host,
        compiler_options,
        user_preferences,
        options,
    );
    let preferences = get_module_specifier_preferences(
        user_preferences,
        host,
        compiler_options,
        &importing_source_file,
        old_import_specifier,
    );

    let mut resolution_mode = options.override_import_mode;
    if resolution_mode == RESOLUTION_MODE_NONE {
        resolution_mode = host.get_default_resolution_mode_for_file(importing_source_file);
    }

    for module_path in &module_paths {
        let first_defined = try_get_module_name_as_node_module(
            module_path,
            &info,
            &importing_source_file,
            host,
            compiler_options,
            user_preferences,
            false, /*packageNameOnly*/
            options.override_import_mode,
        );
        if !first_defined.is_empty() {
            return first_defined;
        }
    }

    get_local_module_specifier(
        to_file_name,
        &info,
        compiler_options,
        host,
        resolution_mode,
        &preferences,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: modulespecifiers/specifiers_test.go:328 TestIsPackageMainFilePreservesDirectoryIntent (ts#64159)
    #[test]
    fn is_package_main_file_preserves_directory_intent() {
        let package_root = "/project/node_modules/pkg";
        assert!(
            !is_package_main_file(
                "/project/node_modules/pkg/types.d.ts",
                package_root,
                "./types/",
                "",
                true
            ),
            "slash-terminated package entrypoint must not match the sibling declaration file"
        );
        assert!(
            is_package_main_file(
                "/project/node_modules/pkg/types/index.d.ts",
                package_root,
                "./types/",
                "",
                true
            ),
            "slash-terminated package entrypoint should match its index declaration"
        );
        assert!(
            is_package_main_file(
                "/project/node_modules/pkg/types.d.ts",
                package_root,
                "./types",
                "",
                true
            ),
            "extensionless package entrypoint should match the declaration file"
        );
    }
}
