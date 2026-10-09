use crate::ls::prelude::*;

// Port of Go `ls/codeactions_importfixes.go`.
//
// PORT (whole file):
// - Go `program.GetTypeChecker(ctx)` is `ls_program::get_type_checker`.
//   Since ts#64543 Go acquires the checker once and passes it to the nested
//   helpers (acquisitions are not reentrant). A helper takes the caller's
//   `ch: &mut Checker` (as the w3 autoimport `View` methods do); the caller
//   borrows the lease around each call (pinned decision: the adder does not
//   store the checker).
// - Go `*autoimport.View` is `Rc<autoimport::View>`; Go `*autoimport.Fix` is
//   `Rc<autoimport::Fix>`.
// - Go `[]*fixInfo` is `Vec<FixInfo>`; the values are never changed after
//   they are made, so a copied pointer is a clone.

use crate::frontend::core_ls_ext::compare_booleans;
use std::sync::LazyLock;

// Go: ls/codeactions_importfixes.go:19 importFixErrorCodes
// PORT: `diagnostics.X.Code()` is `int32`; `Message::code` is `u32`.
static IMPORT_FIX_ERROR_CODES: LazyLock<Vec<i32>> = LazyLock::new(|| {
    vec![
        diag::Cannot_find_name_0.code() as i32,
        diag::Cannot_find_name_0_Did_you_mean_1.code() as i32,
        diag::Cannot_find_name_0_Did_you_mean_the_instance_member_this_0.code() as i32,
        diag::Cannot_find_name_0_Did_you_mean_the_static_member_1_0.code() as i32,
        diag::Cannot_find_namespace_0.code() as i32,
        diag::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead.code() as i32,
        diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here.code() as i32,
        diag::No_value_exists_in_scope_for_the_shorthand_property_0_Either_declare_one_or_provide_an_initializer.code() as i32,
        diag::X_0_cannot_be_used_as_a_value_because_it_was_imported_using_import_type.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_1_or_later.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_include_dom.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha_and_then_add_jest_or_mocha_to_the_types_field_in_your_tsconfig.code() as i32,
        diag::Cannot_find_name_0_Did_you_mean_to_write_this_in_an_async_function.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery_and_then_add_jquery_to_the_types_field_in_your_tsconfig.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode.code() as i32,
        diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig.code() as i32,
        diag::Cannot_find_namespace_0_Did_you_mean_1.code() as i32,
        diag::Cannot_extend_an_interface_0_Did_you_mean_implements.code() as i32,
        diag::This_JSX_tag_requires_0_to_be_in_scope_but_it_could_not_be_found.code() as i32,
    ]
});

// Go: ls/codeactions_importfixes.go:44 importFixID
const IMPORT_FIX_ID: &str = "fixMissingImport";

// Go: ls/codeactions_importfixes.go:48 ImportFixProvider
// ImportFixProvider is the CodeFixProvider for import-related fixes
pub static IMPORT_FIX_PROVIDER: LazyLock<CodeFixProvider> = LazyLock::new(|| CodeFixProvider {
    error_codes: IMPORT_FIX_ERROR_CODES.clone(),
    get_code_actions: get_import_code_actions,
    fix_ids: vec![IMPORT_FIX_ID.to_string()],
    get_all_code_actions: Some(get_all_import_code_actions),
});

// Go: ls/codeactions_importfixes.go:55 fixInfo
#[derive(Clone)]
struct FixInfo {
    fix: Rc<autoimport::Fix>,
    symbol_name: String,
    error_identifier_text: String,
    is_jsx_namespace_fix: bool,
}

