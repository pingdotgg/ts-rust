use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/specifiers.go`.

use crate::modulespecifiers;

impl View {
    // Go: ls/autoimport/specifiers.go:8 GetModuleSpecifier
    // PORT: Go passes `v.program` as the `ModuleSpecifierGenerationHost`; the
    // host for the installed program is `modulespecifiers::ProgramHost`
    // (plan D-LS1). Go `*collections.Set` conditions on a resolved entrypoint
    // are `Option<FxHashSet<String>>` (`None` is nil; Go `IsSubsetOf` on nil
    // is true and `Intersects` with nil is false). Go
    // `v.registry.specifierCache[path]` is a nil `*SyncMap` when the key is
    // missing; `Load` and `Store` on it panic as in Go.
    // PORT: Go map order is random. The cache key is `export.path` and the
    // value comes from `export.module_file_name`, as in Go. For a relative
    // module augmentation export the two name different files, so the call
    // order decides the specifier that the declaring file's exports get for
    // the rest of the session. `View::get_completions` fixes that order.
    // PORT: ts#64159 behavior only: Go `tspath.ModuleSpecifier` is `String`.
    pub fn get_module_specifier(
        &self,
        export: &Export,
        user_preferences: &modulespecifiers::UserPreferences,
    ) -> (String, modulespecifiers::ResultKind) {
        // ts#64159: specifiers.go:12, a relative module augmentation whose
        // module did not resolve. Its specifier is relative to the importing
        // file; on another volume there is none (R4).
        if !export.unresolved_module_specifier.is_empty() {
            let mut specifier = export.unresolved_module_specifier.clone();
            if tspath::path_is_relative(&specifier) {
                let Some(relative_path) = tspath::relative_path_from_directory(
                    &tspath::get_directory_path(source_file_file_name(self.importing_file)),
                    &export.module_file_name,
                    self.program.use_case_sensitive_file_names(),
                ) else {
                    return (String::new(), modulespecifiers::ResultKind::None);
                };
                specifier = tspath::ensure_path_is_non_module_name(&relative_path);
            }
            if modulespecifiers::is_excluded_by_regex(
                &specifier,
                &user_preferences.auto_import_specifier_exclude_regexes,
            ) {
                return (String::new(), modulespecifiers::ResultKind::None);
            }
            return (specifier, modulespecifiers::ResultKind::Relative);
        }

        // ts#64159: specifiers.go:30, an ambient module by its module ID kind.
        if let Some(specifier) = export.module_id.as_module_specifier() {
            if modulespecifiers::is_excluded_by_regex(
                specifier,
                &user_preferences.auto_import_specifier_exclude_regexes,
            ) {
                return (String::new(), modulespecifiers::ResultKind::None);
            }
            return (specifier.to_string(), modulespecifiers::ResultKind::Ambient);
        }

        if !export.package_name.is_empty() {
            if let Some(entrypoints) = self.registry.entrypoints.get(&export.path) {
                for entrypoint in entrypoints {
                    // Go: entrypoint.IncludeConditions.IsSubsetOf(v.conditions)
                    let is_subset = match &entrypoint.include_conditions {
                        None => true,
                        Some(include) => include.iter().all(|key| self.conditions.contains(key)),
                    };
                    // Go: v.conditions.Intersects(entrypoint.ExcludeConditions)
                    let intersects = match &entrypoint.exclude_conditions {
                        None => false,
                        Some(exclude) => self.conditions.iter().any(|key| exclude.contains(key)),
                    };
                    if is_subset && !intersects {
                        let specifier =
                            modulespecifiers::entrypoint_ending::process_entrypoint_ending(
                                entrypoint,
                                user_preferences,
                                &modulespecifiers::ProgramHost,
                                self.program.options(),
                                &self.importing_file,
                                &self.get_allowed_endings(),
                            );

                        if !modulespecifiers::is_excluded_by_regex(
                            &specifier,
                            &user_preferences.auto_import_specifier_exclude_regexes,
                        ) {
                            return (specifier, modulespecifiers::ResultKind::NodeModules);
                        }
                    }
                }
                return (String::new(), modulespecifiers::ResultKind::None);
            }
        }

        let cache = self
            .registry
            .specifier_cache
            .get(&self.importing_file_path)
            .cloned();
        if export.package_name.is_empty() {
            let loaded = cache
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .get(&export.path)
                .cloned();
            if let Some(specifier) = loaded {
                if specifier.is_empty() {
                    return (String::new(), modulespecifiers::ResultKind::None);
                }
                return (specifier, modulespecifiers::ResultKind::Relative);
            }
        }

        let (specifiers, kind) = modulespecifiers::get_module_specifiers_for_file_with_info(
            &self.importing_file,
            &export.module_file_name,
            self.program.options(),
            &modulespecifiers::ProgramHost,
            user_preferences,
            modulespecifiers::ModuleSpecifierOptions::default(),
            true,
        );
        // !!! unsure when this could return multiple specifiers combined with the
        //     new node_modules code. Possibly with local symlinks, which should be
        //     very rare.
        for specifier in specifiers {
            if specifier.contains("/node_modules/") {
                continue;
            }
            cache
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow_mut()
                .insert(export.path.clone(), specifier.clone());
            return (specifier, kind);
        }
        cache
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow_mut()
            .insert(export.path.clone(), String::new());
        (String::new(), modulespecifiers::ResultKind::None)
    }
}
