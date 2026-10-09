//! Port of Go `ls/rename.go`.
//!
//! PORT: Go `*compiler.Program` is `&compiler::NewProgram`. A Go
//! checker lease `ch, done := program.GetTypeChecker(ctx)` is
//! `ls_program::get_type_checker`, with `done` kept alive to the end of the
//! scope (Go `defer done()`).
//! PORT: `symbolAndEntriesToRename` and what it calls are generic over the
//! program (`ProgramView`, program_view.rs) so that a cross-project rename
//! can run them on a search thread; there the lease is
//! `ProgramView::get_type_checker`.

use crate::ls::prelude::*;

use crate::diagnostics::Message;
use crate::spanmap::Feature;

// Go: ls/rename.go:25 RenameInfo
// RenameInfo represents the result of a rename validation check.
// It is used by the `textDocument/prepareRename` LSP handler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenameInfo {
    pub can_rename: bool,
    pub localized_error_message: String,
    pub display_name: String,
    pub trigger_span: lsproto::Range,
    pub file_to_rename: String,
    pub new_file_name: String,
}

// Go: ls/rename.go:34 mappedRenameEdit
struct MappedRenameEdit {
    uri: lsproto::DocumentUri,
    edit: lsproto::TextEdit,
}

// Go: ls/rename.go:39 renameEditKey
#[derive(PartialEq, Eq, Hash)]
struct RenameEditKey {
    uri: lsproto::DocumentUri,
    text_range: lsproto::Range,
}

// Go: ls/rename.go:44 deduplicateRenameEdits
// PORT: Go returns `(nil, false)` on a conflict; here `None`. Go map order
// is random, so the IndexMap keeps first-insert order, as the old
// `changes` map of `symbolAndEntriesToRename` did.
fn deduplicate_rename_edits(
    mapped_edits: Vec<MappedRenameEdit>,
) -> Option<IndexMap<lsproto::DocumentUri, Vec<Option<lsproto::TextEdit>>>> {
    let mut edit_texts: FxHashMap<RenameEditKey, String> = FxHashMap::default();
    let mut unique_edits: Vec<MappedRenameEdit> = Vec::with_capacity(mapped_edits.len());
    for mapped_edit in mapped_edits {
        let key = RenameEditKey {
            uri: mapped_edit.uri.clone(),
            text_range: mapped_edit.edit.range,
        };
        if let Some(existing_text) = edit_texts.get(&key) {
            if *existing_text != mapped_edit.edit.new_text {
                return None;
            }
            continue;
        }
        edit_texts.insert(key, mapped_edit.edit.new_text.clone());
        unique_edits.push(mapped_edit);
    }
    let mut changes: IndexMap<lsproto::DocumentUri, Vec<Option<lsproto::TextEdit>>> =
        IndexMap::new();
    for mapped_edit in unique_edits {
        changes
            .entry(mapped_edit.uri)
            .or_default()
            .push(Some(mapped_edit.edit));
    }
    Some(changes)
}