// Go: ls/codeactions_importfixes.go:62 getImportCodeActions
fn get_import_code_actions(
    ctx: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Vec<CodeAction>, GoError> {
    // ts#64543: the checker is acquired here and passed to `getFixInfos`.
    // Go: defer done(). `_done` releases the checker at the end of the
    // function; the borrow ends after `getFixInfos`.
    let (ch, _done) = ls_program::get_type_checker(fix_context.program, ctx);

    let info = get_fix_infos(
        &mut ch.borrow_mut(),
        fix_context,
        fix_context.error_code,
        fix_context.span.pos(),
    )?;
    if info.is_empty() {
        return Ok(Vec::new());
    }

    let mut actions: Vec<CodeAction> = Vec::new();
    for fix_info in &info {
        let (edits, description, ok) = fix_info.fix.edits(
            ctx,
            fix_context.source_file,
            fix_context.program.options(),
            &fix_context.ls.format_options(),
            &fix_context.ls.converters,
            &fix_context.ls.user_preferences(),
        );

        if ok {
            actions.push(CodeAction {
                description,
                changes: edits,
                fix_id: IMPORT_FIX_ID.to_string(),
                fix_all_description: crate::diagnostics_loc::message_localize(
                    diag::Add_all_missing_imports,
                    &locale::from_context(ctx),
                    &args![],
                ),
            });
        }
    }
    Ok(actions)
}

// Go: ls/codeactions_importfixes.go:97 getAllImportCodeActions
fn get_all_import_code_actions(
    ctx: &Context,
    fix_context: &CodeFixContext<'_>,
) -> Result<Option<CombinedCodeActions>, GoError> {
    if tspath::is_dynamic_file_name(source_file_file_name(fix_context.source_file)) {
        return Ok(None);
    }

    let all_diagnostics =
        ls_program::get_semantic_diagnostics(fix_context.program, ctx, fix_context.source_file);

    let mut import_diags: Vec<Diagnostic> = Vec::new();
    for diag in all_diagnostics {
        if is_fixable_diagnostic(&diag, &IMPORT_FIX_ERROR_CODES) {
            import_diags.push(diag);
        }
    }

    if import_diags.is_empty() {
        return Ok(None);
    }

    // PORT: Go passes `ch` to the views and to `NewImportAdder`, which the
    // pinned Rust view and adder do not take. The lease is held to the end
    // of the function (Go `defer done()`); it is borrowed around each
    // `addImportFromDiagnostic` call (ts#64543 passes it there).
    let (ch, _done) = ls_program::get_type_checker(fix_context.program, ctx);

    let mut view = fix_context
        .ls
        .get_prepared_auto_import_view(fix_context.source_file)?;
    if view.is_none() {
        view = Some(
            fix_context
                .ls
                .get_current_auto_import_view(fix_context.source_file),
        );
    }
    let view = view.unwrap_or_else(|| crate::core::go_nil_dereference());

    let mut import_adder = autoimport::new_import_adder(
        ctx,
        fix_context.program,
        fix_context.source_file,
        view,
        fix_context.ls.format_options(),
        fix_context.ls.converters.clone(),
        fix_context.ls.user_preferences(),
    );

    for diag in &import_diags {
        add_import_from_diagnostic(&mut ch.borrow_mut(), &mut *import_adder, diag, fix_context)?;
    }

    if !import_adder.has_fixes() {
        return Ok(None);
    }

    Ok(Some(CombinedCodeActions {
        description: crate::diagnostics_loc::message_localize(
            diag::Add_all_missing_imports,
            &locale::from_context(ctx),
            &args![],
        ),
        changes: import_adder.edits(),
    }))
}

// Go: ls/codeactions_importfixes.go:154 addImportFromDiagnostic
// addImportFromDiagnostic finds the best import fix for a diagnostic and adds it to the adder.
fn add_import_from_diagnostic(
    ch: &mut Checker,
    import_adder: &mut dyn autoimport::ImportAdder,
    diag: &Diagnostic,
    fix_context: &CodeFixContext<'_>,
) -> Result<(), GoError> {
    let diag_fix_context = CodeFixContext {
        source_file: fix_context.source_file,
        span: TextRange::new(diag.pos(), diag.end()),
        error_code: diag.code(),
        program: fix_context.program,
        ls: fix_context.ls,
        diagnostic: None,
        params: None,
    };

    let infos = get_fix_infos(ch, &diag_fix_context, diag.code(), diag.pos())?;
    if !infos.is_empty() {
        import_adder.add_import_fix(infos[0].fix.clone());
    }
    Ok(())
}

// Go: ls/codeactions_importfixes.go:173 getFixInfos
fn get_fix_infos(
    ch: &mut Checker,
    fix_context: &CodeFixContext<'_>,
    error_code: i32,
    pos: i32,
) -> Result<Vec<FixInfo>, GoError> {
    // Can't compute import fixes for dynamic/untitled files since they don't have real file paths
    if tspath::is_dynamic_file_name(source_file_file_name(fix_context.source_file)) {
        return Ok(Vec::new());
    }

    let symbol_token = astnav::get_token_at_position(fix_context.source_file, pos);
    if error_code
        != diag::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead
            .code() as i32
        && !is_identifier(symbol_token)
    {
        return Ok(Vec::new());
    }

    let view: Option<Rc<autoimport::View>>;
    let mut info: Vec<FixInfo> = Vec::new();

    if error_code
        == diag::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead
            .code() as i32
    {
        let current_view = fix_context
            .ls
            .get_current_auto_import_view(fix_context.source_file);
        info = get_fixes_info_for_umd_import(symbol_token, &current_view, ch);
        view = Some(current_view);
    } else if error_code
        == diag::X_0_cannot_be_used_as_a_value_because_it_was_imported_using_import_type.code()
            as i32
    {
        let compiler_options = fix_context.program.options();
        let symbol_names =
            get_symbol_names_to_import(fix_context.source_file, ch, symbol_token, compiler_options);

        let mut all_type_only_fixes: Vec<FixInfo> = Vec::new();
        for sn in &symbol_names {
            if !sn.is_type_only {
                continue;
            }
            let fix =
                get_type_only_promotion_fix(fix_context.source_file, symbol_token, &sn.name, ch);
            if let Some(fix) = fix {
                all_type_only_fixes.push(FixInfo {
                    fix,
                    symbol_name: sn.name.clone(),
                    error_identifier_text: symbol_token.text().to_string(),
                    is_jsx_namespace_fix: false,
                });
            }
        }

        // For JSX opening tags, there can be separate type-only errors for both the tag name
        // identifier and the JSX namespace identifier. When both produce valid fixes, we
        // disambiguate using the diagnostic message, which quotes the symbol name in single
        // quotes (e.g., "'React' cannot be used as a value..."). If filtering yields nothing
        // (e.g., due to localization), fall back to returning all candidates.
        let mut diagnostic_message = String::new();
        if let Some(diagnostic) = fix_context.diagnostic {
            diagnostic_message = diagnostic.message.as_string();
        }
        if all_type_only_fixes.len() > 1 && !diagnostic_message.is_empty() {
            for fi in &all_type_only_fixes {
                if diagnostic_message.contains(&format!("'{}'", fi.symbol_name)) {
                    info.push(fi.clone());
                }
            }
        }
        if info.is_empty() {
            info = all_type_only_fixes;
        }
        return Ok(info);
    } else {
        view = fix_context
            .ls
            .get_prepared_auto_import_view(fix_context.source_file)?;
        if let Some(prepared_view) = &view {
            info = get_fixes_info_for_non_umd_import(fix_context, symbol_token, prepared_view, ch);
        }
    }

    // Sort fixes by preference
    let view = match view {
        Some(view) => view,
        None => fix_context
            .ls
            .get_current_auto_import_view(fix_context.source_file),
    };
    Ok(sort_fix_info(info, fix_context, &view))
}

// Go: ls/codeactions_importfixes.go:243 getFixesInfoForUMDImport
fn get_fixes_info_for_umd_import(
    token: Node,
    view: &autoimport::View,
    ch: &mut Checker,
) -> Vec<FixInfo> {
    let umd_symbol = get_umd_symbol(token, ch);
    if umd_symbol.is_nil() {
        return Vec::new();
    }

    let export = autoimport::symbol_to_export(umd_symbol, ch);
    let is_valid_type_only_use_site = is_valid_type_only_alias_use_site(token);

    // PORT: Go passes a nil `*Export` on to `GetFixes`, which dereferences
    // it; here the dereference is at this point.
    let export = export.unwrap_or_else(|| crate::core::go_nil_dereference());

    let mut result: Vec<FixInfo> = Vec::new();
    for fix in view.get_fixes(ch, &export, false, is_valid_type_only_use_site, None) {
        let mut error_identifier_text = String::new();
        if is_identifier(token) {
            error_identifier_text = token.text().to_string();
        }
        result.push(FixInfo {
            fix,
            symbol_name: ch.sym(umd_symbol).name.as_str().to_string(),
            error_identifier_text,
            is_jsx_namespace_fix: false,
        });
    }
    result
}

// Go: ls/codeactions_importfixes.go:267 getUmdSymbol
fn get_umd_symbol(token: Node, ch: &mut Checker) -> SymbolId {
    // try the identifier to see if it is the umd symbol
    let mut umd_symbol = SymbolId::NIL;
    if is_identifier(token) {
        umd_symbol = ch.get_resolved_symbol_exported(token);
    }
    if is_umd_export_symbol(&ch.symbols, umd_symbol) {
        return umd_symbol;
    }

    // The error wasn't for the symbolAtLocation, it was for the JSX tag itself, which needs access to e.g. `React`.
    let parent = token.parent();
    if (is_jsx_opening_like_element(parent) && parent.tag_name() == token)
        || is_jsx_opening_fragment(parent)
    {
        let location = if is_jsx_opening_like_element(parent) {
            token
        } else {
            parent
        };
        let jsx_namespace = ch.get_jsx_namespace_exported(parent);
        let parent_symbol = ch.resolve_name_exported(
            &jsx_namespace,
            location,
            SymbolFlags::VALUE,
            false, /* excludeGlobals */
        );
        if is_umd_export_symbol(&ch.symbols, parent_symbol) {
            return parent_symbol;
        }
    }
    SymbolId::NIL
}

// Go: ls/codeactions_importfixes.go:296 isUMDExportSymbol
// PORT: Go reads the symbol through its pointer; here through the arena.
fn is_umd_export_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    symbol.is_some()
        && !symbols.sym(symbol).declarations.is_empty()
        && symbols.sym(symbol).declarations[0].is_some()
        && is_namespace_export_declaration(symbols.sym(symbol).declarations[0])
}

