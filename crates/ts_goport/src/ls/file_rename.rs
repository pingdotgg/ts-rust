//! Port of Go `ls/file_rename.go`.
//!
//! PORT: Go `*compiler.Program` is `&compiler::NewProgram`. Go
//! `*change.Tracker` is `&mut change::Tracker`. Go passes an
//! `*ast.SourceFile` where a program method takes an `ast.HasFileName`; here
//! that is `autoimport::source_file_has_file_name(file)`.

use crate::ls::prelude::*;

// Go: ls/file_rename.go:22 pathUpdater
// PORT: a Go func type. `createPathUpdater` returns it boxed; other functions
// take it as `&PathUpdater`.
pub type PathUpdater<'a> = dyn Fn(&str) -> (String, bool) + 'a;

// Go: ls/file_rename.go:24 toImport
#[derive(Clone, Debug, Default)]
pub struct ToImport {
    pub new_file_name: String,
    pub updated: bool,
}

// Go: ls/file_rename.go:29 movedFile
#[derive(Clone, Debug, Default)]
pub struct MovedFile {
    pub source_file: Node,
    pub new_file_name: String,
}

impl LanguageService {
    // Go: ls/file_rename.go:34 GetEditsForFileRename
    pub fn get_edits_for_file_rename(
        &self,
        ctx: &Context,
        old_uri: &lsproto::DocumentUri,
        new_uri: &lsproto::DocumentUri,
    ) -> Vec<lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile> {
        let program = self.get_program();
        let old_path = old_uri.file_name();
        let new_path = new_uri.file_name();

        let old_to_new = self.create_path_updater(&old_path, &new_path);

        let mut change_tracker = change::new_tracker(
            ctx,
            program.options(),
            self.format_options(),
            self.converters.clone(),
        );
        self.update_tsconfig_files(
            program,
            &mut change_tracker,
            &*old_to_new,
            &old_path,
            &new_path,
        );
        self.update_imports_for_file_rename(program, &mut change_tracker, &*old_to_new);

        let mut document_changes: Vec<
            lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile,
        > = Vec::new();

        // When renaming e.g. `foo.d.css.ts` -> `bar.d.css.ts`, also rename `foo.css` -> `bar.css` if it exists.
        if tspath::is_declaration_file_name(&old_path)
            && tspath::is_declaration_file_name(&new_path)
        {
            let dts_ext = tspath::get_declaration_file_extension(&old_path);
            let original_extensions =
                tspath::get_possible_original_input_extension_for_extension(&dts_ext);
            for ext in &original_extensions {
                let old_original_path = tspath::change_full_extension(&old_path, ext);
                if self.host.file_exists(&old_original_path) {
                    // ts#64159: the new file's declaration extension (Go N'
                    // file_rename.go:56; N read the old file's).
                    let new_dts_ext = tspath::get_declaration_file_extension(&new_path);
                    let new_original_extensions =
                        tspath::get_possible_original_input_extension_for_extension(&new_dts_ext);
                    if new_original_extensions.contains(ext) {
                        let new_original_path = tspath::change_full_extension(&new_path, ext);
                        document_changes.push(
                            lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                                rename_file: Some(lsproto::RenameFile {
                                    old_uri: lsconv::file_name_to_document_uri(&old_original_path),
                                    new_uri: lsconv::file_name_to_document_uri(&new_original_path),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
        }

        // PORT: Go ranges over the `GetChanges` map (random order). The
        // tracker returns an IndexMap in the order files were first changed.
        let (changes, _) = change_tracker.get_changes();
        for (file_name, edits) in changes {
            let uri = lsconv::file_name_to_document_uri(&file_name);
            let mut lsp_edits: Vec<lsproto::TextEditOrAnnotatedTextEditOrSnippetTextEdit> =
                Vec::with_capacity(edits.len());
            for edit in edits {
                lsp_edits.push(lsproto::TextEditOrAnnotatedTextEditOrSnippetTextEdit {
                    text_edit: Some(edit),
                    ..Default::default()
                });
            }
            document_changes.push(
                lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                    text_document_edit: Some(lsproto::TextDocumentEdit {
                        text_document: lsproto::OptionalVersionedTextDocumentIdentifier {
                            uri,
                            ..Default::default()
                        },
                        edits: lsp_edits,
                    }),
                    ..Default::default()
                },
            );
        }

        document_changes
    }

    // Go: ls/file_rename.go:89 createPathUpdater
    // PORT: the body is `new_path_updater`, which takes Go's
    // `l.UseCaseSensitiveFileNames()` as a value. The Go closure reads it on
    // each call; the host is a snapshot, so each call gets the same value.
    // The split lets the Go test run with no LanguageService (a Rust
    // LanguageService needs a program).
    pub fn create_path_updater<'a>(
        &'a self,
        old_path: &str,
        new_path: &str,
    ) -> Box<PathUpdater<'a>> {
        new_path_updater(self.use_case_sensitive_file_names(), old_path, new_path)
    }

    // Go: ls/file_rename.go:112 updateTsconfigFiles
    pub fn update_tsconfig_files(
        &self,
        program: &compiler::NewProgram,
        change_tracker: &mut change::Tracker,
        old_to_new: &PathUpdater<'_>,
        old_path: &str,
        new_path: &str,
    ) {
        // PORT: Go `program.CommandLine()` can be nil; here it is always set.
        let command_line = program.command_line();
        let Some(config_file_info) = command_line.config_file.as_ref() else {
            return;
        };

        let config_file = config_file_info.source_file;
        if config_file.is_nil() {
            return;
        }
        let config_dir = tspath::get_directory_path(source_file_file_name(config_file));
        let json_object_literal = get_ts_config_object_literal_expression(config_file);
        if json_object_literal.is_nil() {
            return;
        }

        for_each_object_property(
            json_object_literal,
            &mut |property: Node, property_name: &str| match property_name {
                "files" | "include" | "exclude" => {
                    let found_exact_match = update_paths_property(
                        config_file,
                        &config_dir,
                        property,
                        change_tracker,
                        old_to_new,
                        &self.converters,
                        self.use_case_sensitive_file_names(),
                    );
                    if found_exact_match
                        || property_name != "include"
                        || !is_array_literal_expression(property.initializer())
                    {
                        return;
                    }
                    let (old_spec, is_default) = command_line.get_matched_include_spec(old_path);
                    if !old_spec.is_empty() && !is_default {
                        let (new_spec, _) = command_line.get_matched_include_spec(new_path);
                        if new_spec.is_empty() {
                            let elements = property.initializer().elements();
                            if !elements.is_empty() {
                                let last_element = elements.get(elements.len() - 1);
                                // ts#64159 (Go N' file_rename.go:130-133): the
                                // absolute name when the roots differ (R4).
                                let new_path_text = tspath::relative_path_from_directory(
                                    &config_dir,
                                    new_path,
                                    self.use_case_sensitive_file_names(),
                                )
                                .unwrap_or_else(|| new_path.to_string());
                                let new_node = change_tracker
                                    .node_factory()
                                    .new_string_literal(new_path_text, TokenFlags::NONE);
                                change_tracker.insert_node_after(
                                    config_file,
                                    last_element,
                                    new_node,
                                );
                            }
                        }
                    }
                }
                "compilerOptions" => {
                    if !is_object_literal_expression(property.initializer()) {
                        return;
                    }
                    for_each_object_property(
                        property.initializer(),
                        &mut |property: Node, property_name: &str| {
                            let option =
                                tsoptions::COMMAND_LINE_COMPILER_OPTIONS_MAP.get(property_name);
                            if let Some(option) = option {
                                let element_option = option.elements();
                                if option.is_file_path
                                    || (option.kind == tsoptions::CommandLineOptionKind::LIST
                                        && element_option
                                            .is_some_and(|element| element.is_file_path))
                                {
                                    update_paths_property(
                                        config_file,
                                        &config_dir,
                                        property,
                                        change_tracker,
                                        old_to_new,
                                        &self.converters,
                                        self.use_case_sensitive_file_names(),
                                    );
                                    return;
                                }
                            }

                            if property_name != "paths"
                                || !is_object_literal_expression(property.initializer())
                            {
                                return;
                            }
                            for_each_object_property(
                                property.initializer(),
                                &mut |paths_property: Node, _: &str| {
                                    if !is_array_literal_expression(paths_property.initializer()) {
                                        return;
                                    }
                                    for element in paths_property.initializer().elements().iter() {
                                        try_update_config_string(
                                            config_file,
                                            &config_dir,
                                            element,
                                            change_tracker,
                                            old_to_new,
                                            &self.converters,
                                            self.use_case_sensitive_file_names(),
                                        );
                                    }
                                },
                            );
                        },
                    );
                }
                _ => {}
            },
        );
    }
}

// Go: ls/file_rename.go:177 updatePathsProperty
pub fn update_paths_property(
    config_file: Node,
    config_dir: &str,
    property: Node,
    change_tracker: &mut change::Tracker,
    old_to_new: &PathUpdater<'_>,
    converters: &lsconv::Converters,
    use_case_sensitive_file_names: bool,
) -> bool {
    let mut elements: Vec<Node> = vec![property.initializer()];
    if is_array_literal_expression(property.initializer()) {
        elements = property.initializer().elements().to_vec();
    }

    let mut found_exact_match = false;
    for element in elements {
        found_exact_match = try_update_config_string(
            config_file,
            config_dir,
            element,
            change_tracker,
            old_to_new,
            converters,
            use_case_sensitive_file_names,
        ) || found_exact_match;
    }
    found_exact_match
}

// Go: ls/file_rename.go:190 tryUpdateConfigString
pub fn try_update_config_string(
    config_file: Node,
    config_dir: &str,
    element: Node,
    change_tracker: &mut change::Tracker,
    old_to_new: &PathUpdater<'_>,
    converters: &lsconv::Converters,
    use_case_sensitive_file_names: bool,
) -> bool {
    if !is_string_literal(element) {
        return false;
    }

    // Go: configDir.ResolveFile(element.Text()) (ts#64159)
    let element_file_name = resolve_file(config_dir, element.text());
    let (updated, ok) = old_to_new(&element_file_name);
    if !ok {
        return false;
    }

    // PORT: Go `End()-1` steps back one Go byte (`go_offset_before`). An
    // unterminated literal at the end of the file can end in a marker unit
    // (see `GO_STRING_MARKER`), which has more port bytes than Go bytes.
    let text_range = TextRange::new(
        get_token_pos_of_node(element, config_file, false) + 1,
        go_offset_before(&source_file_text(config_file), element.end()),
    );
    let (lsp_range, fidelity) = converters.to_lsp_range(&config_file, text_range);
    crate::go_assert!(fidelity.is_exact(), "config files are not content-mapped");
    change_tracker.replace_range_with_text(
        config_file,
        lsp_range,
        &relative_path_from_directory(config_dir, &updated, use_case_sensitive_file_names),
    );
    true
}

// Go: ls/file_rename.go:90 createPathUpdater (the body, see
// `LanguageService::create_path_updater`)
// ts#64159: the file names are rooted and normalized, so they compare as
// rooted text (`CaseSensitivity.CompareFilePaths`, rooted_path.go:508), and a
// file below the old directory keeps its path relative to it
// (`RelativeFilePathFromDirectory`, :563). The relative path is trimmed by
// rune count (tsgo#4900, the Kelvin sign test).
fn new_path_updater(
    use_case_sensitive_file_names: bool,
    old_path: &str,
    new_path: &str,
) -> Box<PathUpdater<'static>> {
    let old_path = old_path.to_string();
    let new_path = new_path.to_string();
    Box::new(move |path: &str| -> (String, bool) {
        if tspath::compare_rooted_text(path, &old_path, use_case_sensitive_file_names) == 0 {
            return (new_path.clone(), true);
        }
        if let Some(relative_path) =
            tspath::relative_path_within_directory(&old_path, path, use_case_sensitive_file_names)
        {
            return (resolve_relative_file(&new_path, &relative_path), true);
        }
        (String::new(), false)
    })
}

// Go: tspath/rooted_path.go:727 RootedDirectoryPath.ResolveFile (ts#64159)
// PORT: the ls lane's copy until tspath has the rooted path helpers. Go
// ToRootedFilePath normalizes `path` against `directory` (and panics for an
// empty directory, or a URL query or fragment).
fn resolve_file(directory: &str, path: &str) -> String {
    if path.is_empty() {
        return directory.to_string();
    }
    tspath::get_normalized_absolute_path(path, directory)
}

// Go: tspath/rooted_path.go:746 RootedDirectoryPath.ResolveRelativeFile (ts#64159)
fn resolve_relative_file(directory: &str, path: &str) -> String {
    if path.is_empty() {
        return directory.to_string();
    }
    if path == ".." || path.starts_with("../") || tspath::has_trailing_directory_separator(path) {
        return tspath::get_normalized_absolute_path(path, directory);
    }
    if tspath::has_trailing_directory_separator(directory) {
        format!("{directory}{path}")
    } else {
        format!("{directory}/{path}")
    }
}

impl LanguageService {
    // Go: ls/file_rename.go:208 updateRelativePath
    pub fn update_relative_path(
        &self,
        old_to_new: &PathUpdater<'_>,
        old_import_from_path: &str,
        new_import_from_path: &str,
        relative_specifier: &str,
    ) -> String {
        // Go: oldImportFromPath.Directory().ResolveFile(relativeSpecifier) (ts#64159)
        let old_absolute = resolve_file(
            &tspath::get_directory_path(old_import_from_path),
            relative_specifier,
        );
        let (mut new_absolute, ok) = old_to_new(&old_absolute);
        if !ok {
            new_absolute = old_absolute;
        }
        relative_import_path_from_directory(
            &tspath::get_directory_path(new_import_from_path),
            &new_absolute,
            self.use_case_sensitive_file_names(),
        )
    }

    // Go: ls/file_rename.go:217 updateImportsForFileRename
    pub fn update_imports_for_file_rename(
        &self,
        program: &compiler::NewProgram,
        change_tracker: &mut change::Tracker,
        old_to_new: &PathUpdater<'_>,
    ) {
        let all_files: Vec<Node> = program
            .get_source_files()
            .iter()
            .map(|source_file| source_file.root)
            .collect();
        // Go: `defer done()`; `_done` releases the lease at the end of the scope.
        let (checker_rc, _done) =
            ls_program::get_type_checker(program, &crate::gostd::context::background());
        let checker = &mut *checker_rc.borrow_mut();
        let module_specifier_preferences = self.user_preferences().module_specifier_preferences();

        let mut moved_files: Vec<MovedFile> = Vec::new();
        for &source_file in &all_files {
            let (new_file_name, ok) = old_to_new(source_file_original_file_name(source_file));
            if ok {
                moved_files.push(MovedFile {
                    source_file,
                    new_file_name,
                });
            }
        }

        for source_file in all_files {
            let old_file_name = source_file_original_file_name(source_file);
            let (new_from_old, file_moved) = old_to_new(old_file_name);
            let mut new_import_from_path = old_file_name.to_string();
            if file_moved {
                new_import_from_path = new_from_old;
            }

            for ref_ in &source_file_info(source_file).referenced_files {
                if !tspath::is_external_module_name_relative(&ref_.file_name) {
                    continue;
                }
                let updated = self.update_relative_path(
                    old_to_new,
                    old_file_name,
                    &new_import_from_path,
                    &ref_.file_name,
                );
                if updated != ref_.file_name {
                    change_tracker.replace_text_range_with_text(source_file, ref_.range, &updated);
                }
            }

            for import_string_literal in source_file_imports(source_file).iter() {
                let updated = self.get_updated_import_specifier(
                    program,
                    checker,
                    source_file,
                    import_string_literal,
                    old_to_new,
                    &moved_files,
                    &new_import_from_path,
                    file_moved,
                    &module_specifier_preferences,
                );
                if !updated.is_empty() && updated != import_string_literal.text() {
                    change_tracker.replace_text_range_with_text(
                        source_file,
                        create_string_text_range(source_file, import_string_literal),
                        &updated,
                    );
                }
            }
        }
    }

    // Go: ls/file_rename.go:258 getUpdatedImportSpecifier
    // We assume the source file did not move to a different program.
    // PORT: Go passes the program as the `ModuleSpecifierGenerationHost`;
    // here that is `modulespecifiers::ProgramHost` (the installed program).
    pub fn get_updated_import_specifier(
        &self,
        program: &compiler::NewProgram,
        checker: &mut Checker,
        source_file: Node, // old importing source file
        import_literal: Node,
        old_to_new: &PathUpdater<'_>,
        moved_files: &[MovedFile],
        new_import_from_path: &str,
        importing_source_file_moved: bool,
        user_preferences: &modulespecifiers::UserPreferences,
    ) -> String {
        let imported_module_symbol = checker.get_symbol_at_location_exported(import_literal);
        if is_ambient_module_symbol(&checker.symbols, imported_module_symbol) {
            return String::new();
        }

        let target = get_source_file_to_import(program, source_file, import_literal, old_to_new);

        let Some(target) = target else {
            // First fall back: try every file affected by the rename to see if any of them would match the import specifier, and if so, obtain the updated specifier for that file.
            let updated = get_updated_import_specifier_from_moved_source_files(
                program,
                source_file,
                import_literal,
                moved_files,
                new_import_from_path,
                user_preferences,
            );
            if !updated.is_empty() && updated != import_literal.text() {
                return updated;
            }
            // Fall back to a regular path update for unresolved module.
            if tspath::is_external_module_name_relative(import_literal.text()) {
                return self.update_relative_path(
                    old_to_new,
                    source_file_file_name(source_file),
                    new_import_from_path,
                    import_literal.text(),
                );
            }
            return String::new();
        };

        // Optimization: neither the importing or imported file changed.
        if !target.updated
            && !(importing_source_file_moved
                && tspath::is_external_module_name_relative(import_literal.text()))
        {
            return String::new();
        }

        modulespecifiers::update_module_specifier(
            program.options(),
            &modulespecifiers::ProgramHost,
            source_file,
            new_import_from_path,
            import_literal.text(),
            &target.new_file_name,
            user_preferences,
            modulespecifiers::ModuleSpecifierOptions {
                override_import_mode: program.get_mode_for_usage_location(
                    &autoimport::source_file_has_file_name(source_file),
                    import_literal,
                ),
            },
        )
    }
}

// Go: ls/file_rename.go:308 getSourceFileToImport
// PORT: Go returns `*toImport`; nil is `None`.
pub fn get_source_file_to_import(
    program: &compiler::NewProgram,
    source_file: Node,
    import_literal: Node,
    old_to_new: &PathUpdater<'_>,
) -> Option<ToImport> {
    if let Some(resolved) = program.get_resolved_module_from_module_specifier(
        &autoimport::source_file_has_file_name(source_file),
        import_literal,
    ) {
        if !resolved.resolved_file_name.is_empty() {
            let old_file_name = resolved.resolved_file_name.clone();
            let (new_file_name, ok) = old_to_new(&old_file_name);
            if ok {
                return Some(ToImport {
                    new_file_name,
                    updated: true,
                });
            }
            return Some(ToImport {
                new_file_name: old_file_name,
                updated: false,
            });
        }
    }

    None
}

// Go: ls/file_rename.go:327 getUpdatedImportSpecifierFromMovedSourceFiles
// As a fall back for unresolved modules, we'll check every file affected by the rename to see if any of them would match
// the import specifier, and if so, we'll obtain the updated specifier for that file.
// PORT: Go passes the program as the `ModuleSpecifierGenerationHost`; here
// that is `modulespecifiers::ProgramHost` (the installed program).
pub fn get_updated_import_specifier_from_moved_source_files(
    program: &compiler::NewProgram,
    source_file: Node,
    import_literal: Node,
    moved_files: &[MovedFile],
    importing_source_file_name: &str,
    user_preferences: &modulespecifiers::UserPreferences,
) -> String {
    let resolution_mode = program.get_mode_for_usage_location(
        &autoimport::source_file_has_file_name(source_file),
        import_literal,
    );
    for candidate in moved_files {
        let old_specifier = modulespecifiers::update_module_specifier(
            program.options(),
            &modulespecifiers::ProgramHost,
            source_file,
            importing_source_file_name,
            import_literal.text(),
            source_file_file_name(candidate.source_file),
            user_preferences,
            modulespecifiers::ModuleSpecifierOptions {
                override_import_mode: resolution_mode,
            },
        );
        if old_specifier != import_literal.text() {
            continue;
        }

        return modulespecifiers::update_module_specifier(
            program.options(),
            &modulespecifiers::ProgramHost,
            source_file,
            importing_source_file_name,
            import_literal.text(),
            &candidate.new_file_name,
            user_preferences,
            modulespecifiers::ModuleSpecifierOptions {
                override_import_mode: resolution_mode,
            },
        );
    }
    String::new()
}

// Go: ls/file_rename.go:362 createStringTextRange
// PORT: Go `End()-1` steps back one Go byte (`go_offset_before`), as in
// `try_update_config_string`.
pub fn create_string_text_range(source_file: Node, node: Node) -> TextRange {
    TextRange::new(
        get_token_pos_of_node(node, source_file, false) + 1,
        go_offset_before(&source_file_text(source_file), node.end()),
    )
}

// Go: ls/file_rename.go:366 getTsConfigObjectLiteralExpression
// PORT: Go returns `*ast.ObjectLiteralExpression`; nil is `Node::NIL`.
pub fn get_ts_config_object_literal_expression(ts_config_source_file: Node) -> Node {
    if ts_config_source_file.is_some()
        && ts_config_source_file.statement_list().is_some()
        && !ts_config_source_file.statements().is_empty()
    {
        let expression = ts_config_source_file.statements().get(0).expression();
        if is_object_literal_expression(expression) {
            return expression;
        }
    }
    Node::NIL
}

// Go: ls/file_rename.go:376 forEachObjectProperty
// PORT: Go `cb func(property *ast.PropertyAssignment, propertyName string)`.
pub fn for_each_object_property(object_literal: Node, cb: &mut dyn FnMut(Node, &str)) {
    if object_literal.is_nil() {
        return;
    }
    for property in object_literal.properties().iter() {
        if !is_property_assignment(property) {
            continue;
        }
        let (name, ok) = try_get_text_of_property_name(property.name());
        if ok {
            cb(property, &name);
        }
    }
}

// Go: ls/file_rename.go:385 relativePathFromDirectory
// ts#64159: the absolute name when the roots differ (R4).
pub fn relative_path_from_directory(
    from_directory: &str,
    to: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    tspath::relative_path_from_directory(from_directory, to, use_case_sensitive_file_names)
        .unwrap_or_else(|| to.to_string())
}

// Go: ls/file_rename.go:392 relativeImportPathFromDirectory
// ts#64159: a relative path gets "./" (`RelativePath.AsModuleSpecifier`,
// relative_path.go:31); the absolute name when the roots differ (R4).
pub fn relative_import_path_from_directory(
    from_directory: &str,
    to: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    match tspath::relative_path_from_directory(from_directory, to, use_case_sensitive_file_names) {
        Some(relative_path) => tspath::ensure_path_is_non_module_name(&relative_path),
        None => to.to_string(),
    }
}

// Go: ls/file_rename.go:398 isAmbientModuleSymbol
// PORT: Go reads `symbol.Declarations` without a checker; the symbol arena
// is the first parameter, as for ast helpers that take a symbol.
pub fn is_ambient_module_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    if symbol.is_nil() {
        return false;
    }
    symbols
        .sym(symbol)
        .declarations
        .iter()
        .any(|&declaration| is_module_with_string_literal_name(declaration))
}

// Go: ls/file_rename_test.go (tsgo#4900)
#[cfg(test)]
mod tests {
    use super::new_path_updater;
    use crate::ls::rename::try_remove_index_file_name;