impl LanguageService {
    // Go: ls/rename.go:65 ProvideRename
    // PORT: Go `orchestrator CrossProjectOrchestrator` is an interface value
    // that can be nil: `Option<&dyn CrossProjectOrchestrator>`.
    pub fn provide_rename(
        &self,
        ctx: &Context,
        params: &lsproto::RenameParams,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::WorkspaceEditOrNull, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_rename,
            Some(start_search::<RenameSearch>),
            combine_rename_response,
            true,  /*isRename*/
            false, /*implementations*/
            SymbolEntryTransformOptions::default(),
            None, /*defaultProjectData*/
        )
    }

    // Go: ls/rename.go:79 GetRenameInfo
    pub fn get_rename_info(
        &self,
        ctx: &Context,
        new_name: &str,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> RenameInfo {
        let (program, source_file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            source_file,
            position,
            Feature::RENAME,
        );
        for mapped in &positions {
            if !mapped.fidelity.is_exact() {
                continue;
            }
            let source_file = mapped.script;
            let node = astnav::get_touching_property_name(source_file, mapped.position);
            let node = get_adjusted_location(node, true /*forRename*/, source_file);
            if node_is_eligible_for_rename(node) {
                let (rename_info, ok) =
                    self.get_rename_info_for_node(ctx, new_name, node, source_file, program);
                if ok {
                    return rename_info;
                }
            }
        }
        get_rename_info_error(ctx, diag::You_cannot_rename_this_element)
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/rename.go:98 symbolAndEntriesToRename
    pub fn symbol_and_entries_to_rename(
        &self,
        ctx: &Context,
        params: &lsproto::RenameParams,
        data: SymbolAndEntriesData,
        _options: SymbolEntryTransformOptions,
    ) -> Result<lsproto::WorkspaceEditOrNull, GoError> {
        if !node_is_eligible_for_rename(data.original_node) {
            return Ok(lsproto::WorkspaceEditOrNull::default());
        }

        let program = self.get_program();

        // Defense-in-depth: validate rename eligibility even if the client skipped prepareRename.
        // Use getRenameInfoForNode directly with the already-resolved node to avoid
        // re-resolving the position and polluting state baselines.
        let source_file = get_source_file_of_node(data.original_node);
        let (info, ok) = self.get_rename_info_for_node(
            ctx,
            &params.new_name,
            data.original_node,
            source_file,
            program,
        );
        if !ok || !info.can_rename {
            return Ok(lsproto::WorkspaceEditOrNull::default());
        }

        let entries: Vec<Rc<RefCell<ReferenceEntry>>> = data
            .symbols_and_entries
            .iter()
            .flat_map(|s| s.borrow().references.clone())
            .collect();
        let mut mapped_edits: Vec<MappedRenameEdit> = Vec::new();
        // Go: `defer done()`; `_done` releases the lease at the end of the scope.
        let (checker, _done) = program.get_type_checker(ctx);
        let ch = &mut *checker.borrow_mut();

        let quote_preference = lsutil::get_quote_preference(source_file, &self.user_preferences());
        let use_aliases_for_rename = self
            .user_preferences()
            .use_aliases_for_rename
            .is_true_or_unknown();

        for entry in &entries {
            let uri = self.get_file_name_of_entry(entry);
            let entry_node = entry.borrow().node;
            if self.user_preferences().allow_rename_of_import_path != Tristate::True
                && entry_node.is_some()
                && is_string_literal_like(entry_node)
                && try_get_import_from_module_specifier(entry_node).is_some()
            {
                continue;
            }
            let (rng, ok) = self.rename_edit_range(entry);
            if !ok {
                // The occurrence lies outside a verbatim span of a content-mapped file, so it cannot be
                // written back to the original text. Skip it and keep renaming the remaining occurrences.
                continue;
            }
            let text_edit = lsproto::TextEdit {
                range: rng,
                new_text: self.get_text_for_rename(
                    data.original_node,
                    entry,
                    &params.new_name,
                    ch,
                    quote_preference,
                    use_aliases_for_rename,
                ),
            };
            mapped_edits.push(MappedRenameEdit {
                uri,
                edit: text_edit,
            });
        }
        let Some(changes) = deduplicate_rename_edits(mapped_edits) else {
            return Ok(lsproto::WorkspaceEditOrNull::default());
        };
        Ok(lsproto::WorkspaceEditOrNull {
            workspace_edit: Some(lsproto::WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
        })
    }

    // Go: ls/rename.go:153 renameEditRange
    // renameEditRange returns the LSP range at which a rename occurrence should be edited. For occurrences in
    // content-mapped files it maps the transformed range strictly, returning ok=false when the occurrence is
    // not fully within a single verbatim span, so the caller can skip an edit that cannot be applied to the
    // original text.
    pub fn rename_edit_range(&self, entry: &Rc<RefCell<ReferenceEntry>>) -> (lsproto::Range, bool) {
        self.resolve_entry(entry);
        let (node, entry_source_file, text_range) = {
            let e = entry.borrow();
            (
                e.node,
                e.source_file,
                e.text_range
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
            )
        };
        if node.is_nil() {
            let (location, fidelity) =
                self.source_file_range_to_lsp_location(entry_source_file, text_range);
            return (location.range, fidelity.is_exact());
        }
        let source_file = get_source_file_of_node(node);
        // PORT: Go `sourceFile == nil || sourceFile.SpanMap() == nil`; the
        // span map of a nil file is `None`.
        if source_file_span_map(source_file).is_none() {
            return (self.get_range_of_entry(entry), true);
        }
        let (lsp_range, fidelity) = self.converters.to_lsp_range(&source_file, text_range);
        (lsp_range, fidelity.is_exact())
    }

    // Go: ls/rename.go:168 getRenameInfoForNode
    // getRenameInfoForNode performs detailed validation for a rename operation on a specific node.
    pub fn get_rename_info_for_node(
        &self,
        ctx: &Context,
        new_name: &str,
        node: Node,
        source_file: Node,
        program: &P,
    ) -> (RenameInfo, bool) {
        // Go: `defer done()`; `_done` releases the lease at the end of the scope.
        let (checker, _done) = program.get_type_checker(ctx);
        let ch = &mut *checker.borrow_mut();

        let symbol = ch.get_symbol_at_location_exported(node);
        if symbol.is_nil() {
            if is_string_literal_like(node) {
                // Allow renaming of string literal types with contextual string literal types
                let typ = get_contextual_type_from_parent_or_ancestor_type_node(node, ch);
                if typ.is_some()
                    && (ch.ty(typ).is_string_literal()
                        || (ch.ty(typ).is_union()
                            && ch
                                .ty(typ)
                                .types()
                                .iter()
                                .all(|&t| ch.ty(t).is_string_literal())))
                {
                    return (
                        get_rename_info_success(node, source_file, node.text(), &self.converters),
                        true,
                    );
                }
            } else if is_label_name(node) {
                let name = node.text();
                return (
                    get_rename_info_success(node, source_file, name, &self.converters),
                    true,
                );
            }
            return (RenameInfo::default(), false);
        }

        // Only allow a symbol to be renamed if it actually has at least one declaration.
        if ch.sym(symbol).declarations.is_empty() {
            return (RenameInfo::default(), false);
        }

        if let Some(msg) = self.rename_blocked_reason(source_file, node, symbol, ch, program) {
            return (get_rename_info_error(ctx, msg), true);
        }

        if is_string_literal_like(node) && try_get_import_from_module_specifier(node).is_some() {
            if self
                .user_preferences()
                .allow_rename_of_import_path
                .is_true()
            {
                return self.get_rename_info_for_module(
                    &ch.symbols,
                    ctx,
                    new_name,
                    node,
                    source_file,
                    symbol,
                );
            }
            return (RenameInfo::default(), false);
        }

        let display_name = ch.symbol_to_string_exported(symbol);
        (
            get_rename_info_success(node, source_file, &display_name, &self.converters),
            true,
        )
    }
}

/// `ProvideRename` as a `CrossProjectSearch`: its searches in other projects
/// run on search threads.
pub struct RenameSearch;

impl CrossProjectSearch for RenameSearch {
    type Req = lsproto::RenameParams;
    type Resp = lsproto::WorkspaceEditOrNull;

    fn to_resp<P: ProgramView>(
        ls: &LanguageService<P>,
        ctx: &Context,
        params: &Self::Req,
        data: SymbolAndEntriesData,
        options: SymbolEntryTransformOptions,
    ) -> Result<Self::Resp, GoError> {
        ls.symbol_and_entries_to_rename(ctx, params, data, options)
    }
}

// Go: ls/rename.go:209 nodeIsEligibleForRename
pub fn node_is_eligible_for_rename(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    match node.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::PrivateIdentifier
        | SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::ThisKeyword => true,
        SyntaxKind::NumericLiteral => is_literal_name_of_property_declaration_or_index_access(node),
        _ => false,
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/rename.go:229 renameBlockedReason
    // renameBlockedReason returns a non-nil diagnostic message if the rename should be blocked
    // because the symbol is a library definition, a default keyword, or would cross node_modules boundaries.
    pub fn rename_blocked_reason(
        &self,
        source_file: Node,
        node: Node,
        symbol: SymbolId,
        ch: &mut Checker,
        program: &P,
    ) -> Option<&'static Message> {
        for &declaration in ch.sym(symbol).declarations.iter() {
            if is_defined_in_library_file(program, declaration) {
                return Some(
                    diag::You_cannot_rename_elements_that_are_defined_in_the_standard_TypeScript_library,
                );
            }
        }

        // Cannot rename `default` as in `import { default as foo } from "./someModule"`
        let symbol_parent = ch.sym(symbol).parent;
        if is_identifier(node)
            && node.text() == "default"
            && symbol_parent.is_some()
            && ch.sym(symbol_parent).flags.intersects(SymbolFlags::MODULE)
        {
            return Some(diag::You_cannot_rename_this_element);
        }

        if let Some(msg) =
            would_rename_in_other_node_modules(source_file, symbol, ch, &self.user_preferences())
        {
            return Some(msg);
        }

        None
    }
}

// Go: ls/rename.go:249 isDefinedInLibraryFile
// isDefinedInLibraryFile checks if a declaration is from a default library file (e.g., lib.d.ts).
pub fn is_defined_in_library_file<P: ProgramView>(program: &P, declaration: Node) -> bool {
    let decl_source_file = get_source_file_of_node(declaration);
    program.is_source_file_default_library(&tspath::Path(
        source_file_info(decl_source_file).path.clone(),
    )) && tspath::is_declaration_file_name(source_file_file_name(decl_source_file))
}

// Go: ls/rename.go:255 wouldRenameInOtherNodeModules
// wouldRenameInOtherNodeModules checks if renaming the symbol would affect node_modules.
pub fn would_rename_in_other_node_modules(
    original_file: Node,
    symbol: SymbolId,
    ch: &mut Checker,
    preferences: &lsutil::UserPreferences,
) -> Option<&'static Message> {
    let mut sym = symbol;
    if !preferences.use_aliases_for_rename.is_true_or_unknown()
        && ch.sym(sym).flags.intersects(SymbolFlags::ALIAS)
    {
        let import_specifier = ch
            .sym(sym)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_import_specifier(d))
            .unwrap_or(Node::NIL);
        if import_specifier.is_some() && import_specifier.property_name().is_nil() {
            sym = ch.get_aliased_symbol(sym);
        }
    }

    let declarations = ch.sym(sym).declarations.to_vec();
    if declarations.is_empty() {
        return None;
    }

    let original_package =
        module::node_module_package_root_for_file(source_file_file_name(original_file));
    if original_package.is_empty() {
        // Original source file is not in node_modules.
        for &declaration in &declarations {
            if is_inside_node_modules(source_file_file_name(get_source_file_of_node(declaration))) {
                return Some(
                    diag::You_cannot_rename_elements_that_are_defined_in_a_node_modules_folder,
                );
            }
        }
        return None;
    }

    // Original source file is in node_modules.
    for &declaration in &declarations {
        let decl_package = module::node_module_package_root_for_file(source_file_file_name(
            get_source_file_of_node(declaration),
        ));
        if !decl_package.is_empty() && decl_package != original_package {
            return Some(
                diag::You_cannot_rename_elements_that_are_defined_in_another_node_modules_folder,
            );
        }
    }
    None
}