// Go: ls/codeactions_importfixes.go:302 getFixesInfoForNonUMDImport
fn get_fixes_info_for_non_umd_import(
    fix_context: &CodeFixContext<'_>,
    symbol_token: Node,
    view: &autoimport::View,
    ch: &mut Checker,
) -> Vec<FixInfo> {
    let compiler_options = fix_context.program.options();

    let is_valid_type_only_use_site = is_valid_type_only_alias_use_site(symbol_token);
    let symbol_names =
        get_symbol_names_to_import(fix_context.source_file, ch, symbol_token, compiler_options);
    let mut all_info: Vec<FixInfo> = Vec::new();

    // Compute usage position for JSDoc import type fixes
    let (usage_position, fidelity) = fix_context.ls.converters.to_lsp_position(
        &fix_context.source_file,
        get_token_pos_of_node(symbol_token, fix_context.source_file, false),
    );
    if !fidelity.is_exact() {
        return Vec::new();
    }

    for sn in &symbol_names {
        // Type-only imports are handled by the promotion code path, not the auto-import path.
        if sn.is_type_only {
            continue;
        }

        let symbol_name = &sn.name;
        // "default" is a keyword and not a legal identifier for the import
        if symbol_name == "default" {
            continue;
        }

        let is_jsx_tag_name = symbol_name == symbol_token.text() && is_jsx_tag_name(symbol_token);
        let mut query_kind = autoimport::QueryKind::EXACT_MATCH;
        if is_jsx_tag_name {
            query_kind = autoimport::QueryKind::CASE_INSENSITIVE_MATCH;
        }

        let exports = view.search_exported(symbol_name, query_kind);
        for export in &exports {
            if is_jsx_tag_name && !(export.name() == *symbol_name || export.is_renameable()) {
                continue;
            }

            let fixes = view.get_fixes(
                ch,
                export,
                is_jsx_tag_name,
                is_valid_type_only_use_site,
                Some(usage_position),
            );
            for fix in fixes {
                all_info.push(FixInfo {
                    fix,
                    symbol_name: symbol_name.clone(),
                    error_identifier_text: String::new(),
                    is_jsx_namespace_fix: symbol_name != symbol_token.text(),
                });
            }
        }
    }

    all_info
}