    // Go: ls/file_rename_test.go:46 TestTryRemoveIndexFileName (ts#64159)
    #[test]
    fn test_try_remove_index_file_name() {
        assert_eq!(try_remove_index_file_name("/project/index.ts"), "/project");
        assert_eq!(try_remove_index_file_name("/index.ts"), "");
        assert_eq!(try_remove_index_file_name("c:/index.ts"), "c:/");
        assert_eq!(try_remove_index_file_name("^/index.ts"), "");
    }

    // Go: ls/file_rename_test.go:57 TestCreatePathUpdaterCaseFoldingShrinksOldPath
    // TestCreatePathUpdaterCaseFoldingShrinksOldPath verifies that a case-insensitive
    // descendant update does not depend on the byte lengths of differently-cased paths.
    // PORT: Go builds `&LanguageService{host: caseInsensitiveHost{}}`, whose
    // only used method is `CaseSensitivity() == CaseInsensitive`. A Rust
    // LanguageService needs a program, so the test calls the updater body
    // with that value.
    #[test]
    fn test_create_path_updater_case_folding_shrinks_old_path() {
        let old_path = "/a/\u{212A}\u{212A}\u{212A}\u{212A}";
        let new_path = "/a/new";
        let updater =
            new_path_updater(false /*useCaseSensitiveFileNames*/, old_path, new_path);

        let (updated, ok) = updater("/a/kkkk/x.ts");
        assert!(ok);
        assert_eq!(updated, "/a/new/x.ts");
    }
}