// Go: ls/rename.go:290 ClientSupportsWillRenameFiles
pub fn client_supports_will_rename_files(ctx: &Context) -> bool {
    lsproto::get_client_capabilities(ctx)
        .workspace
        .file_operations
        .will_rename
}

// Go: ls/rename.go:294 ClientSupportsDocumentChanges
pub fn client_supports_document_changes(ctx: &Context) -> bool {
    lsproto::get_client_capabilities(ctx)
        .workspace
        .workspace_edit
        .document_changes
}

// Go: ls/rename.go:298 ClientSupportsRenameResourceOperations
pub fn client_supports_rename_resource_operations(ctx: &Context) -> bool {
    lsproto::get_client_capabilities(ctx)
        .workspace
        .workspace_edit
        .resource_operations
        .contains(&lsproto::ResourceOperationKind::RENAME)
}

// Go: ls/rename.go:346 tryRemoveIndexFileName (ts#64159)
// The directory of an index file, or "" when the file is not an index file
// or is the index file in the root of the file system or of a dynamic name.
pub fn try_remove_index_file_name(file_name: &str) -> String {
    if tspath::remove_file_extension(&tspath::get_base_file_name(file_name)) == "index" {
        let root_length = tspath::get_root_length(file_name);
        let (root, relative) = file_name.split_at(root_length);
        if (root == "/" || tspath::is_dynamic_file_name(root))
            && tspath::remove_file_extension(relative) == "index"
        {
            return String::new();
        }
        return tspath::get_directory_path(file_name);
    }
    String::new()
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/rename.go:303 getRenameInfoForModule
    // getRenameInfoForModule handles rename validation for module specifiers.
    // PORT: Go reads `moduleSymbol.Declarations` without a checker; the
    // symbol arena is an extra first parameter, as for ast helpers.
    pub fn get_rename_info_for_module(
        &self,
        symbols: &SymbolArena,
        ctx: &Context,
        new_name: &str,
        specifier: Node,
        source_file: Node,
        module_symbol: SymbolId,
    ) -> (RenameInfo, bool) {
        if !tspath::is_external_module_name_relative(specifier.text()) {
            return (
                get_rename_info_error(ctx, diag::You_cannot_rename_a_module_via_a_global_import),
                true,
            );
        }
        if !client_supports_document_changes(ctx)
            || !client_supports_rename_resource_operations(ctx)
        {
            return (
                get_rename_info_error(ctx, diag::File_rename_is_not_supported_by_the_editor),
                true,
            );
        }

        let module_source_file = symbols
            .sym(module_symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_source_file(d))
            .unwrap_or(Node::NIL);
        if module_source_file.is_nil() {
            return (RenameInfo::default(), false);
        }

        let file_name = source_file_file_name(module_source_file);
        let mut without_index = String::new();
        if !specifier.text().ends_with("/index") && !specifier.text().ends_with("/index.js") {
            without_index = try_remove_index_file_name(file_name);
        }

        let mut display_name = file_name.to_string();
        if !without_index.is_empty() {
            display_name = without_index;
        }
        let new_file_name =
            self.get_new_file_name_for_module_rename(&display_name, specifier.text(), new_name);

        // Span should only be the last component of the path. + 1 to account for the quote character.
        // PORT: Go `strings.LastIndex(..) + 1` is 0 when "/" is absent.
        let index_after_last_slash = specifier.text().rfind('/').map_or(0, |i| i + 1);
        let start = astnav::get_start_of_node(specifier, source_file, false /*includeJSDoc*/)
            + 1
            + index_after_last_slash as i32;
        let length = specifier.text().len() as i32 - index_after_last_slash as i32;

        let (trigger_span, fidelity) = self
            .converters
            .to_lsp_range(&source_file, TextRange::new(start, start + length));
        if !fidelity.is_exact() {
            return (RenameInfo::default(), false);
        }
        (
            RenameInfo {
                can_rename: true,
                display_name: specifier.text()[index_after_last_slash..].to_string(),
                trigger_span,
                file_to_rename: display_name,
                new_file_name,
                ..Default::default()
            },
            true,
        )
    }

    // Go: ls/rename.go:359 getNewFileNameForModuleRename
    // Adjust the new name based on the old path that an import specifier resolves to.
    // For example, if specifier "a.js" resolves to file a.ts, renaming "a.js" -> "b.js" should mean file rename a.ts -> b.ts.
    pub fn get_new_file_name_for_module_rename(
        &self,
        old_path: &str,
        specifier_text: &str,
        new_name: &str,
    ) -> String {
        // ts#64159: `oldPath.Directory().ResolveFile(newName)` (Go N'
        // rename.go:361), so the new name is normalized.
        let directory = tspath::get_directory_path(old_path);
        let mut new_path = if new_name.is_empty() {
            directory
        } else {
            tspath::get_normalized_absolute_path(new_name, &directory)
        };
        let ignore_case = !self.host.use_case_sensitive_file_names();
        let old_ext = if tspath::is_declaration_file_name(old_path) {
            tspath::get_declaration_file_extension(old_path)
        } else {
            tspath::get_any_extension_from_path(old_path, &[] /*extensions*/, ignore_case)
        };
        if !tspath::has_extension(&new_path) {
            new_path = new_path + &old_ext;
        } else if tspath::get_any_extension_from_path(
            &new_path,
            &[], /*extensions*/
            ignore_case,
        ) == tspath::get_any_extension_from_path(
            specifier_text,
            &[], /*extensions*/
            ignore_case,
        ) {
            new_path = tspath::change_any_extension(
                &new_path,
                &old_ext,
                &[], /*extensions*/
                ignore_case,
            );
        }
        new_path
    }

    // Go: ls/rename.go:377 getTextForRename
    // PORT: Go `entry *ReferenceEntry` is the shared entry handle.
    pub fn get_text_for_rename(
        &self,
        original_node: Node,
        entry: &Rc<RefCell<ReferenceEntry>>,
        new_text: &str,
        ch: &mut Checker,
        quote_preference: lsutil::QuotePreference,
        use_aliases_for_rename: bool,
    ) -> String {
        let (entry_kind, entry_node) = {
            let entry = entry.borrow();
            (entry.kind, entry.node)
        };
        if use_aliases_for_rename
            && entry_kind != EntryKind::RANGE
            && (is_identifier(original_node) || is_string_literal_like(original_node))
        {
            let node = get_reparsed_node_for_node(entry_node);
            let kind = entry_kind;
            let parent = node.parent();
            let name = original_node.text();
            let is_shorthand_assignment = is_shorthand_property_assignment(parent);
            if is_shorthand_assignment
                || (is_object_binding_element_without_property_name(parent)
                    && parent.name() == node
                    && parent.dot_dot_dot_token().is_nil())
            {
                if kind == EntryKind::SEARCHED_LOCAL_FOUND_PROPERTY {
                    return format!("{name}: {new_text}");
                }
                if kind == EntryKind::SEARCHED_PROPERTY_FOUND_LOCAL {
                    return format!("{new_text}: {name}");
                }
                // In `const o = { x }; o.x`, symbolAtLocation at `x` in `{ x }` is the property symbol.
                // For a binding element `const { x } = o;`, symbolAtLocation at `x` is the property symbol.
                if is_shorthand_assignment {
                    let grand_parent = parent.parent();
                    if is_object_literal_expression(grand_parent)
                        && is_binary_expression(grand_parent.parent())
                        && is_module_exports_access_expression(grand_parent.parent().left())
                    {
                        return format!("{name}: {new_text}");
                    }
                    return format!("{new_text}: {name}");
                }
                return format!("{name}: {new_text}");
            } else if is_import_specifier(parent) && parent.property_name().is_nil() {
                // If the original symbol was using this alias, just rename the alias.
                let original_symbol = if is_export_specifier(original_node.parent()) {
                    ch.get_export_specifier_local_target_symbol(original_node.parent())
                } else {
                    ch.get_symbol_at_location_exported(original_node)
                };
                if original_symbol.is_some()
                    && ch.sym(original_symbol).declarations.contains(&parent)
                {
                    return format!("{name} as {new_text}");
                }
                return new_text.to_string();
            } else if is_export_specifier(parent) && parent.property_name().is_nil() {
                // If the symbol for the node is same as declared node symbol use prefix text
                if original_node == entry_node
                    || ch.get_symbol_at_location_exported(original_node)
                        == ch.get_symbol_at_location_exported(entry_node)
                {
                    return format!("{name} as {new_text}");
                }
                return format!("{new_text} as {name}");
            }
        }

        // If the node is a numerical indexing literal, then add quotes around the property access.
        if entry_kind != EntryKind::RANGE
            && is_numeric_literal(entry_node)
            && is_access_expression(entry_node.parent())
        {
            let quote = get_quote_from_preference(quote_preference);
            return format!("{quote}{new_text}{quote}");
        }

        new_text.to_string()
    }
}