// Go: ls/codeactions_importfixes.go:353 getTypeOnlyPromotionFix
fn get_type_only_promotion_fix(
    source_file: Node,
    symbol_token: Node,
    symbol_name: &str,
    ch: &mut Checker,
) -> Option<Rc<autoimport::Fix>> {
    // Get the symbol at the token location
    let symbol = ch.resolve_name_exported(
        symbol_name,
        symbol_token,
        SymbolFlags::VALUE,
        true, /* excludeGlobals */
    );
    if symbol.is_nil() {
        return None;
    }

    // Get the type-only alias declaration
    let type_only_alias_declaration = ch.get_type_only_alias_declaration_exported(symbol);
    if type_only_alias_declaration.is_nil()
        || get_source_file_of_node(type_only_alias_declaration) != source_file
    {
        return None;
    }

    Some(Rc::new(autoimport::Fix {
        auto_import_fix: lsproto::AutoImportFix {
            kind: lsproto::AutoImportFixKind::PROMOTE_TYPE_ONLY,
            ..Default::default()
        },
        type_only_alias_declaration,
        ..Default::default()
    }))
}

// Go: ls/codeactions_importfixes.go:374 symbolNameInfo
struct SymbolNameInfo {
    name: String,
    is_type_only: bool, // whether the symbol currently resolves to a type-only import
}

// Go: ls/codeactions_importfixes.go:379 getSymbolNamesToImport
fn get_symbol_names_to_import(
    source_file: Node,
    ch: &mut Checker,
    symbol_token: Node,
    compiler_options: &CompilerOptions,
) -> Vec<SymbolNameInfo> {
    let parent = symbol_token.parent();
    if (is_jsx_opening_like_element(parent) || is_jsx_closing_element(parent))
        && parent.tag_name() == symbol_token
        && jsx_mode_needs_explicit_import(compiler_options.jsx)
    {
        let jsx_namespace = ch.get_jsx_namespace_exported(source_file);
        if needs_jsx_namespace_fix(&jsx_namespace, symbol_token, ch) {
            let mut result: Vec<SymbolNameInfo> = Vec::new();
            if !is_intrinsic_jsx_name(symbol_token.text()) {
                let comp_symbol = ch.resolve_name_exported(
                    symbol_token.text(),
                    symbol_token,
                    SymbolFlags::VALUE,
                    false, /* excludeGlobals */
                );
                if comp_symbol.is_nil() {
                    result.push(SymbolNameInfo {
                        name: symbol_token.text().to_string(),
                        is_type_only: false,
                    });
                } else if ch
                    .get_type_only_alias_declaration_exported(comp_symbol)
                    .is_some()
                {
                    result.push(SymbolNameInfo {
                        name: symbol_token.text().to_string(),
                        is_type_only: true,
                    });
                }
            }
            let mut ns_is_type_only = false;
            let ns_symbol = ch.resolve_name_exported(
                &jsx_namespace,
                symbol_token,
                SymbolFlags::VALUE,
                true, /* excludeGlobals */
            );
            if ns_symbol.is_some() {
                ns_is_type_only = ch
                    .get_type_only_alias_declaration_exported(ns_symbol)
                    .is_some();
            }
            result.push(SymbolNameInfo {
                name: jsx_namespace,
                is_type_only: ns_is_type_only,
            });
            return result;
        }
    }
    let mut token_is_type_only = false;
    let sym = ch.resolve_name_exported(
        symbol_token.text(),
        symbol_token,
        SymbolFlags::VALUE,
        true, /* excludeGlobals */
    );
    if sym.is_some() {
        token_is_type_only = ch.get_type_only_alias_declaration_exported(sym).is_some();
    }
    vec![SymbolNameInfo {
        name: symbol_token.text().to_string(),
        is_type_only: token_is_type_only,
    }]
}

// Go: ls/codeactions_importfixes.go:410 needsJsxNamespaceFix
fn needs_jsx_namespace_fix(jsx_namespace: &str, symbol_token: Node, ch: &mut Checker) -> bool {
    if is_intrinsic_jsx_name(symbol_token.text()) {
        return true;
    }
    let namespace_symbol = ch.resolve_name_exported(
        jsx_namespace,
        symbol_token,
        SymbolFlags::VALUE,
        true, /* excludeGlobals */
    );
    if namespace_symbol.is_nil() {
        return true;
    }
    if ch
        .sym(namespace_symbol)
        .declarations
        .iter()
        .any(|&d| is_type_only_import_or_export_declaration(d))
    {
        return !ch
            .sym(namespace_symbol)
            .flags
            .intersects(SymbolFlags::VALUE);
    }
    false
}

// Go: ls/codeactions_importfixes.go:424 jsxModeNeedsExplicitImport
fn jsx_mode_needs_explicit_import(jsx: JsxEmit) -> bool {
    jsx == JsxEmit::REACT || jsx == JsxEmit::REACT_NATIVE
}

// Go: ls/codeactions_importfixes.go:428 sortFixInfo
fn sort_fix_info(
    fixes: Vec<FixInfo>,
    fix_context: &CodeFixContext<'_>,
    view: &autoimport::View,
) -> Vec<FixInfo> {
    if fixes.is_empty() {
        return fixes;
    }

    // Create a copy to avoid modifying the original
    let mut sorted: Vec<FixInfo> = fixes.clone();

    // Sort by:
    // 1. JSX namespace fixes last
    // 2. Fix comparison using view.CompareFixes
    crate::gostd::slices::sort_func(&mut sorted, |a: &FixInfo, b: &FixInfo| -> i32 {
        // JSX namespace fixes should come last
        let cmp = compare_booleans(a.is_jsx_namespace_fix, b.is_jsx_namespace_fix);
        if cmp != 0 {
            return cmp;
        }
        view.compare_fixes_for_sorting(&a.fix, &b.fix)
    });

    sorted
}