// Go: ls/rename.go:423 getQuoteFromPreference
pub fn get_quote_from_preference(quote_preference: lsutil::QuotePreference) -> &'static str {
    if quote_preference == lsutil::QuotePreference::SINGLE {
        return "'";
    }
    "\""
}

// Go: ls/rename.go:430 getRenameInfoError
pub fn get_rename_info_error(ctx: &Context, message: &'static Message) -> RenameInfo {
    RenameInfo {
        can_rename: false,
        localized_error_message: crate::diagnostics_loc::message_localize(
            message,
            &locale::from_context(ctx),
            &Vec::new(),
        ),
        ..Default::default()
    }
}

// Go: ls/rename.go:437 getRenameInfoSuccess
pub fn get_rename_info_success(
    node: Node,
    source_file: Node,
    display_name: &str,
    converters: &lsconv::Converters,
) -> RenameInfo {
    let mut start = astnav::get_start_of_node(node, source_file, false /*includeJSDoc*/);
    let mut end = node.end();
    if is_string_literal_like(node) {
        // Exclude the quotes
        start += 1;
        // PORT: Go `end--` steps back one Go byte (`go_offset_before`). An
        // unterminated literal can end in a marker unit (see
        // `GO_STRING_MARKER`), which has more port bytes than Go bytes.
        end = go_offset_before(&source_file_text(source_file), end);
    }
    let (trigger_span, fidelity) =
        converters.to_lsp_range(&source_file, TextRange::new(start, end));
    if !fidelity.is_exact() {
        return RenameInfo {
            can_rename: false,
            ..Default::default()
        };
    }
    RenameInfo {
        can_rename: true,
        display_name: display_name.to_string(),
        trigger_span,
        ..Default::default()
    }
}
