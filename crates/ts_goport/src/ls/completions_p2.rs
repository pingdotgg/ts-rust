use crate::ls::prelude::*;

// Port of Go `ls/completions.go` lines 1709-3498: completion entry building,
// keyword lists and the context helpers.
//
// PORT (whole file):
// - Go text scanning (`utf8.DecodeRuneInString`, `utf8.DecodeLastRuneInString`,
//   `unicode.IsSpace`, `unicode.IsDigit`) runs on byte offsets with the
//   scanner's rune decoders. A Go `rune` is an `i32`.
// - Go `*ast.Symbol` reads without a checker take the symbol arena as the
//   first parameter (`symbols: &SymbolArena`), as the ast helpers do.
// - Go `*checker.Type` is `TypeId`; its methods read the checker arena
//   (`type_checker.ty(t)`).
// - Go `*symbolOriginInfo` parameters are `Option<&SymbolOriginInfo>`.
// - Go `collections.Set[string]` results are `FxHashSet<String>`.

use crate::astnav;
use crate::frontend::json_ext;
use crate::frontend::scanner::scanner_p1::{
    RUNE_ERROR, utf8_decode_last_rune_in_string, utf8_decode_rune_in_string,
};
use crate::frontend::stringutil_ls;
use crate::gostd::{Context, GoError, unicode};
use crate::ls::lsutil;
use crate::lsp::lsproto;

// Go: ls/completions.go:1801 keywordCompletionData
pub fn keyword_completion_data(
    keyword_filters: KeywordCompletionFilters,
    filter_out_ts_only_keywords: bool,
    is_new_identifier_location: bool,
) -> CompletionDataKeyword {
    CompletionDataKeyword {
        keyword_completions: get_keyword_completions(keyword_filters, filter_out_ts_only_keywords),
        is_new_identifier_location,
    }
}

// Go: ls/completions.go:1812 getDefaultCommitCharacters
pub fn get_default_commit_characters(is_new_identifier_location: bool) -> Vec<String> {
    if is_new_identifier_location {
        return Vec::new();
    }
    ALL_COMMIT_CHARACTERS
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl LanguageService {
    // Go: ls/completions.go:1819 completionInfoFromData
    // PORT: Go mutates `data.symbols` through the pointer, so `data` is
    // `&mut`.
    pub fn completion_info_from_data(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        file: Node,
        compiler_options: &CompilerOptions,
        data: &mut CompletionDataData,
        position: i32,
        optional_replacement_span: Option<lsproto::Range>,
        include_symbols: bool,
    ) -> Result<Option<CompletionList>, GoError> {
        let keyword_filters = data.keyword_filters;
        let is_new_identifier_location = data.is_new_identifier_location;
        let context_token = data.context_token;
        let mut literals = data.literals.clone();
        let preferences = self.user_preferences();

        // Verify if the file is JSX language variant
        if source_file_info(file).language_variant == LanguageVariant::JSX {
            let list = self.get_jsx_closing_tag_completion(ctx, data.location, file, position);
            if list.is_some() {
                return Ok(list);
            }
        }

        // When the completion is for the expression of a case clause (e.g. `case |`),
        // filter literals & enum symbols whose values are already present in existing case clauses.
        let case_clause = find_ancestor(context_token, is_case_clause);
        if case_clause.is_some()
            && (context_token.kind() == SyntaxKind::CaseKeyword
                || is_node_descendant_of(context_token, case_clause.expression()))
        {
            let clauses = case_clause.parent().clauses().nodes().to_vec();
            let tracker = new_case_clause_tracker(type_checker, &clauses);
            literals = literals
                .into_iter()
                .filter(|literal| !tracker.has_value(literal))
                .collect();
            data.symbols = data
                .symbols
                .iter()
                .copied()
                .filter(|&symbol| {
                    let value_declaration = type_checker.sym(symbol).value_declaration;
                    if value_declaration.is_some() && is_enum_member(value_declaration) {
                        let value = type_checker.get_constant_value(value_declaration);
                        if let Some(value) = value {
                            if tracker.has_value(&value) {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();
        }

        let is_checked = is_checked_file(file, compiler_options);
        if is_checked
            && !is_new_identifier_location
            && data.symbols.is_empty()
            && keyword_filters == KeywordCompletionFilters::NONE
        {
            return Ok(None);
        }

        let (mut unique_names, mut sorted_entries) = self.get_completion_entries_from_symbols(
            ctx,
            type_checker,
            data,
            Node::NIL, /*replacementToken*/
            position,
            file,
            compiler_options,
            include_symbols,
        )?;

        if data.keyword_filters != KeywordCompletionFilters::NONE {
            let keyword_completions = get_keyword_completions(
                data.keyword_filters,
                !data.inside_js_doc_tag_type_expression && is_source_file_js(file),
            );
            for keyword_entry in keyword_completions {
                let label = keyword_entry.completion_item.label.clone();
                if data.is_type_only_location && is_type_keyword(string_to_token(&label))
                    || !data.is_type_only_location
                        && is_contextual_keyword_in_auto_importable_expression_space(&label)
                    || !unique_names.contains(&label)
                {
                    unique_names.insert(label);
                    sorted_entries.push(keyword_entry);
                }
            }
        }

        for keyword_entry in get_contextual_keywords(file, context_token, position) {
            if !unique_names.contains(&keyword_entry.label) {
                unique_names.insert(keyword_entry.label.clone());
                sorted_entries.push(CompletionItem {
                    completion_item: keyword_entry,
                    symbol: SymbolId::NIL,
                });
            }
        }

        for literal in &literals {
            let literal_entry = create_completion_item_for_literal(file, &preferences, literal);
            unique_names.insert(literal_entry.label.clone());
            sorted_entries.push(CompletionItem {
                completion_item: literal_entry,
                symbol: SymbolId::NIL,
            });
        }

        if !is_checked {
            sorted_entries = self.get_js_completion_entries(
                ctx,
                file,
                position,
                &mut unique_names,
                sorted_entries,
            );
        }

        if context_token.is_some()
            && !data.is_right_of_open_tag
            && !data.is_right_of_dot_or_question_dot
        {
            let case_block = find_ancestor_kind(context_token, SyntaxKind::CaseBlock);
            if case_block.is_some() {
                let cases_item = self.get_exhaustive_case_snippets(
                    ctx,
                    case_block,
                    file,
                    position,
                    compiler_options,
                    &self.program,
                    type_checker,
                )?;
                if let Some(cases_item) = cases_item {
                    sorted_entries.push(CompletionItem {
                        completion_item: cases_item,
                        symbol: SymbolId::NIL,
                    });
                }
            }
        }

        // PORT: Go passes `&data.defaultCommitCharacters`, a non-nil pointer
        // to the field. The field is set at completions.go:1678 and is never
        // nil here; a nil slice would marshal as `[]` (json v2), the same as
        // the empty Vec.
        let default_commit_characters = data.default_commit_characters.clone().unwrap_or_default();
        let item_defaults = self.set_item_defaults(
            ctx,
            position,
            file,
            &mut sorted_entries,
            Some(&default_commit_characters),
            optional_replacement_span,
        );

        Ok(Some(CompletionList {
            is_incomplete: data.has_unresolved_auto_imports,
            item_defaults,
            apply_kind: None,
            items: sorted_entries,
        }))
    }

    // Go: ls/completions.go:1954 getCompletionEntriesFromSymbols
    pub fn get_completion_entries_from_symbols(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        data: &CompletionDataData,
        replacement_token: Node,
        position: i32,
        file: Node,
        compiler_options: &CompilerOptions,
        include_symbols: bool,
    ) -> Result<(FxHashSet<String>, Vec<CompletionItem>), GoError> {
        let closest_symbol_declaration =
            get_closest_symbol_declaration(data.context_token, data.location);
        let use_semicolons = lsutil::probably_uses_semicolons(file);
        let preferences = self.user_preferences();
        let is_member_completion = is_member_completion_kind(data.completion_kind);
        let mut sorted_entries: Vec<CompletionItem> =
            Vec::with_capacity(data.symbols.len() + data.auto_imports.len());
        // Tracks unique names.
        // Value is set to false for global variables or completions from external module exports, because we can have multiple of those;
        // true otherwise. Based on the order we add things we will always see locals first, then globals, then module exports.
        // So adding a completion for a local will prevent us from adding completions for external module exports sharing the same name.
        // PORT: Go map; only membership and the final key set are read.
        let mut uniques: UniqueNamesMap = FxHashMap::default();
        for (index, &symbol) in data.symbols.iter().enumerate() {
            let origin = data.symbol_to_origin_info_map.get(&(index as i32));
            let (name, needs_convert_property_access) =
                get_completion_entry_display_name_for_symbol(
                    file,
                    &preferences,
                    &type_checker.symbols,
                    symbol,
                    origin,
                    data.completion_kind,
                    data.is_jsx_identifier_expected,
                );
            if name.is_empty()
                || uniques.get(&name).copied().unwrap_or(false)
                    && (origin.is_none() || !origin_is_object_literal_method(origin))
                || data.completion_kind == CompletionKind::GLOBAL
                    && !should_include_symbol(
                        symbol,
                        data,
                        closest_symbol_declaration,
                        file,
                        type_checker,
                        compiler_options,
                    )
            {
                continue;
            }

            // When in a value location in a JS file, ignore symbols that definitely seem to be type-only.
            if !data.is_type_only_location
                && is_source_file_js(file)
                && symbol_appears_to_be_type_only(symbol, type_checker)
            {
                continue;
            }

            let mut original_sort_text: SortText = data
                .symbol_to_sort_text_map
                .get(&get_symbol_id(&type_checker.symbols, symbol))
                .cloned()
                .unwrap_or_default();
            if original_sort_text.is_empty() {
                original_sort_text = SORT_TEXT_LOCATION_PRIORITY.to_string();
            }

            let sort_text: SortText = if is_deprecated(symbol, type_checker) {
                deprecate_sort_text(&original_sort_text)
            } else {
                original_sort_text
            };
            let entry = self.create_completion_item(
                ctx,
                type_checker,
                symbol,
                &sort_text,
                replacement_token,
                data,
                position,
                file,
                &name,
                needs_convert_property_access,
                origin,
                use_semicolons,
                compiler_options,
                is_member_completion,
            )?;
            let Some(entry) = entry else {
                continue;
            };

            // True for locals; false for globals, module exports from other files, `this.` completions.
            let should_shadow_later_symbols = (origin.is_none()
                || origin_is_type_only_alias(origin))
                && !(type_checker.sym(symbol).parent.is_nil()
                    && !type_checker
                        .sym(symbol)
                        .declarations
                        .iter()
                        .any(|&d| get_source_file_of_node(d) == file));
            uniques.insert(name, should_shadow_later_symbols);
            let sym = if include_symbols {
                symbol
            } else {
                SymbolId::NIL
            };
            sorted_entries.push(CompletionItem {
                completion_item: entry,
                symbol: sym,
            });
        }

        for auto_import in &data.auto_imports {
            // !!! check for type-only in JS
            // !!! deprecation

            let mut replacement_span: Option<lsproto::Range> = None;
            let mut insert_text = String::new();
            let mut filter_text = String::new();
            let mut is_snippet = false;
            let mut sort_text: SortText = SORT_TEXT_AUTO_IMPORT_SUGGESTIONS.to_string();

            if let Some(import_statement_completion) = &data.import_statement_completion {
                is_snippet = client_supports_item_snippet(ctx);
                (insert_text, replacement_span) =
                    get_insert_text_and_replacement_span_for_import_completion(
                        &auto_import.fix,
                        autoimport::get_import_kind_for_import_statement(
                            file,
                            &auto_import.export,
                            self.get_program(),
                        ),
                        import_statement_completion,
                        use_semicolons,
                        file,
                        &preferences,
                        is_snippet,
                    );
                // The edit range covers the whole import statement typed so far, and clients match that text against the
                // filter text, so it has to be the statement being inserted (as in Strada), not just the bare name.
                filter_text = insert_text.clone();
                sort_text = SORT_TEXT_LOCATION_PRIORITY.to_string();
            }

            // Non-contextual keywords (e.g., `function`, `class`, `const`) cannot be used as identifiers,
            // so auto-imports with these names should not shadow keyword completions.
            let token = string_to_token(&auto_import.fix.name);
            if token != SyntaxKind::Unknown && crate::ast::is_non_contextual_keyword(token) {
                continue;
            }

            if !auto_import.export.is_unresolved_alias() {
                if data.is_type_only_location {
                    if !auto_import.export.flags.intersects(SymbolFlags::TYPE)
                        && !auto_import.export.flags.intersects(SymbolFlags::MODULE)
                    {
                        continue;
                    }
                } else if data.import_statement_completion.is_none()
                    && !auto_import.export.flags.intersects(SymbolFlags::VALUE)
                {
                    continue;
                }
            }

            let mut entry = self.create_lsp_completion_item(
                ctx,
                &auto_import.fix.name,
                &insert_text,
                &filter_text,
                &sort_text,
                auto_import.export.script_element_kind,
                auto_import.export.script_element_kind_modifiers,
                replacement_span,
                None,
                Some(lsproto::CompletionItemLabelDetails {
                    description: Some(auto_import.fix.module_specifier.clone()),
                    ..Default::default()
                }),
                file,
                position,
                false, /*isMemberCompletion*/
                is_snippet,
                data.import_statement_completion.is_none(), /*hasAction*/
                false,                                      /*preselect*/
                &auto_import.fix.module_specifier,
                Some(auto_import.fix.auto_import_fix.clone()),
                None, /*additionalTextEdits*/
                None, /*detail*/
            );

            entry
                .data
                .as_mut()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .is_import_statement_completion = data.import_statement_completion.is_some();

            let is_shadowed = uniques.get(&auto_import.fix.name).copied().unwrap_or(false);
            if !is_shadowed {
                uniques.insert(auto_import.fix.name.clone(), false);
                sorted_entries.push(CompletionItem {
                    completion_item: entry,
                    symbol: SymbolId::NIL,
                });
            }
        }

        let mut unique_set: FxHashSet<String> =
            FxHashSet::with_capacity_and_hasher(uniques.len(), Default::default());
        for name in uniques.keys() {
            unique_set.insert(name.clone());
        }
        Ok((unique_set, sorted_entries))
    }
}

// Go: ls/completions.go:2125 completionNameForLiteral
pub fn completion_name_for_literal(
    file: Node,
    preferences: &lsutil::UserPreferences,
    literal: &LiteralValue,
) -> String {
    match literal {
        LiteralValue::String(literal) => quote(file, preferences, literal),
        LiteralValue::Number(literal) => {
            // Go: `core.StringifyJson(literal, "", "")`; the error is ignored
            // (an error gives "").
            json_ext::marshal_indent(&literal.0, "" /*prefix*/, "" /*suffix*/).unwrap_or_default()
        }
        LiteralValue::PseudoBigInt(literal) => format!("{literal}n"),
        LiteralValue::Bool(literal) => {
            crate::core::go_panic(format!("Unhandled literal value: {literal}"))
        }
    }
}

// Go: ls/completions.go:2142 getInsertTextAndReplacementSpanForImportCompletion
pub fn get_insert_text_and_replacement_span_for_import_completion(
    fix: &autoimport::Fix,
    import_kind: lsproto::ImportKind,
    import_statement_completion: &ImportStatementCompletionInfo,
    use_semicolons: bool,
    file: Node,
    preferences: &lsutil::UserPreferences,
    is_snippet: bool,
) -> (String, Option<lsproto::Range>) {
    let quoted_module_specifier =
        escape_snippet_text(&quote(file, preferences, &fix.module_specifier));
    let tab_stop = if is_snippet { "$1" } else { "" };
    let suffix = if use_semicolons { ";" } else { "" };
    let top_level_type_only_text = if import_statement_completion.is_top_level_type_only {
        format!(" {} ", token_to_string(SyntaxKind::TypeKeyword))
    } else {
        " ".to_string()
    };
    let name = escape_snippet_text(&fix.name);
    let replacement_span = import_statement_completion.replacement_span;

    match import_kind {
        lsproto::ImportKind::COMMON_JS => (
            format!(
                "import{top_level_type_only_text}{name}{tab_stop} = require({quoted_module_specifier}){suffix}"
            ),
            replacement_span,
        ),
        lsproto::ImportKind::DEFAULT => (
            format!(
                "import{top_level_type_only_text}{name}{tab_stop} from {quoted_module_specifier}{suffix}"
            ),
            replacement_span,
        ),
        lsproto::ImportKind::NAMESPACE => (
            format!(
                "import{top_level_type_only_text}* as {name} from {quoted_module_specifier}{suffix}"
            ),
            replacement_span,
        ),
        lsproto::ImportKind::NAMED => {
            let type_only = if import_statement_completion.could_be_type_only_import_specifier {
                format!("{} ", token_to_string(SyntaxKind::TypeKeyword))
            } else {
                String::new()
            };
            (
                format!(
                    "import{top_level_type_only_text}{{ {type_only}{name}{tab_stop} }} from {quoted_module_specifier}{suffix}"
                ),
                replacement_span,
            )
        }
        _ => crate::core::go_panic(format!("unhandled import kind: {}", import_kind.string())),
    }
}

// Go: ls/completions.go:2164 createCompletionItemForLiteral
pub fn create_completion_item_for_literal(
    file: Node,
    preferences: &lsutil::UserPreferences,
    literal: &LiteralValue,
) -> lsproto::CompletionItem {
    lsproto::CompletionItem {
        label: completion_name_for_literal(file, preferences, literal),
        kind: Some(lsproto::CompletionItemKind::CONSTANT),
        sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
        commit_characters: Some(Vec::new()),
        ..Default::default()
    }
}

impl LanguageService {
    // Go: ls/completions.go:2177 createCompletionItem
    // PORT: Go returns a nil `*lsproto.CompletionItem` when there is no dot
    // to convert; that is `None`. Go reassigns the `sortText` and `name`
    // parameters, so they are copied into locals.
    pub fn create_completion_item(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        symbol: SymbolId,
        sort_text: &str,
        replacement_token: Node,
        data: &CompletionDataData,
        position: i32,
        file: Node,
        name: &str,
        needs_convert_property_access: bool,
        origin: Option<&SymbolOriginInfo>,
        use_semicolons: bool,
        compiler_options: &CompilerOptions,
        is_member_completion: bool,
    ) -> Result<Option<lsproto::CompletionItem>, GoError> {
        let mut sort_text: SortText = sort_text.to_string();
        let mut name: String = name.to_string();
        let context_token = data.context_token;
        let mut insert_text = String::new();
        let mut filter_text = String::new();
        let mut replacement_span =
            self.get_replacement_range_for_context_token(file, replacement_token, position);
        let mut is_snippet = false;
        let mut has_action = false;
        let mut source = get_source_from_origin(origin);
        let mut label_details: Option<lsproto::CompletionItemLabelDetails> = None;
        let preferences = self.user_preferences();
        let insert_question_dot = origin_is_nullable_member(origin);
        let use_braces = origin_is_symbol_member(origin) || needs_convert_property_access;
        if origin_is_this_type_node(origin) {
            if needs_convert_property_access {
                insert_text = format!(
                    "this{}[{}]",
                    if insert_question_dot { "?." } else { "" },
                    quote_property_name(file, &preferences, &name),
                );
            } else {
                insert_text = format!(
                    "this{}{}",
                    if insert_question_dot { "?." } else { "." },
                    name,
                );
            }
        } else if data.property_access_to_convert.is_some() && (use_braces || insert_question_dot) {
            // We should only have needsConvertPropertyAccess if there's a property access to convert. But see microsoft/TypeScript#21790.
            // Somehow there was a global with a non-identifier name. Hopefully someone will complain about getting a "foo bar" global completion and provide a repro.
            if use_braces {
                if needs_convert_property_access {
                    insert_text = format!("[{}]", quote_property_name(file, &preferences, &name));
                } else {
                    insert_text = format!("[{name}]");
                }
            } else {
                insert_text = name.clone();
            }

            if insert_question_dot
                || data
                    .property_access_to_convert
                    .question_dot_token()
                    .is_some()
            {
                insert_text = format!("?.{insert_text}");
            }

            let mut dot = astnav::find_child_of_kind(
                data.property_access_to_convert,
                SyntaxKind::DotToken,
                file,
            );
            if dot.is_nil() {
                dot = astnav::find_child_of_kind(
                    data.property_access_to_convert,
                    SyntaxKind::QuestionDotToken,
                    file,
                );
            }

            if dot.is_nil() {
                return Ok(None);
            }

            // If the text after the '.' starts with this name, write over it. Else, add new text.
            let end = if name.starts_with(data.property_access_to_convert.name().text()) {
                data.property_access_to_convert.end()
            } else {
                dot.end()
            };
            let (lsp_range, fidelity) = self.create_lsp_range_from_bounds(
                astnav::get_start_of_node(dot, file, false /*includeJSDoc*/),
                end,
                file,
            );
            if !fidelity.is_exact() {
                return Ok(None);
            }
            replacement_span = Some(lsp_range);
        }

        if data.jsx_initializer.is_initializer {
            if insert_text.is_empty() {
                insert_text = name.clone();
            }
            insert_text = format!("{{{insert_text}}}");
            if data.jsx_initializer.initializer.is_some() {
                let (lsp_range, fidelity) =
                    self.create_lsp_range_from_node(data.jsx_initializer.initializer, file);
                if !fidelity.is_exact() {
                    return Ok(None);
                }
                replacement_span = Some(lsp_range);
            }
        }

        if origin_is_promise(origin) && data.property_access_to_convert.is_some() {
            if insert_text.is_empty() {
                insert_text = name.clone();
            }
            let preceding_token =
                astnav::find_preceding_token(file, data.property_access_to_convert.pos());
            let mut await_text = String::new();
            if preceding_token.is_some()
                && lsutil::position_is_asi_candidate(
                    preceding_token.end(),
                    preceding_token.parent(),
                    file,
                )
            {
                await_text = ";".to_string();
            }

            await_text += &format!(
                "(await {})",
                get_text_of_node(data.property_access_to_convert.expression())
            );
            if needs_convert_property_access {
                insert_text = await_text + &insert_text;
            } else {
                let dot_str = if insert_question_dot { "?." } else { "." };
                insert_text = await_text + dot_str + &insert_text;
            }
            let is_in_await_expression =
                is_await_expression(data.property_access_to_convert.parent());
            let wrap_node = if is_in_await_expression {
                data.property_access_to_convert.parent()
            } else {
                data.property_access_to_convert.expression()
            };
            let (lsp_range, fidelity) = self.create_lsp_range_from_bounds(
                astnav::get_start_of_node(wrap_node, file, false /*includeJSDoc*/),
                data.property_access_to_convert.end(),
                file,
            );
            if !fidelity.is_exact() {
                return Ok(None);
            }
            replacement_span = Some(lsp_range);
        }

        if origin_is_type_only_alias(origin) {
            has_action = true;
        }

        // Provide object member completions when missing commas, and insert missing commas.
        // For example:
        //
        //    interface I {
        //        a: string;
        //        b: number
        //     }
        //
        //     const cc: I = { a: "red" | }
        //
        // Completion should add a comma after "red" and provide completions for b
        if data.completion_kind == CompletionKind::OBJECT_PROPERTY_DECLARATION
            && context_token.is_some()
            && !node_has_kind(
                astnav::find_preceding_token_ex(
                    file,
                    context_token.pos(),
                    context_token,
                    false, /*excludeJSDoc*/
                ),
                SyntaxKind::CommaToken,
            )
        {
            if is_method_declaration(context_token.parent().parent())
                || is_get_accessor_declaration(context_token.parent().parent())
                || is_set_accessor_declaration(context_token.parent().parent())
                || is_spread_assignment(context_token.parent())
                || lsutil::get_last_token(
                    find_ancestor(context_token.parent(), is_property_assignment),
                    file,
                ) == context_token
                || is_shorthand_property_assignment(context_token.parent())
                    && get_line_of_position(file, context_token.end())
                        != get_line_of_position(file, position)
            {
                source = COMPLETION_SOURCE_OBJECT_LITERAL_MEMBER_WITH_COMMA.to_string();
                has_action = true;
            }
        }

        let mut additional_text_edits: Option<Vec<lsproto::TextEdit>> = None;
        if preferences
            .include_completions_with_class_member_snippets
            .is_true()
            && data.completion_kind == CompletionKind::MEMBER_LIKE
            && is_class_like_member_completion(&type_checker.symbols, symbol, data.location, file)
        {
            let member_completion_entry = self.get_entry_for_member_completion(
                ctx,
                type_checker,
                symbol,
                &name,
                data.location,
                position,
                context_token,
                file,
            )?;
            let Some(member_completion_entry) = member_completion_entry else {
                return Ok(None);
            };
            insert_text = member_completion_entry.insert_text;
            filter_text = member_completion_entry.filter_text;
            is_snippet = member_completion_entry.is_snippet;
            if !member_completion_entry.additional_text_edits.is_empty() {
                additional_text_edits = Some(member_completion_entry.additional_text_edits);
                has_action = true;
                source = COMPLETION_SOURCE_CLASS_MEMBER_SNIPPET.to_string();
            }
        }

        if origin_is_object_literal_method(origin) {
            let origin = origin.unwrap();
            insert_text = origin.as_object_literal_method().insert_text.clone();
            is_snippet = origin.as_object_literal_method().is_snippet;
            label_details = origin.as_object_literal_method().label_details.clone();
            if !client_supports_item_label_details(ctx) {
                // PORT: Go dereferences `labelDetails.Detail`; a nil pointer
                // panics there.
                name = name
                    + origin
                        .as_object_literal_method()
                        .label_details
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .detail
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                label_details = None;
            }
            source = COMPLETION_SOURCE_OBJECT_LITERAL_METHOD_SNIPPET.to_string();
            sort_text = sort_below(&sort_text);
        }

        if data.is_jsx_identifier_expected
            && !data.is_right_of_open_tag
            && client_supports_item_snippet(ctx)
            && preferences.jsx_attribute_completion_style
                != lsutil::JsxAttributeCompletionStyle::NONE
            && !(data.location.parent().is_some()
                && is_jsx_attribute(data.location.parent())
                && data.location.parent().initializer().is_some())
        {
            let mut use_braces = preferences.jsx_attribute_completion_style
                == lsutil::JsxAttributeCompletionStyle::BRACES;
            let t = type_checker.get_type_of_symbol_at_location(symbol, data.location);

            // If is boolean like or undefined, don't return a snippet, we want to return just the completion.
            if preferences.jsx_attribute_completion_style
                == lsutil::JsxAttributeCompletionStyle::AUTO
                && !type_checker.ty(t).is_boolean_like()
                && !(type_checker.ty(t).is_union()
                    && type_checker
                        .ty(t)
                        .types()
                        .iter()
                        .any(|&t| type_checker.ty(t).is_boolean_like()))
            {
                if type_checker.ty(t).is_string_like()
                    || type_checker.ty(t).is_union() && {
                        let types = type_checker.ty(t).types().to_vec();
                        types.iter().all(|&t| {
                            type_checker
                                .ty(t)
                                .flags
                                .intersects(TypeFlags::STRING_LIKE | TypeFlags::UNDEFINED)
                                || is_string_and_empty_anonymous_object_intersection(
                                    type_checker,
                                    t,
                                )
                        })
                    }
                {
                    // If type is string-like or undefined, use quotes.
                    insert_text = format!(
                        "{}={}",
                        escape_snippet_text(&name),
                        quote(file, &preferences, "$1")
                    );
                    is_snippet = true;
                } else {
                    // Use braces for everything else.
                    use_braces = true;
                }
            }

            if use_braces {
                insert_text = escape_snippet_text(&name) + "={$1}";
                is_snippet = true;
            }
        }

        let parent_named_import_or_export =
            find_ancestor(data.location, is_named_imports_or_exports);
        if parent_named_import_or_export.is_some() {
            if !is_identifier_text(&name, LanguageVariant::STANDARD) {
                insert_text = quote_property_name(file, &preferences, &name);

                if parent_named_import_or_export.kind() == SyntaxKind::NamedImports {
                    // Check if it is `import { ^here as name } from '...'``.
                    // We have to access the scanner here to check if it is `{ ^here as name }`` or `{ ^here, as, name }`.
                    let text = source_file_text(file);
                    let mut scanner = crate::frontend::scanner::new_scanner();
                    scanner.set_text(&text);
                    scanner.reset_pos(position);
                    if !(scanner.scan() == SyntaxKind::AsKeyword
                        && scanner.scan() == SyntaxKind::Identifier)
                    {
                        insert_text +=
                            &format!(" as {}", generate_identifier_for_arbitrary_string(&name));
                    }
                }
            } else if parent_named_import_or_export.kind() == SyntaxKind::NamedImports {
                let possible_token = string_to_token(&name);
                if possible_token != SyntaxKind::Unknown
                    && (possible_token == SyntaxKind::AwaitKeyword
                        || lsutil::is_non_contextual_keyword(possible_token))
                {
                    insert_text = format!("{name} as {name}_");
                }
            }
        }

        // Commit characters

        let element_kind = lsutil::get_symbol_kind(Some(&mut *type_checker), symbol, data.location);
        let mut commit_characters: Option<Vec<String>> = None;
        if client_supports_item_commit_characters(ctx) {
            if element_kind == lsutil::ScriptElementKind::WARNING
                || element_kind == lsutil::ScriptElementKind::STRING
            {
                commit_characters = Some(Vec::new());
            } else if !client_supports_default_commit_characters(ctx) {
                // PORT: Go `new(data.defaultCommitCharacters)` is a non-nil
                // pointer; a nil slice marshals as `[]` (json v2).
                commit_characters =
                    Some(data.default_commit_characters.clone().unwrap_or_default());
            }
            // Otherwise use the completion list default.
        }

        let preselect =
            is_recommended_completion_match(symbol, data.recommended_completion, type_checker);
        let kind_modifiers = lsutil::get_symbol_modifiers(Some(&mut *type_checker), symbol);

        Ok(Some(self.create_lsp_completion_item(
            ctx,
            &name,
            &insert_text,
            &filter_text,
            &sort_text,
            element_kind,
            kind_modifiers,
            replacement_span,
            commit_characters,
            label_details,
            file,
            position,
            is_member_completion,
            is_snippet,
            has_action,
            preselect,
            &source,
            None, /*autoImportFix*/
            additional_text_edits,
            None, /*detail*/
        )))
    }
}

// Go: ls/completions.go:2466 memberCompletionEntry
#[derive(Clone, Debug, Default)]
pub struct MemberCompletionEntry {
    pub insert_text: String,
    pub filter_text: String,
    pub is_snippet: bool,
    pub additional_text_edits: Vec<lsproto::TextEdit>,
}

impl LanguageService {
    // Go: ls/completions.go:2473 getEntryForObjectLiteralMethodCompletion
    // PORT: Go returns `*symbolOriginInfoObjectLiteralMethod`; nil is `None`.
    pub fn get_entry_for_object_literal_method_completion(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        symbol: SymbolId,
        enclosing_declaration: Node,
        file: Node,
    ) -> Option<SymbolOriginInfoObjectLiteralMethod> {
        let mut snippet_printer = create_snippet_printer(
            PrinterOptions {
                remove_comments: true,
                new_line: get_new_line_kind(
                    &self.format_options().editor_settings.new_line_character,
                ),
                target: self.get_program().options().get_emit_script_target(),
                ..Default::default()
            },
            None, /*emitContext*/
        );

        let is_snippet = client_supports_item_snippet(ctx);
        let method = self.create_object_literal_method(
            &snippet_printer,
            type_checker,
            symbol,
            enclosing_declaration,
            file,
            is_snippet,
        );
        if method.is_nil() {
            return None;
        }

        let mut insert_text = snippet_printer.print_and_format_node_with_settings(
            ctx,
            method,
            file,
            &change::get_format_code_settings_for_writing(self.format_options(), file),
        );
        insert_text += ",";

        Some(SymbolOriginInfoObjectLiteralMethod {
            insert_text,
            label_details: Some(lsproto::CompletionItemLabelDetails {
                detail: Some(self.print_object_literal_method_label_detail(
                    method,
                    file,
                    snippet_printer.factory(),
                )),
                ..Default::default()
            }),
            is_snippet,
        })
    }

    // Go: ls/completions.go:2498 createObjectLiteralMethod
    pub fn create_object_literal_method(
        &self,
        snippet_printer: &SnippetPrinter,
        type_checker: &mut Checker,
        symbol: SymbolId,
        enclosing_declaration: Node,
        file: Node,
        is_snippet: bool,
    ) -> Node {
        let factory = snippet_printer.factory();
        let emit_context = &snippet_printer.emit_context;

        let declaration = type_checker
            .sym(symbol)
            .declarations
            .first()
            .copied()
            .unwrap_or(Node::NIL);
        if !is_object_literal_method_completion_candidate_declaration(declaration) {
            return Node::NIL;
        }

        let type_of_symbol =
            type_checker.get_type_of_symbol_at_location(symbol, enclosing_declaration);
        let mut effective_type = type_checker.get_widened_type_exported(type_of_symbol);
        if type_checker
            .ty(effective_type)
            .flags
            .intersects(TypeFlags::UNION)
            && type_checker.ty(effective_type).types().len() < 10
        {
            let types = type_checker.ty(effective_type).types().to_vec();
            effective_type =
                type_checker.get_union_type_ex_exported(&types, UnionReduction::SUBTYPE);
        }
        if type_checker
            .ty(effective_type)
            .flags
            .intersects(TypeFlags::UNION)
        {
            let mut function_type = TypeId::NIL;
            let types = type_checker.ty(effective_type).types().to_vec();
            for union_type in types {
                if type_checker
                    .get_signatures_of_type_exported(union_type, SignatureKind::CALL)
                    .is_empty()
                {
                    continue;
                }
                if function_type.is_some() {
                    return Node::NIL;
                }
                function_type = union_type;
            }
            if function_type.is_nil() {
                return Node::NIL;
            }
            effective_type = function_type;
        }

        let signatures =
            type_checker.get_signatures_of_type_exported(effective_type, SignatureKind::CALL);
        if signatures.len() != 1 {
            return Node::NIL;
        }

        let mut flags = NodeBuilderFlags::OMIT_THIS_PARAMETER;
        if lsutil::get_quote_preference(file, &self.user_preferences())
            == lsutil::QuotePreference::SINGLE
        {
            flags |= NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE;
        }
        let type_node = type_checker.type_to_type_node_exported(
            effective_type,
            enclosing_declaration,
            flags,
            None, /*idToSymbol*/
        );
        if type_node.is_nil() || type_node.kind() != SyntaxKind::FunctionType {
            return Node::NIL;
        }

        let type_node_parameters = type_node.parameters();
        let mut parameters: Vec<Node> = Vec::with_capacity(type_node_parameters.len());
        for parameter in type_node_parameters.iter() {
            parameters.push(factory.new_parameter_declaration(
                ModifierList::NIL, /*modifiers*/
                parameter.dot_dot_dot_token(),
                factory.clone_node(parameter.name()),
                Node::NIL, /*questionToken*/
                Node::NIL, /*typeNode*/
                parameter.initializer(),
            ));
        }

        let mut body = factory.new_block(
            factory.new_node_list(&[] /*nodes*/),
            true, /*multiLine*/
        );
        if is_snippet {
            body = create_snippet_tab_stop_body(factory, emit_context);
        }

        factory.new_method_declaration(
            ModifierList::NIL, /*modifiers*/
            Node::NIL,         /*asteriskToken*/
            factory.clone_node(declaration.name()),
            Node::NIL,     /*postfixToken*/
            NodeList::NIL, /*typeParameters*/
            factory.new_node_list(&parameters),
            Node::NIL, /*typeNode*/
            Node::NIL, /*fullSignature*/
            body,
        )
    }
}

// Go: ls/completions.go:2572 isObjectLiteralMethodCompletionCandidateDeclaration
pub fn is_object_literal_method_completion_candidate_declaration(declaration: Node) -> bool {
    if declaration.is_nil() {
        return false;
    }
    matches!(
        declaration.kind(),
        SyntaxKind::PropertySignature
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
    )
}

// Go: ls/completions.go:2584 objectLiteralMethodSymbol
// PORT: Go `origin *symbolOriginInfo` is never nil here; it is held by value.
#[derive(Clone, Debug, Default)]
pub struct ObjectLiteralMethodSymbol {
    pub symbol: SymbolId,
    pub origin: SymbolOriginInfo,
}

impl LanguageService {
    // Go: ls/completions.go:2589 collectObjectLiteralMethodSymbols
    pub fn collect_object_literal_method_symbols(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        members: &[SymbolId],
        enclosing_declaration: Node,
        file: Node,
    ) -> Vec<ObjectLiteralMethodSymbol> {
        if is_source_file_js(file) {
            return Vec::new();
        }

        let preferences = self.user_preferences();
        let mut methods: Vec<ObjectLiteralMethodSymbol> = Vec::new();
        for &member in members {
            if !is_object_literal_method_symbol(&type_checker.symbols, member) {
                continue;
            }
            let (display_name, _) = get_completion_entry_display_name_for_symbol(
                file,
                &preferences,
                &type_checker.symbols,
                member,
                None, /*origin*/
                CompletionKind::OBJECT_PROPERTY_DECLARATION,
                false, /*isJsxIdentifierExpected*/
            );
            if display_name.is_empty() {
                continue;
            }
            let Some(entry) = self.get_entry_for_object_literal_method_completion(
                ctx,
                type_checker,
                member,
                enclosing_declaration,
                file,
            ) else {
                continue;
            };
            methods.push(ObjectLiteralMethodSymbol {
                symbol: member,
                origin: SymbolOriginInfo {
                    kind: SymbolOriginInfoKind::OBJECT_LITERAL_METHOD,
                    data: SymbolOriginInfoData::ObjectLiteralMethod(entry),
                    ..Default::default()
                },
            });
        }
        methods
    }
}

// Go: ls/completions.go:2619 isObjectLiteralMethodSymbol
// PORT: Go reads `symbol.Flags` through the pointer; `symbols` is the arena
// that holds `symbol`.
pub fn is_object_literal_method_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    symbols
        .sym(symbol)
        .flags
        .intersects(SymbolFlags::PROPERTY | SymbolFlags::METHOD)
}

impl LanguageService {
    // Go: ls/completions.go:2623 printObjectLiteralMethodLabelDetail
    pub fn print_object_literal_method_label_detail(
        &self,
        method: Node,
        file: Node,
        factory: &NodeFactory,
    ) -> String {
        let method_signature = factory.new_method_signature_declaration(
            ModifierList::NIL, /*modifiers*/
            factory.new_identifier(""),
            method.postfix_token(),
            method.type_parameter_list(),
            method.parameter_list(),
            method.type_(),
        );
        let mut signature_printer = new_printer(
            PrinterOptions {
                remove_comments: true,
                omit_trailing_semicolon: true,
                new_line: get_new_line_kind(
                    &self.format_options().editor_settings.new_line_character,
                ),
                target: self.get_program().options().get_emit_script_target(),
                ..Default::default()
            },
            PrintHandlers::default(),
            None, /*emitContext*/
        );
        signature_printer.emit(method_signature, file)
    }

    // Go: ls/completions.go:2642 getEntryForMemberCompletion
    // PORT: Go returns `(*memberCompletionEntry, error)`; nil is `None`. The
    // missing member fixer borrows the change tracker, the checker and the
    // import adder; it is last used for `createMemberFromSymbol`, so the Go
    // uses after that call read them directly.
    pub fn get_entry_for_member_completion(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        symbol: SymbolId,
        name: &str,
        location: Node,
        position: i32,
        context_token: Node,
        file: Node,
    ) -> Result<Option<MemberCompletionEntry>, GoError> {
        let class_like_declaration = find_ancestor(location, is_class_like);
        if class_like_declaration.is_nil() {
            return Ok(None);
        }

        let mut import_adder = self.create_import_adder(ctx, type_checker, file)?;

        let mut change_tracker = change::new_tracker(
            ctx,
            self.get_program().options(),
            self.format_options(),
            Rc::clone(&self.converters),
        );
        let mut fixer = new_missing_member_fixer(
            &mut change_tracker,
            self.get_program(),
            &mut *type_checker,
            self.user_preferences(),
            import_adder.as_deref_mut(),
            locale::from_context(ctx),
        );

        let present_modifiers = self.get_present_member_modifiers(context_token, file, position);
        let abstract_ = present_modifiers
            .modifiers
            .intersects(ModifierFlags::ABSTRACT)
            && class_like_declaration
                .modifier_flags()
                .intersects(ModifierFlags::ABSTRACT);
        let is_snippet = client_supports_item_snippet(ctx);
        let factory = fixer.change_tracker.node_factory();
        let mut body = factory.new_block(factory.new_node_list(&[]), true /*multiLine*/);
        if is_snippet {
            body = create_snippet_tab_stop_body(
                fixer.change_tracker.node_factory(),
                &fixer.change_tracker.emit_context,
            );
        }

        let nodes = fixer.create_member_from_symbol(
            symbol,
            class_like_declaration,
            file,
            body,
            PreserveOptionalFlags::PROPERTY,
            abstract_,
        );
        let mut additional_text_edits: Vec<lsproto::TextEdit> = Vec::new();
        if let Some(import_adder) = import_adder.as_mut() {
            if import_adder.has_fixes() {
                additional_text_edits = import_adder.edits();
            }
        }
        if let Some(erase_range) = present_modifiers.erase_range {
            additional_text_edits.push(lsproto::TextEdit {
                range: erase_range,
                new_text: String::new(),
            });
        }

        let mut modifiers = ModifierFlags::NONE;
        let mut completion_nodes: Vec<Node> = Vec::with_capacity(nodes.len());
        for node in nodes {
            if node.is_nil() {
                continue;
            }
            if completion_nodes.is_empty() {
                modifiers = node.modifier_flags();
                if abstract_ {
                    modifiers |= ModifierFlags::ABSTRACT;
                }
                if is_class_element(node)
                    && type_checker.get_member_override_modifier_status_exported(
                        class_like_declaration,
                        node,
                        symbol,
                    ) == MemberOverrideStatus::NEEDS_OVERRIDE
                {
                    modifiers |= ModifierFlags::OVERRIDE;
                }
            }
            completion_nodes.push(node);
        }

        if completion_nodes.is_empty() {
            return Ok(Some(MemberCompletionEntry {
                insert_text: name.to_string(),
                filter_text: name.to_string(),
                is_snippet,
                additional_text_edits,
            }));
        }

        let mut allowed_modifiers = modifiers | ModifierFlags::OVERRIDE | ModifierFlags::PUBLIC;
        if type_checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::METHOD)
        {
            allowed_modifiers |= ModifierFlags::ASYNC;
        } else {
            allowed_modifiers |= ModifierFlags::AMBIENT | ModifierFlags::READONLY;
        }

        let allowed_and_present = present_modifiers.modifiers & allowed_modifiers;
        if present_modifiers.modifiers.without(allowed_modifiers) != ModifierFlags::NONE {
            return Ok(None);
        }

        if modifiers.intersects(ModifierFlags::PROTECTED)
            && allowed_and_present.intersects(ModifierFlags::PUBLIC)
        {
            modifiers = modifiers.without(ModifierFlags::PROTECTED);
        }

        if allowed_and_present != ModifierFlags::NONE
            && !allowed_and_present.intersects(ModifierFlags::PUBLIC)
        {
            modifiers = modifiers.without(ModifierFlags::PUBLIC);
        }

        modifiers |= allowed_and_present;
        let new_line = self.format_options().editor_settings.new_line_character;
        let mut snippet_printer = create_snippet_printer(
            PrinterOptions {
                remove_comments: true,
                new_line: get_new_line_kind(&new_line),
                target: self.get_program().options().get_emit_script_target(),
                ..Default::default()
            },
            Some(Rc::clone(&change_tracker.emit_context)),
        );

        let mut decorated_node = Node::NIL;
        if !present_modifiers.decorators.is_empty() {
            let last_node_index = completion_nodes.len() - 1;
            if can_have_decorators(completion_nodes[last_node_index]) {
                decorated_node = completion_nodes[last_node_index];
            }
        }

        let mut texts: Vec<String> = Vec::with_capacity(completion_nodes.len());
        for node in completion_nodes {
            let decorators: &[Node] = if node == decorated_node {
                &present_modifiers.decorators
            } else {
                &[]
            };
            let node = replace_modifiers(
                change_tracker.node_factory(),
                node,
                create_modifier_list(change_tracker.node_factory(), modifiers, decorators),
            );
            let text = snippet_printer.print_and_format_node_with_settings(
                ctx,
                node,
                file,
                &change::get_format_code_settings_for_writing(self.format_options(), file),
            );
            texts.push(text);
        }

        let insert_text = texts.join(new_line.as_str());
        if insert_text.is_empty() {
            return Ok(None);
        }

        Ok(Some(MemberCompletionEntry {
            insert_text,
            filter_text: name.to_string(),
            is_snippet,
            additional_text_edits,
        }))
    }
}

// Go: ls/completions.go:2759 presentMemberModifiers
#[derive(Clone, Debug, Default)]
pub struct PresentMemberModifiers {
    pub modifiers: ModifierFlags,
    pub decorators: Vec<Node>,
    pub erase_range: Option<lsproto::Range>,
}

impl LanguageService {
    // Go: ls/completions.go:2765 getPresentMemberModifiers
    pub fn get_present_member_modifiers(
        &self,
        context_token: Node,
        file: Node,
        position: i32,
    ) -> PresentMemberModifiers {
        if context_token.is_nil()
            || get_line_of_position(file, position)
                > get_line_of_position(file, context_token.end())
        {
            return PresentMemberModifiers::default();
        }

        let mut modifiers = ModifierFlags::NONE;
        let mut decorators: Vec<Node> = Vec::new();
        let mut range_pos = position;
        let mut range_end = position;

        if is_property_declaration(context_token.parent()) {
            let context_modifier_kind = modifier_like_kind(context_token);
            if context_modifier_kind == SyntaxKind::Unknown {
                return PresentMemberModifiers::default();
            }

            let modifier_nodes = context_token.parent().modifier_nodes().to_vec();
            if !modifier_nodes.is_empty() {
                modifiers |= modifiers_to_flags(&modifier_nodes) & ModifierFlags::MODIFIER;
                for &modifier in &modifier_nodes {
                    if is_decorator(modifier) {
                        decorators.push(modifier);
                    }
                    range_pos = range_pos.min(get_token_pos_of_node(
                        modifier, file, false, /*includeJSDoc*/
                    ));
                }
            }

            let context_modifier_flag = modifier_to_flag(context_modifier_kind);
            if !modifiers.intersects(context_modifier_flag) {
                modifiers |= context_modifier_flag;
                range_pos = range_pos.min(astnav::get_start_of_node(
                    context_token,
                    file,
                    false, /*includeJSDoc*/
                ));
            }

            if context_token.parent().name() != context_token {
                range_end = astnav::get_start_of_node(
                    context_token.parent().name(),
                    file,
                    false, /*includeJSDoc*/
                );
            }
        }

        let mut erase_range: Option<lsproto::Range> = None;
        if range_pos < range_end {
            let (lsp_range, fidelity) =
                self.create_lsp_range_from_bounds(range_pos, range_end, file);
            if fidelity.is_exact() {
                erase_range = Some(lsp_range);
            }
        }

        PresentMemberModifiers {
            modifiers,
            decorators,
            erase_range,
        }
    }
}

// Go: ls/completions.go:2818 modifierLikeKind
pub fn modifier_like_kind(node: Node) -> SyntaxKind {
    if node.is_nil() {
        return SyntaxKind::Unknown;
    }
    if is_modifier(node) {
        return node.kind();
    }
    if is_identifier(node) {
        let keyword_kind = identifier_to_keyword_kind(node);
        if keyword_kind != SyntaxKind::Unknown && is_modifier_kind(keyword_kind) {
            return keyword_kind;
        }
    }
    SyntaxKind::Unknown
}

// Go: ls/completions.go:2834 createModifierList
// PORT: Go returns a nil `*ast.ModifierList` when there are no nodes; that is
// `ModifierList::NIL`.
pub fn create_modifier_list(
    factory: &NodeFactory,
    flags: ModifierFlags,
    decorators: &[Node],
) -> ModifierList {
    let mut nodes: Vec<Node> = Vec::new();
    for &decorator in decorators {
        nodes.push(factory.clone_node(decorator));
    }
    nodes.extend(create_modifiers_from_modifier_flags(flags, &mut |kind| {
        factory.new_modifier(kind)
    }));
    if nodes.is_empty() {
        return ModifierList::NIL;
    }
    factory.new_modifier_list(&nodes)
}

// Go: ls/completions.go:2846 createSnippetTabStopBody
pub fn create_snippet_tab_stop_body(factory: &NodeFactory, emit_context: &EmitContext) -> Node {
    let empty_statement = factory.new_empty_statement();
    emit_context.set_snippet_element(
        empty_statement,
        SnippetElement {
            kind: SnippetKind::TAB_STOP,
            order: 0,
        },
    );
    factory.new_block(
        factory.new_node_list(&[empty_statement]),
        true, /*multiLine*/
    )
}

impl LanguageService {
    // Go: ls/completions.go:2855 createImportAdder
    // PORT: Go returns a nil-able `autoimport.ImportAdder` interface; here an
    // `Option<Box<dyn ImportAdder>>`. Go passes the checker; the Rust adder
    // does not store it (as in `getExhaustiveCaseSnippets`).
    pub fn create_import_adder(
        &self,
        ctx: &Context,
        type_checker: &mut Checker,
        file: Node,
    ) -> Result<Option<Box<dyn autoimport::ImportAdder>>, GoError> {
        if tspath::is_dynamic_file_name(source_file_file_name(file)) {
            return Ok(None);
        }
        let view = self.get_prepared_auto_import_view(file)?;
        let Some(view) = view else {
            return Ok(None);
        };
        Ok(Some(autoimport::new_import_adder(
            ctx,
            self.get_program(),
            file,
            view,
            self.format_options(),
            Rc::clone(&self.converters),
            self.user_preferences(),
        )))
    }
}

// Go: ls/completions.go:2869 isRecommendedCompletionMatch
pub fn is_recommended_completion_match(
    local_symbol: SymbolId,
    recommended_completion: SymbolId,
    type_checker: &mut Checker,
) -> bool {
    local_symbol == recommended_completion
        || type_checker
            .sym(local_symbol)
            .flags
            .intersects(SymbolFlags::EXPORT_VALUE)
            && type_checker.get_export_symbol_of_symbol(local_symbol) == recommended_completion
}

// Go: ls/completions.go:2875 wordSeparators
// Ported from vscode.
// PORT: Go `collections.Set[rune]`; a rune is an `i32`.
pub static WORD_SEPARATORS: [i32; 29] = [
    '`' as i32,
    '~' as i32,
    '!' as i32,
    '@' as i32,
    '%' as i32,
    '^' as i32,
    '&' as i32,
    '*' as i32,
    '(' as i32,
    ')' as i32,
    '-' as i32,
    '=' as i32,
    '+' as i32,
    '[' as i32,
    '{' as i32,
    ']' as i32,
    '}' as i32,
    '\\' as i32,
    '|' as i32,
    ';' as i32,
    ':' as i32,
    '\'' as i32,
    '"' as i32,
    ',' as i32,
    '.' as i32,
    '<' as i32,
    '>' as i32,
    '/' as i32,
    '?' as i32,
];

// Go: ls/completions.go:2882 getWordLengthAndStart
// Finds the length and first rune of the word that ends at the given position.
// e.g. for "abc def.ghi|jkl", the word length is 3 and the word start is 'g'.
// PORT: Go cuts `text := sourceFile.Text()[:position]`. The decoders read the
// whole text and stop at `position`, so no `&str` is cut inside a character.
// A position past the text panics there with Go's text
// (`go_check_slice_bounds`): the API in an LSP server with JSDoc completions
// off reaches it (with them on, the JSDoc snippet check panics first).
pub fn get_word_length_and_start(source_file: Node, position: i32) -> (i32, i32) {
    // !!! Port other case of vscode's `DEFAULT_WORD_REGEXP` that covers words that start like numbers, e.g. -123.456abcd.
    let text = source_file_text(source_file);
    crate::ls::utilities::go_check_slice_bounds(&text, 0, position);
    let text_len = position as usize;
    let mut total_size: i32 = 0;
    let mut first_rune: i32 = 0;
    let (mut r, mut size) = utf8_decode_last_rune_in_string(&text, text_len);
    while size != 0 {
        if WORD_SEPARATORS.contains(&r) || unicode_is_space(r) {
            break;
        }
        total_size += size;
        first_rune = r;
        (r, size) = utf8_decode_last_rune_in_string(&text, text_len - total_size as usize);
    }
    // If word starts with `@`, disregard this first character.
    if first_rune == '@' as i32 {
        total_size -= 1;
        (first_rune, _) = decode_rune_in_range(&text, text_len - total_size as usize, text_len);
    }
    (total_size, first_rune)
}

// Go: ls/completions.go:2905 trimElementAccess
// `["ab c"]` -> `ab c`
// `['ab c']` -> `ab c`
// `[123]` -> `123`
pub fn trim_element_access(text: &str) -> String {
    let mut text = text.strip_prefix('[').unwrap_or(text);
    text = text.strip_suffix(']').unwrap_or(text);
    if text.starts_with('\'') && text.ends_with('\'') {
        let trimmed = text.strip_suffix('\'').unwrap_or(text);
        text = trimmed.strip_prefix('\'').unwrap_or(trimmed);
    }
    if text.starts_with('"') && text.ends_with('"') {
        let trimmed = text.strip_suffix('"').unwrap_or(text);
        text = trimmed.strip_prefix('"').unwrap_or(trimmed);
    }
    text.to_string()
}

// Go: ls/completions.go:2918 getFilterText
// Ported from vscode ts extension: `getFilterText`.
pub fn get_filter_text(
    file: Node,
    position: i32,
    insert_text: &str,
    label: &str,
    word_start: i32,
    dot_accessor: &str,
) -> String {
    // Private field completion, e.g. label `#bar`.
    if let Some(after) = label.strip_prefix('#') {
        if !insert_text.is_empty() {
            if let Some(after) = insert_text.strip_prefix("this.#") {
                if word_start == '#' as i32 {
                    // `method() { this.#| }`
                    // `method() { #| }`
                    return String::new();
                } else {
                    // `method() { this.| }`
                    // `method() { | }`
                    return after.to_string();
                }
            }
        } else if word_start == '#' as i32 {
            // `method() { this.#| }`
            return String::new();
        } else {
            // `method() { this.| }`
            // `method() { | }`
            return after.to_string();
        }
    }

    // For `this.` completions, generally don't set the filter text since we don't want them to be overly deprioritized. microsoft/vscode#74164
    if insert_text.starts_with("this.") {
        return String::new();
    }

    // Handle the case:
    // ```
    // const xyz = { 'ab c': 1 };
    // xyz.ab|
    // ```
    // In which case we want to insert a bracket accessor but should use `.abc` as the filter text instead of
    // the bracketed insert text.
    if insert_text.starts_with('[') {
        return dot_accessor.to_string() + &trim_element_access(insert_text);
    }

    if insert_text.starts_with("?.") {
        // Handle this case like the case above:
        // ```
        // const xyz = { 'ab c': 1 } | undefined;
        // xyz.ab|
        // ```
        // filterText should be `.ab c` instead of `?.['ab c']`.
        if insert_text.starts_with("?.[") {
            return dot_accessor.to_string() + &trim_element_access(&insert_text[2..]);
        } else {
            // ```
            // const xyz = { abc: 1 } | undefined;
            // xyz.ab|
            // ```
            // filterText should be `.abc` instead of `?.abc.
            return dot_accessor.to_string() + &insert_text[2..];
        }
    }

    // In all other cases, fall back to using the insertText.
    insert_text.to_string()
}

// Go: ls/completions.go:2992 getDotAccessor
// Ported from vscode's `provideCompletionItems`.
pub fn get_dot_accessor(file: Node, position: i32) -> String {
    let full_text = source_file_text(file);
    let text = &full_text.as_bytes()[..position as usize];
    let mut total_size: i32 = 0;
    if text.ends_with(b"?.") {
        total_size += 2;
        return full_text[(position - total_size) as usize..position as usize].to_string();
    }
    if text.ends_with(b".") {
        total_size += 1;
        return full_text[(position - total_size) as usize..position as usize].to_string();
    }
    String::new()
}

// Go: ls/completions.go:3006 strPtrIsEmpty
pub fn str_ptr_is_empty(ptr: Option<String>) -> bool {
    match ptr {
        None => true,
        Some(ptr) => ptr.is_empty(),
    }
}

// Go: ls/completions.go:3013 strPtrTo
pub fn str_ptr_to(v: &str) -> Option<String> {
    if v.is_empty() {
        return None;
    }
    Some(v.to_string())
}

// Go: ls/completions.go:3020 boolToPtr
pub fn bool_to_ptr(v: bool) -> Option<bool> {
    if v {
        return Some(true);
    }
    None
}

// Go: ls/completions.go:3027 getLineOfPosition
pub fn get_line_of_position(file: Node, pos: i32) -> i32 {
    get_ecma_line_of_position(file, pos)
}

// Go: ls/completions.go:3032 getLineEndOfPosition
pub fn get_line_end_of_position(file: Node, pos: i32) -> i32 {
    let line = get_line_of_position(file, pos);
    let line_starts = &*get_ecma_line_starts(file);
    let last_char_pos: i32 = if (line + 1) as usize >= line_starts.len() {
        file.end()
    } else {
        line_starts[(line + 1) as usize] - 1
    };
    let full_text_text = source_file_text(file);
    let full_text = full_text_text.as_bytes();
    if last_char_pos > 0
        && (last_char_pos as usize) < full_text.len()
        && full_text[last_char_pos as usize] == b'\n'
        && full_text[last_char_pos as usize - 1] == b'\r'
    {
        return last_char_pos - 1;
    }
    last_char_pos
}

// Go: ls/completions.go:3048 isClassLikeMemberCompletion
// PORT: Go reads `symbol.Flags` through the pointer; `symbols` is the arena
// that holds `symbol`.
pub fn is_class_like_member_completion(
    symbols: &SymbolArena,
    symbol: SymbolId,
    location: Node,
    file: Node,
) -> bool {
    if is_in_js_file(location) {
        return false;
    }
    let member_flags = SymbolFlags::CLASS_MEMBER & SymbolFlags::ENUM_MEMBER_EXCLUDES;
    symbols.sym(symbol).flags.intersects(member_flags)
        && (is_class_like(location)
            || (location.parent().is_some()
                && location.parent().parent().is_some()
                && is_class_element(location.parent())
                && location == location.parent().name()
                && lsutil::get_last_token(location.parent(), file) == location.parent().name()
                && is_class_like(location.parent().parent()))
            || (location.parent().is_some()
                && is_syntax_list(location)
                && is_class_like(location.parent())))
}

// Go: ls/completions.go:3059 symbolAppearsToBeTypeOnly
pub fn symbol_appears_to_be_type_only(symbol: SymbolId, type_checker: &mut Checker) -> bool {
    let target = type_checker.skip_alias(symbol);
    let flags = type_checker
        .sym(target)
        .combined_local_and_export_symbol_flags(&type_checker.symbols);
    let declarations = &type_checker.sym(symbol).declarations;
    !flags.intersects(SymbolFlags::VALUE)
        && (declarations.is_empty()
            || !is_in_js_file(declarations[0])
            || flags.intersects(SymbolFlags::TYPE))
}

// Go: ls/completions.go:3065 shouldIncludeSymbol
pub fn should_include_symbol(
    symbol: SymbolId,
    data: &CompletionDataData,
    closest_symbol_declaration: Node,
    file: Node,
    type_checker: &mut Checker,
    compiler_options: &CompilerOptions,
) -> bool {
    let mut all_flags = type_checker.sym(symbol).flags;
    let location = data.location;
    // export = /**/ here we want to get all meanings, so any symbol is ok
    if location.parent().is_some() && is_export_assignment(location.parent()) {
        return true;
    }

    // Filter out variables from their own initializers
    // `const a = /* no 'a' here */`
    let value_declaration = type_checker.sym(symbol).value_declaration;
    if closest_symbol_declaration.is_some()
        && is_variable_declaration(closest_symbol_declaration)
        && value_declaration == closest_symbol_declaration
    {
        return false;
    }

    // Filter out current and latter parameters from defaults
    // `function f(a = /* no 'a' and 'b' here */, b) { }` or
    // `function f<T = /* no 'T' and 'T2' here */>(a: T, b: T2) { }`
    let mut symbol_declaration = Node::NIL;
    if value_declaration.is_some() {
        symbol_declaration = value_declaration;
    } else if !type_checker.sym(symbol).declarations.is_empty() {
        symbol_declaration = type_checker.sym(symbol).declarations[0];
    }

    if closest_symbol_declaration.is_some() && symbol_declaration.is_some() {
        if is_parameter_declaration(closest_symbol_declaration)
            && is_parameter_declaration(symbol_declaration)
        {
            let parameters = closest_symbol_declaration.parent().parameter_list();
            if symbol_declaration.pos() >= closest_symbol_declaration.pos()
                && symbol_declaration.pos() < parameters.end()
            {
                return false;
            }
        } else if is_type_parameter_declaration(closest_symbol_declaration)
            && is_type_parameter_declaration(symbol_declaration)
        {
            if closest_symbol_declaration == symbol_declaration
                && data.context_token.is_some()
                && data.context_token.kind() == SyntaxKind::ExtendsKeyword
            {
                // filter out the directly self-recursive type parameters
                // `type A<K extends /* no 'K' here*/> = K`
                return false;
            }
            if is_in_type_parameter_default(data.context_token)
                && !is_infer_type_node(closest_symbol_declaration.parent())
            {
                let type_parameters = closest_symbol_declaration.parent().type_parameter_list();
                if !type_parameters.is_nil()
                    && symbol_declaration.pos() >= closest_symbol_declaration.pos()
                    && symbol_declaration.pos() < type_parameters.end()
                {
                    return false;
                }
            }
        }
    }

    // External modules can have global export declarations that will be
    // available as global keywords in all scopes. But if the external module
    // already has an explicit export and user only wants to use explicit
    // module imports then the global keywords will be filtered out so auto
    // import suggestions will win in the completion.
    let symbol_origin = type_checker.skip_alias(symbol);
    // We only want to filter out the global keywords.
    // Auto Imports are not available for scripts so this conditional is always false.
    let symbol_parent = type_checker.sym(symbol).parent;
    if source_file_info(file).external_module_indicator.is_some()
        && compiler_options.allow_umd_global_access != Tristate::True
        && symbol != symbol_origin
        && data
            .symbol_to_sort_text_map
            .get(&get_symbol_id(&type_checker.symbols, symbol))
            .map_or("", |s| s.as_str())
            == SORT_TEXT_GLOBALS_OR_KEYWORDS
        && symbol_parent.is_some()
        && type_checker.is_external_module_symbol(symbol_parent)
    {
        return false;
    }

    all_flags = all_flags
        | type_checker
            .sym(symbol_origin)
            .combined_local_and_export_symbol_flags(&type_checker.symbols);
    if type_checker
        .sym(symbol)
        .flags
        .intersects(SymbolFlags::ALIAS)
    {
        all_flags = all_flags | type_checker.get_symbol_flags_exported(symbol);
    }

    // import m = /**/ <-- It can only access namespace (if typing import = x. this would get member symbols and not namespace)
    if is_in_right_side_of_internal_import_equals_declaration(data.location) {
        return all_flags.intersects(SymbolFlags::NAMESPACE);
    }

    if data.is_type_only_location {
        // It's a type, but you can reach it by namespace.type as well.
        return symbol_can_be_referenced_at_type_location(symbol, type_checker, None);
    }

    // expressions are value space (which includes the value namespaces)
    all_flags.intersects(SymbolFlags::VALUE)
}

// Go: ls/completions.go:3157 getCompletionEntryDisplayNameForSymbol
// PORT: `checker.IsKnownSymbol(symbol)` is `isLateBoundName(symbol.Name)`;
// it reads the arena directly (the port has it as a `Checker` method).
pub fn get_completion_entry_display_name_for_symbol(
    file: Node,
    preferences: &lsutil::UserPreferences,
    symbols: &SymbolArena,
    symbol: SymbolId,
    origin: Option<&SymbolOriginInfo>,
    completion_kind: CompletionKind,
    is_jsx_identifier_expected: bool,
) -> (String, bool) {
    if origin_is_ignore(origin) {
        return (String::new(), false);
    }

    let name: String = if origin_includes_symbol_name(origin) {
        origin.unwrap().symbol_name()
    } else {
        symbol_name(symbols, symbol)
    };
    let flags = symbols.sym(symbol).flags;
    if name.is_empty()
        // If the symbol is external module, don't show it in the completion list
        // (i.e declare module "http" { const x; } | // <= request completion here, "http" should not be there)
        || flags.intersects(SymbolFlags::MODULE) && starts_with_quote(&name)
        // If the symbol is the internal name of an ES symbol, it is not a valid entry. Internal names for ES symbols start with "__@"
        || crate::checker::is_late_bound_name(&symbols.sym(symbol).name)
    {
        return (String::new(), false);
    }

    let variant = if is_jsx_identifier_expected {
        LanguageVariant::JSX
    } else {
        LanguageVariant::STANDARD
    };
    // name is a valid identifier or private identifier text
    let value_declaration = symbols.sym(symbol).value_declaration;
    if is_identifier_text(&name, variant)
        || value_declaration.is_some()
            && is_private_identifier_class_element_declaration(value_declaration)
    {
        return (name, false);
    }
    if flags.intersects(SymbolFlags::ALIAS) {
        // Allow non-identifier import/export aliases since we can insert them as string literals
        return (name, true);
    }

    match completion_kind {
        CompletionKind::MEMBER_LIKE => {
            if origin_is_computed_property_name(origin) {
                return (origin.unwrap().symbol_name(), false);
            }
            (String::new(), false)
        }
        CompletionKind::OBJECT_PROPERTY_DECLARATION => (quote(file, preferences, &name), false),
        CompletionKind::PROPERTY_ACCESS | CompletionKind::GLOBAL => {
            // For a 'this.' completion it will be in a global context, but may have a non-identifier name.
            // Don't add a completion for a name starting with a space. See https://github.com/Microsoft/TypeScript/pull/20547
            let (ch, _) = utf8_decode_rune_in_string(&name, 0);
            if ch == ' ' as i32 {
                return (String::new(), false);
            }
            (name, true)
        }
        CompletionKind::NONE | CompletionKind::STRING => (name, false),
        _ => crate::core::go_panic(format!("Unexpected completion kind: {}", completion_kind.0)),
    }
}

// !!! refactor symbolOriginInfo so that we can tell the difference between flags and the kind of data it has
// Go: ls/completions.go:3219 originIsIgnore
pub fn origin_is_ignore(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| origin.kind.intersects(SymbolOriginInfoKind::IGNORE))
}

// Go: ls/completions.go:3223 originIncludesSymbolName
pub fn origin_includes_symbol_name(origin: Option<&SymbolOriginInfo>) -> bool {
    origin_is_computed_property_name(origin)
}

// Go: ls/completions.go:3227 originIsComputedPropertyName
pub fn origin_is_computed_property_name(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| {
        origin
            .kind
            .intersects(SymbolOriginInfoKind::COMPUTED_PROPERTY_NAME)
    })
}

// Go: ls/completions.go:3231 originIsObjectLiteralMethod
pub fn origin_is_object_literal_method(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| {
        origin
            .kind
            .intersects(SymbolOriginInfoKind::OBJECT_LITERAL_METHOD)
    })
}

// Go: ls/completions.go:3235 originIsThisTypeNode
pub fn origin_is_this_type_node(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| origin.kind.intersects(SymbolOriginInfoKind::THIS_TYPE))
}

// Go: ls/completions.go:3239 originIsTypeOnlyAlias
pub fn origin_is_type_only_alias(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| {
        origin
            .kind
            .intersects(SymbolOriginInfoKind::TYPE_ONLY_ALIAS)
    })
}

// Go: ls/completions.go:3243 originIsSymbolMember
pub fn origin_is_symbol_member(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| origin.kind.intersects(SymbolOriginInfoKind::SYMBOL_MEMBER))
}

// Go: ls/completions.go:3247 originIsNullableMember
pub fn origin_is_nullable_member(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| origin.kind.intersects(SymbolOriginInfoKind::NULLABLE))
}

// Go: ls/completions.go:3251 originIsPromise
pub fn origin_is_promise(origin: Option<&SymbolOriginInfo>) -> bool {
    origin.is_some_and(|origin| origin.kind.intersects(SymbolOriginInfoKind::PROMISE))
}

// Go: ls/completions.go:3255 getSourceFromOrigin
pub fn get_source_from_origin(origin: Option<&SymbolOriginInfo>) -> String {
    if origin_is_this_type_node(origin) {
        return COMPLETION_SOURCE_THIS_PROPERTY.to_string();
    }

    if origin_is_type_only_alias(origin) {
        return COMPLETION_SOURCE_TYPE_ONLY_ALIAS.to_string();
    }

    String::new()
}

// Go: ls/completions.go:3270 getRelevantTokens
// In a scenarion such as `const x = 1 * |`, the context and previous tokens are both `*`.
// In `const x = 1 * o|`, the context token is *, and the previous token is `o`.
// `contextToken` and `previousToken` can both be nil if we are at the beginning of the file.
// PORT: returns `(contextToken, previousToken)`.
pub fn get_relevant_tokens(position: i32, file: Node) -> (Node, Node) {
    let previous_token = astnav::find_preceding_token(file, position);
    if previous_token.is_some()
        && position <= previous_token.end()
        && (is_member_name(previous_token) || is_keyword_kind(previous_token.kind()))
    {
        let context_token = astnav::find_preceding_token(file, previous_token.pos());
        return (context_token, previous_token);
    }
    (previous_token, previous_token)
}

// Go: ls/completions.go:3280 CompletionsTriggerCharacter
// "." | '"' | "'" | "`" | "/" | "@" | "<" | "#" | " " | "*"
pub type CompletionsTriggerCharacter = String;

// Go: ls/completions.go:3282 isValidTrigger
// PORT: the `CompletionsTriggerCharacter` (Go string) parameter is `&str`.
pub fn is_valid_trigger(
    file: Node,
    trigger_character: &str,
    context_token: Node,
    position: i32,
) -> bool {
    match trigger_character {
        "." | "@" => true,
        "\"" | "'" | "`" => {
            // Only automatically bring up completions if this is an opening quote.
            context_token.is_some()
                && is_string_literal_or_template(context_token)
                && position
                    == astnav::get_start_of_node(context_token, file, false /*includeJSDoc*/) + 1
        }
        "#" => {
            context_token.is_some()
                && is_private_identifier(context_token)
                && get_containing_class(context_token).is_some()
        }
        "<" => {
            // Opening JSX tag
            context_token.is_some()
                && context_token.kind() == SyntaxKind::LessThanToken
                && (!is_binary_expression(context_token.parent())
                    || binary_expression_may_be_open_tag(context_token.parent()))
        }
        "/" => {
            if context_token.is_nil() {
                return false;
            }
            if is_string_literal_like(context_token) {
                return try_get_import_from_module_specifier(context_token).is_some();
            }
            context_token.kind() == SyntaxKind::LessThanSlashToken
                && is_jsx_closing_element(context_token.parent())
        }
        " " => {
            context_token.is_some()
                && context_token.kind() == SyntaxKind::ImportKeyword
                && context_token.parent().kind() == SyntaxKind::SourceFile
        }
        "*" => is_potentially_valid_js_doc_snippet_completion_position(file, position),
        _ => crate::core::go_panic(format!("Unknown trigger character: {trigger_character}")),
    }
}

// Go: ls/completions.go:3317 isStringLiteralOrTemplate
pub fn is_string_literal_or_template(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateExpression
            | SyntaxKind::TaggedTemplateExpression
    )
}

// Go: ls/completions.go:3326 binaryExpressionMayBeOpenTag
pub fn binary_expression_may_be_open_tag(binary_expression: Node) -> bool {
    node_is_missing(binary_expression.left())
}

// Go: ls/completions.go:3330 isCheckedFile
pub fn is_checked_file(file: Node, compiler_options: &CompilerOptions) -> bool {
    !is_source_file_js(file) || is_check_js_enabled_for_file(file, compiler_options)
}

// Go: ls/completions.go:3334 isContextTokenValueLocation
pub fn is_context_token_value_location(context_token: Node) -> bool {
    context_token.is_some()
        && ((context_token.kind() == SyntaxKind::TypeOfKeyword
            && (context_token.parent().kind() == SyntaxKind::TypeQuery
                || is_type_of_expression(context_token.parent())))
            || (context_token.kind() == SyntaxKind::AssertsKeyword
                && context_token.parent().kind() == SyntaxKind::TypePredicate))
}

// Go: ls/completions.go:3340 isPossiblyTypeArgumentPosition
pub fn is_possibly_type_argument_position(
    token: Node,
    source_file: Node,
    type_checker: &mut Checker,
) -> bool {
    let info = get_possible_type_arguments_info(token, source_file);
    match info {
        None => false,
        Some(info) => {
            is_part_of_type_node(info.called)
                || !get_possible_generic_signatures(
                    info.called,
                    info.n_type_arguments,
                    type_checker,
                )
                .is_empty()
                || is_possibly_type_argument_position(info.called, source_file, type_checker)
        }
    }
}

// Go: ls/completions.go:3347 isContextTokenTypeLocation
pub fn is_context_token_type_location(context_token: Node) -> bool {
    if context_token.is_some() {
        let parent_kind = context_token.parent().kind();
        match context_token.kind() {
            SyntaxKind::ColonToken => {
                return parent_kind == SyntaxKind::PropertyDeclaration
                    || parent_kind == SyntaxKind::PropertySignature
                    || parent_kind == SyntaxKind::Parameter
                    || parent_kind == SyntaxKind::VariableDeclaration
                    || is_function_like_kind(parent_kind);
            }
            SyntaxKind::EqualsToken => {
                return parent_kind == SyntaxKind::TypeAliasDeclaration
                    || parent_kind == SyntaxKind::TypeParameter;
            }
            SyntaxKind::AsKeyword => {
                return parent_kind == SyntaxKind::AsExpression;
            }
            SyntaxKind::LessThanToken => {
                return parent_kind == SyntaxKind::TypeReference
                    || parent_kind == SyntaxKind::TypeAssertionExpression;
            }
            SyntaxKind::ExtendsKeyword => {
                return parent_kind == SyntaxKind::TypeParameter;
            }
            SyntaxKind::SatisfiesKeyword => {
                return parent_kind == SyntaxKind::SatisfiesExpression;
            }
            SyntaxKind::OpenBracketToken | SyntaxKind::CommaToken => {
                return parent_kind == SyntaxKind::TupleType;
            }
            _ => {}
        }
    }
    false
}

/// Go `collections.Set[ast.SymbolId]` passed by value
/// (`symbolCanBeReferencedAtTypeLocation`, completions.go:2792).
// PORT: Go copies the `Set` struct on each call. A copy shares the map once
// the map exists; `Add` on a nil map makes a new map in that copy only.
// `None` is the nil map; a clone of `Some` shares the one map. Callers that
// pass Go's `collections.Set[ast.SymbolId]{}` pass `None`.
pub type SymbolIdSetValue = Option<Rc<RefCell<FxHashSet<u64>>>>;

// Go: collections/set.go:57 (*Set).AddIfAbsent, on a by-value `Set` copy.
fn symbol_id_set_add_if_absent(set: &mut SymbolIdSetValue, key: u64) -> bool {
    // Go: Has
    if let Some(m) = set.as_ref() {
        if RefCell::borrow(m).contains(&key) {
            return false;
        }
    }
    // Go: Add (makes the map in this copy when it is nil)
    RefCell::borrow_mut(set.get_or_insert_with(Default::default)).insert(key);
    true
}

// Go: ls/completions.go:3375 symbolCanBeReferencedAtTypeLocation
// True if symbol is a type or a module containing at least one type.
pub fn symbol_can_be_referenced_at_type_location(
    symbol: SymbolId,
    type_checker: &mut Checker,
    seen_modules: SymbolIdSetValue,
) -> bool {
    // Since an alias can be merged with a local declaration, we need to test both the alias and its target.
    // This code used to just test the result of `skipAlias`, but that would ignore any locally introduced meanings.
    non_alias_can_be_referenced_at_type_location(symbol, type_checker, seen_modules.clone()) || {
        let export_symbol = type_checker.sym(symbol).export_symbol;
        let target = type_checker.skip_alias(if export_symbol.is_some() {
            export_symbol
        } else {
            symbol
        });
        non_alias_can_be_referenced_at_type_location(target, type_checker, seen_modules)
    }
}

// Go: ls/completions.go:3386 nonAliasCanBeReferencedAtTypeLocation
pub fn non_alias_can_be_referenced_at_type_location(
    symbol: SymbolId,
    type_checker: &mut Checker,
    mut seen_modules: SymbolIdSetValue,
) -> bool {
    let flags = type_checker.sym(symbol).flags;
    flags.intersects(SymbolFlags::TYPE)
        || type_checker.is_unknown_symbol(symbol)
        || flags.intersects(SymbolFlags::MODULE)
            && symbol_id_set_add_if_absent(
                &mut seen_modules,
                get_symbol_id(&type_checker.symbols, symbol),
            )
            && type_checker
                .get_exports_of_module_exported(symbol)
                .into_iter()
                .any(|e| {
                    symbol_can_be_referenced_at_type_location(e, type_checker, seen_modules.clone())
                })
}

// Go: core/core.go:705 CheckEachDefined
fn check_each_defined(s: Vec<SymbolId>, msg: &str) -> Vec<SymbolId> {
    for value in &s {
        if value.is_nil() {
            crate::core::go_panic(msg.to_string());
        }
    }
    s
}

// Go: ls/completions.go:3397 getPropertiesForCompletion
// Gets all properties on a type, but if that type is a union of several types,
// excludes array-like types or callable/constructable types.
pub fn get_properties_for_completion(t: TypeId, type_checker: &mut Checker) -> Vec<SymbolId> {
    if type_checker.ty(t).is_union() {
        let types = type_checker.ty(t).types().to_vec();
        check_each_defined(
            type_checker.get_all_possible_properties_of_types(&types),
            "getAllPossiblePropertiesOfTypes() should all be defined.",
        )
    } else {
        check_each_defined(
            type_checker.get_apparent_properties(t),
            "getApparentProperties() should all be defined.",
        )
    }
}

// Go: ls/completions.go:3406 getLeftMostName
// Given 'a.b.c', returns 'a'.
pub fn get_left_most_name(e: Node) -> Node {
    if is_identifier(e) {
        e
    } else if is_property_access_expression(e) {
        get_left_most_name(e.expression())
    } else {
        Node::NIL
    }
}

// Go: ls/completions.go:3416 getFirstSymbolInChain
pub fn get_first_symbol_in_chain(
    symbol: SymbolId,
    enclosing_declaration: Node,
    type_checker: &mut Checker,
) -> SymbolId {
    let chain = type_checker.get_accessible_symbol_chain_exported(
        symbol,
        enclosing_declaration,
        SymbolFlags::ALL, /*meaning*/
        false,            /*useOnlyExternalAliasing*/
    );
    if !chain.is_empty() {
        return chain[0];
    }
    let parent = type_checker.sym(symbol).parent;
    if parent.is_some() {
        if is_module_symbol(&type_checker.symbols, parent) {
            return symbol;
        }
        return get_first_symbol_in_chain(parent, enclosing_declaration, type_checker);
    }
    SymbolId::NIL
}

// Go: ls/completions.go:3435 isModuleSymbol
pub fn is_module_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    symbols
        .sym(symbol)
        .declarations
        .iter()
        .any(|decl| decl.kind() == SyntaxKind::SourceFile)
}

// Go: ls/completions.go:3439 getNullableSymbolOriginInfoKind
pub fn get_nullable_symbol_origin_info_kind(
    kind: SymbolOriginInfoKind,
    insert_question_dot: bool,
) -> SymbolOriginInfoKind {
    let mut kind = kind;
    if insert_question_dot {
        kind |= SymbolOriginInfoKind::NULLABLE;
    }
    kind
}

// Go: ls/completions.go:3446 isStaticProperty
pub fn is_static_property(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let value_declaration = symbols.sym(symbol).value_declaration;
    value_declaration.is_some()
        && value_declaration
            .modifier_flags()
            .intersects(ModifierFlags::STATIC)
        && is_class_like(value_declaration.parent())
}

// Go: ls/completions.go:3454 getContextualTypeForConditionalExpression
// getContextualTypeForConditionalExpression handles completion within a conditional expression
// (ternary operator) by using the parent expression to find the contextual type.
pub fn get_contextual_type_for_conditional_expression(
    conditional_expr: Node,
    position: i32,
    file: Node,
    type_checker: &mut Checker,
) -> TypeId {
    let arg_info =
        get_argument_info_for_completions(conditional_expr, position, file, type_checker);
    if let Some(arg_info) = arg_info {
        return type_checker.get_contextual_type_for_argument_at_index_exported(
            arg_info.invocation,
            arg_info.argument_index,
        );
    }
    // Fall through to regular contextual type logic if not in an argument
    let contextual_type = type_checker
        .get_contextual_type_exported(conditional_expr, ContextFlags::IGNORE_NODE_INFERENCES);
    if contextual_type.is_some() {
        return contextual_type;
    }
    type_checker.get_contextual_type_exported(conditional_expr, ContextFlags::NONE)
}

// Go: ls/completions.go:3467 getContextualType
pub fn get_contextual_type(
    previous_token: Node,
    position: i32,
    file: Node,
    type_checker: &mut Checker,
) -> TypeId {
    let parent = previous_token.parent();
    match previous_token.kind() {
        SyntaxKind::Identifier => {
            return get_contextual_type_from_parent(
                previous_token,
                type_checker,
                ContextFlags::NONE,
            );
        }
        SyntaxKind::EqualsToken => {
            return match parent.kind() {
                SyntaxKind::VariableDeclaration => type_checker
                    .get_contextual_type_exported(parent.initializer(), ContextFlags::NONE),
                SyntaxKind::BinaryExpression => type_checker.get_type_at_location(parent.left()),
                SyntaxKind::JsxAttribute => {
                    type_checker.get_contextual_type_for_jsx_attribute_exported(parent)
                }
                _ => TypeId::NIL,
            };
        }
        SyntaxKind::NewKeyword => {
            return type_checker.get_contextual_type_exported(parent, ContextFlags::NONE);
        }
        SyntaxKind::CaseKeyword => {
            let case_clause = if is_case_clause(parent) {
                parent
            } else {
                Node::NIL
            };
            if case_clause.is_some() {
                return get_switched_type(case_clause, type_checker);
            }
            return TypeId::NIL;
        }
        SyntaxKind::OpenBraceToken => {
            if is_jsx_expression(parent)
                && !is_jsx_element(parent.parent())
                && !is_jsx_fragment(parent.parent())
            {
                return type_checker
                    .get_contextual_type_for_jsx_attribute_exported(parent.parent());
            }
            return TypeId::NIL;
        }
        SyntaxKind::OpenBracketToken => {
            // When completing after `[` in an array literal (e.g., `[/*here*/]`),
            // we should provide contextual type for the first element
            if is_array_literal_expression(parent) {
                let contextual_array_type =
                    type_checker.get_contextual_type_exported(parent, ContextFlags::NONE);
                if contextual_array_type.is_some() {
                    // Get the type for the first element (index 0)
                    return type_checker.get_contextual_type_for_array_literal_at_position(
                        contextual_array_type,
                        parent,
                        position,
                    );
                }
            }
            return TypeId::NIL;
        }
        SyntaxKind::CloseBracketToken => {
            // When completing after `]` (e.g., `[x]/*here*/`), we should not provide a contextual type
            // for the closing bracket token itself. Without this case, CloseBracketToken would fall through
            // to the default case, and if the parent is an array literal, GetContextualType would try to
            // find the token's index in the array elements (returning -1), leading to an out-of-bounds panic
            // in getContextualTypeForElementExpression.
            return TypeId::NIL;
        }
        SyntaxKind::QuestionToken => {
            // When completing after `?` in a ternary conditional (e.g., `foo(a ? /*here*/)`),
            // we need to look at the parent conditional expression to find the contextual type.
            if is_conditional_expression(parent) {
                return get_contextual_type_for_conditional_expression(
                    parent,
                    position,
                    file,
                    type_checker,
                );
            }
            return TypeId::NIL;
        }
        SyntaxKind::ColonToken => {
            // When completing after `:` in a ternary conditional (e.g., `foo(a ? b : /*here*/)`),
            // we need to look at the parent conditional expression to find the contextual type.
            // Only handle this if parent is ConditionalExpression, otherwise fall through to default
            // (colons are used in other contexts like object literals, type annotations, etc.)
            if is_conditional_expression(parent) {
                return get_contextual_type_for_conditional_expression(
                    parent,
                    position,
                    file,
                    type_checker,
                );
            }
        }
        SyntaxKind::CommaToken => {
            // When completing after `,` in an array literal (e.g., `[x, /*here*/]`),
            // we should provide contextual type for the element after the comma.
            if is_array_literal_expression(parent) {
                let contextual_array_type =
                    type_checker.get_contextual_type_exported(parent, ContextFlags::NONE);
                if contextual_array_type.is_some() {
                    return type_checker.get_contextual_type_for_array_literal_at_position(
                        contextual_array_type,
                        parent,
                        position,
                    );
                }
                return TypeId::NIL;
            }
        }
        _ => {}
    }
    // Default case: see if we're in an argument position.
    let arg_info = get_argument_info_for_completions(previous_token, position, file, type_checker);
    if let Some(arg_info) = arg_info {
        type_checker.get_contextual_type_for_argument_at_index_exported(
            arg_info.invocation,
            arg_info.argument_index,
        )
    } else if is_equality_operator_kind(previous_token.kind())
        && is_binary_expression(parent)
        && is_equality_operator_kind(parent.operator_token().kind())
    {
        // completion at `x ===/**/`
        type_checker.get_type_at_location(parent.left())
    } else {
        let contextual_type = type_checker
            .get_contextual_type_exported(previous_token, ContextFlags::IGNORE_NODE_INFERENCES);
        if contextual_type.is_some() {
            return contextual_type;
        }
        type_checker.get_contextual_type_exported(previous_token, ContextFlags::NONE)
    }
}

// Go: ls/completions.go:3556 getSwitchedType
pub fn get_switched_type(case_clause: Node, type_checker: &mut Checker) -> TypeId {
    type_checker.get_type_at_location(case_clause.parent().parent().expression())
}

// Go: ls/completions.go:3560 isEqualityOperatorKind
pub fn is_equality_operator_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::EqualsEqualsEqualsToken
            | SyntaxKind::EqualsEqualsToken
            | SyntaxKind::ExclamationEqualsEqualsToken
            | SyntaxKind::ExclamationEqualsToken
    )
}

// Go: ls/completions.go:3571 isLiteral
// We disregard boolean literals for completion purposes.
// PORT: Go reads the `*checker.Type`; the port reads the checker arena.
pub fn is_literal(type_checker: &Checker, t: TypeId) -> bool {
    let t = type_checker.ty(t);
    t.is_string_literal() || t.is_number_literal() || t.is_big_int_literal()
}

// Go: ls/completions.go:3575 getRecommendedCompletion
pub fn get_recommended_completion(
    previous_token: Node,
    contextual_type: TypeId,
    type_checker: &mut Checker,
) -> SymbolId {
    let types: Vec<TypeId> = if type_checker.ty(contextual_type).is_union() {
        type_checker.ty(contextual_type).types().to_vec()
    } else {
        vec![contextual_type]
    };
    // For a union, return the first one with a recommended completion.
    // Go: core.FirstNonNil
    for t in types {
        let symbol = type_checker.ty(t).symbol();
        // Don't make a recommended completion for an abstract class.
        let result = if symbol.is_some()
            && type_checker
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::ENUM_MEMBER | SymbolFlags::ENUM | SymbolFlags::CLASS)
            && !is_abstract_constructor_symbol(&type_checker.symbols, symbol)
        {
            get_first_symbol_in_chain(symbol, previous_token, type_checker)
        } else {
            SymbolId::NIL
        };
        if result.is_some() {
            return result;
        }
    }
    SymbolId::NIL
}

// Go: ls/completions.go:3598 isAbstractConstructorSymbol
pub fn is_abstract_constructor_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    if symbols.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
        let declaration = get_class_like_declaration_of_symbol(symbols, symbol);
        return declaration.is_some()
            && has_syntactic_modifier(declaration, ModifierFlags::ABSTRACT);
    }
    false
}

// Go: ls/completions.go:3606 startsWithQuote
pub fn starts_with_quote(s: &str) -> bool {
    let (r, _) = utf8_decode_rune_in_string(s, 0);
    r == '"' as i32 || r == '\'' as i32
}

// Go: ls/completions.go:3611 getClosestSymbolDeclaration
pub fn get_closest_symbol_declaration(context_token: Node, location: Node) -> Node {
    if context_token.is_nil() {
        return Node::NIL;
    }

    let mut closest_declaration =
        find_ancestor_or_quit(context_token, |node: Node| -> FindAncestorResult {
            if is_function_block(node) || is_arrow_function_body(node) || is_binding_pattern(node) {
                return FindAncestorResult::FIND_ANCESTOR_QUIT;
            }

            if (is_parameter_declaration(node) || is_type_parameter_declaration(node))
                && !is_index_signature_declaration(node.parent())
            {
                return FindAncestorResult::FIND_ANCESTOR_TRUE;
            }
            FindAncestorResult::FIND_ANCESTOR_FALSE
        });

    if closest_declaration.is_nil() {
        closest_declaration = find_ancestor_or_quit(location, |node: Node| -> FindAncestorResult {
            if is_function_block(node) || is_arrow_function_body(node) || is_binding_pattern(node) {
                return FindAncestorResult::FIND_ANCESTOR_QUIT;
            }

            if is_variable_declaration(node) {
                return FindAncestorResult::FIND_ANCESTOR_TRUE;
            }
            FindAncestorResult::FIND_ANCESTOR_FALSE
        });
    }
    closest_declaration
}

// Go: ls/completions.go:3643 isArrowFunctionBody
pub fn is_arrow_function_body(node: Node) -> bool {
    node.parent().is_some()
        && is_arrow_function(node.parent())
        && (node.parent().body() == node ||
            // const a = () => /**/;
            node.kind() == SyntaxKind::EqualsGreaterThanToken)
}

// Go: ls/completions.go:3650 isInTypeParameterDefault
pub fn is_in_type_parameter_default(context_token: Node) -> bool {
    if context_token.is_nil() {
        return false;
    }

    let mut node = context_token;
    let mut parent = context_token.parent();
    while parent.is_some() {
        if is_type_parameter_declaration(parent) {
            return parent.default_type() == node || node.kind() == SyntaxKind::EqualsToken;
        }
        node = parent;
        parent = parent.parent();
    }

    false
}

// Go: ls/completions.go:3668 isDeprecated
pub fn is_deprecated(symbol: SymbolId, type_checker: &mut Checker) -> bool {
    let target = type_checker.skip_alias(symbol);
    let declarations: Vec<Node> = type_checker.sym(target).declarations.to_vec();
    !declarations.is_empty()
        && declarations
            .iter()
            .all(|&decl| type_checker.is_deprecated_declaration(decl))
}

impl LanguageService {
    // Go: ls/completions.go:3673 getReplacementRangeForContextToken
    pub fn get_replacement_range_for_context_token(
        &self,
        file: Node,
        context_token: Node,
        position: i32,
    ) -> Option<lsproto::Range> {
        if context_token.is_nil() {
            return None;
        }

        // !!! ensure range is single line
        match context_token.kind() {
            SyntaxKind::StringLiteral | SyntaxKind::NoSubstitutionTemplateLiteral => {
                self.create_range_from_string_literal_like_content(file, context_token, position)
            }
            _ => {
                let (lsp_range, fidelity) = self.create_lsp_range_from_node(context_token, file);
                if !fidelity.is_exact() {
                    return None;
                }
                Some(lsp_range)
            }
        }
    }

    // Go: ls/completions.go:3691 createRangeFromStringLiteralLikeContent
    pub fn create_range_from_string_literal_like_content(
        &self,
        file: Node,
        node: Node,
        position: i32,
    ) -> Option<lsproto::Range> {
        let mut replacement_end = node.end() - 1;
        let node_start = astnav::get_start_of_node(node, file, false /*includeJSDoc*/);
        if is_unterminated_literal(node) {
            // we return no replacement range only if unterminated string is empty
            if node_start == replacement_end {
                return None;
            }
            replacement_end = position.min(node.end());
        }
        let (lsp_range, fidelity) =
            self.create_lsp_range_from_bounds(node_start + 1, replacement_end, file);
        if !fidelity.is_exact() {
            return None;
        }
        Some(lsp_range)
    }
}

// Go: ls/completions.go:3708 quotePropertyName
pub fn quote_property_name(
    file: Node,
    preferences: &lsutil::UserPreferences,
    name: &str,
) -> String {
    let (r, _) = utf8_decode_rune_in_string(name, 0);
    // Go `unicode.IsDigit` (go1.27.1, Unicode 17.0.0). The decoded rune is
    // a char: RuneError (U+FFFD) for an empty name or a bad byte.
    if char::from_u32(r as u32).is_some_and(unicode::is_digit) {
        return name.to_string();
    }
    quote(file, preferences, name)
}

// Go: ls/completions.go:3719 isStringAndEmptyAnonymousObjectIntersection
// Checks whether type is `string & {}`, which is semantically equivalent to string but
// is not reduced by the checker as a special case used for supporting string literal completions
// for string type.
pub fn is_string_and_empty_anonymous_object_intersection(
    type_checker: &mut Checker,
    t: TypeId,
) -> bool {
    if !type_checker.ty(t).is_intersection() {
        return false;
    }

    let types = type_checker.ty(t).types().to_vec();
    types.len() == 2
        && (are_intersected_types_avoiding_string_reduction(type_checker, types[0], types[1])
            || are_intersected_types_avoiding_string_reduction(type_checker, types[1], types[0]))
}

// Go: ls/completions.go:3729 areIntersectedTypesAvoidingStringReduction
pub fn are_intersected_types_avoiding_string_reduction(
    type_checker: &mut Checker,
    t1: TypeId,
    t2: TypeId,
) -> bool {
    type_checker.ty(t1).is_string() && type_checker.is_empty_anonymous_object_type(t2)
}

// Go: ls/completions.go:3733 escapeSnippetText
pub fn escape_snippet_text(text: &str) -> String {
    text.replace('$', "\\$")
}

// Go: ls/completions.go:3737 isNamedImportsOrExports
pub fn is_named_imports_or_exports(node: Node) -> bool {
    is_named_imports(node) || is_named_exports(node)
}

// Go: ls/completions.go:3741 generateIdentifierForArbitraryString
pub fn generate_identifier_for_arbitrary_string(text: &str) -> String {
    let mut needs_underscore = false;
    let mut identifier = String::new();

    // Convert "(example, text)" into "_example_text_"
    let mut pos: usize = 0;
    while pos < text.len() {
        let (ch, size) = utf8_decode_rune_in_string(text, pos);
        let c = rune_to_char(ch);
        let valid_char = if pos == 0 {
            is_identifier_start(c)
        } else {
            is_identifier_part(c)
        };
        if size > 0 && valid_char {
            if needs_underscore {
                identifier.push('_');
            }
            identifier.push(c);
            needs_underscore = false;
        } else {
            needs_underscore = true;
        }
        pos += size as usize;
    }

    if needs_underscore {
        identifier.push('_');
    }

    // Default to "_" if the provided text was empty
    if identifier.is_empty() {
        return "_".to_string();
    }

    identifier
}

// Go: ls/completions.go:3781 getCompletionsSymbolKind
// Copied from vscode TS extension.
pub fn get_completions_symbol_kind(kind: lsutil::ScriptElementKind) -> lsproto::CompletionItemKind {
    use lsutil::ScriptElementKind as K;
    match kind {
        K::PRIMITIVE_TYPE | K::KEYWORD => lsproto::CompletionItemKind::KEYWORD,
        K::CONST_ELEMENT
        | K::LET_ELEMENT
        | K::VARIABLE_ELEMENT
        | K::LOCAL_VARIABLE_ELEMENT
        | K::ALIAS
        | K::PARAMETER_ELEMENT => lsproto::CompletionItemKind::VARIABLE,

        K::MEMBER_VARIABLE_ELEMENT
        | K::MEMBER_GET_ACCESSOR_ELEMENT
        | K::MEMBER_SET_ACCESSOR_ELEMENT => lsproto::CompletionItemKind::FIELD,

        K::FUNCTION_ELEMENT | K::LOCAL_FUNCTION_ELEMENT => lsproto::CompletionItemKind::FUNCTION,

        K::MEMBER_FUNCTION_ELEMENT
        | K::CONSTRUCT_SIGNATURE_ELEMENT
        | K::CALL_SIGNATURE_ELEMENT
        | K::INDEX_SIGNATURE_ELEMENT => lsproto::CompletionItemKind::METHOD,

        K::ENUM_ELEMENT => lsproto::CompletionItemKind::ENUM,

        K::ENUM_MEMBER_ELEMENT => lsproto::CompletionItemKind::ENUM_MEMBER,

        K::MODULE_ELEMENT | K::EXTERNAL_MODULE_NAME => lsproto::CompletionItemKind::MODULE,

        K::CLASS_ELEMENT | K::TYPE_ELEMENT => lsproto::CompletionItemKind::CLASS,

        K::INTERFACE_ELEMENT => lsproto::CompletionItemKind::INTERFACE,

        K::WARNING => lsproto::CompletionItemKind::TEXT,

        K::SCRIPT_ELEMENT => lsproto::CompletionItemKind::FILE,

        K::DIRECTORY => lsproto::CompletionItemKind::FOLDER,

        K::STRING => lsproto::CompletionItemKind::CONSTANT,

        _ => lsproto::CompletionItemKind::PROPERTY,
    }
}

// Go: ls/completions.go:3836 CompareCompletionEntries
// Editors will use the `sortText` and then fall back to `name` for sorting, but leave ties in response order.
// So, it's important that we sort those ties in the order we want them displayed if it matters. We don't
// strictly need to sort by name or SortText here since clients are going to do it anyway, but we have to
// do the work of comparing them so we can sort those ties appropriately.
// PORT: Go dereferences `SortText`; a nil pointer panics there.
pub fn compare_completion_entries(a: &lsproto::CompletionItem, b: &lsproto::CompletionItem) -> i32 {
    let compare_strings = stringutil_ls::compare_strings_case_insensitive_then_sensitive;
    let mut result = compare_strings(
        a.sort_text
            .as_deref()
            .unwrap_or_else(|| crate::core::go_nil_dereference()),
        b.sort_text
            .as_deref()
            .unwrap_or_else(|| crate::core::go_nil_dereference()),
    );
    if result == stringutil_ls::COMPARISON_EQUAL {
        result = compare_strings(&a.label, &b.label);
    }
    result
}

thread_local! {
    // Go: ls/completions.go:3846 keywordCompletionsCache
    // PORT: Go `collections.SyncMap[KeywordCompletionFilters, ...]`; the key
    // is the filter value. One map per thread (every request runs on the
    // dispatch thread).
    static KEYWORD_COMPLETIONS_CACHE: RefCell<FxHashMap<i32, Vec<lsproto::CompletionItem>>> =
        RefCell::new(FxHashMap::default());

    // Go: ls/completions.go:3847 allKeywordCompletions (sync.OnceValue)
    static ALL_KEYWORD_COMPLETIONS: Vec<lsproto::CompletionItem> = {
        let first = SyntaxKind::FIRST_KEYWORD as u16;
        let last = SyntaxKind::LAST_KEYWORD as u16;
        let mut result = Vec::with_capacity((last - first + 1) as usize);
        for i in first..=last {
            let kind = SyntaxKind::try_from(i).expect("keyword kind");
            result.push(lsproto::CompletionItem {
                label: token_to_string(kind).to_string(),
                kind: Some(lsproto::CompletionItemKind::KEYWORD),
                sort_text: Some(SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string()),
                ..Default::default()
            });
        }
        result
    };
}

// Go: ls/completions.go:3847 allKeywordCompletions
// PORT: Go returns the shared slice; the port returns a copy. Callers copy
// every item before they change it (`cloneItems`).
pub fn all_keyword_completions() -> Vec<lsproto::CompletionItem> {
    ALL_KEYWORD_COMPLETIONS.with(|items| items.clone())
}

// Go: ls/completions.go:3860 cloneItems
// PORT: Go returns nil for a nil input; a `Vec` has no nil, so both are empty.
pub fn clone_items(items: &[lsproto::CompletionItem]) -> Vec<CompletionItem> {
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let item_clone = item.clone();
        entries.push(CompletionItem {
            completion_item: item_clone,
            symbol: SymbolId::NIL,
        });
    }
    entries
}

// Go: ls/completions.go:3872 getKeywordCompletions
pub fn get_keyword_completions(
    keyword_filter: KeywordCompletionFilters,
    filter_out_ts_only_keywords: bool,
) -> Vec<CompletionItem> {
    if !filter_out_ts_only_keywords {
        return clone_items(&get_typescript_keyword_completions(keyword_filter));
    }

    let index = keyword_filter.0 + KeywordCompletionFilters::LAST.0 + 1;
    let cached = KEYWORD_COMPLETIONS_CACHE.with(|cache| cache.borrow().get(&index).cloned());
    if let Some(cached) = cached {
        return clone_items(&cached);
    }
    let result: Vec<lsproto::CompletionItem> = get_typescript_keyword_completions(keyword_filter)
        .into_iter()
        .filter(|ci| !is_type_script_only_keyword(string_to_token(&ci.label)))
        .collect();
    KEYWORD_COMPLETIONS_CACHE.with(|cache| {
        cache.borrow_mut().insert(index, result.clone());
    });
    clone_items(&result)
}

// Go: ls/completions.go:3891 getTypescriptKeywordCompletions
pub fn get_typescript_keyword_completions(
    keyword_filter: KeywordCompletionFilters,
) -> Vec<lsproto::CompletionItem> {
    let cached =
        KEYWORD_COMPLETIONS_CACHE.with(|cache| cache.borrow().get(&keyword_filter.0).cloned());
    if let Some(cached) = cached {
        return cached;
    }
    let result: Vec<lsproto::CompletionItem> = all_keyword_completions()
        .into_iter()
        .filter(|entry| {
            let kind = string_to_token(&entry.label);
            match keyword_filter {
                KeywordCompletionFilters::NONE => false,
                KeywordCompletionFilters::ALL => {
                    is_function_like_body_keyword(kind)
                        || kind == SyntaxKind::DeclareKeyword
                        || kind == SyntaxKind::ModuleKeyword
                        || kind == SyntaxKind::TypeKeyword
                        || kind == SyntaxKind::NamespaceKeyword
                        || kind == SyntaxKind::AbstractKeyword
                        || is_type_keyword(kind) && kind != SyntaxKind::UndefinedKeyword
                }
                KeywordCompletionFilters::FUNCTION_LIKE_BODY_KEYWORDS => {
                    is_function_like_body_keyword(kind)
                }
                KeywordCompletionFilters::CLASS_ELEMENT_KEYWORDS => {
                    is_class_member_completion_keyword(kind)
                }
                KeywordCompletionFilters::INTERFACE_ELEMENT_KEYWORDS => {
                    is_interface_or_type_literal_completion_keyword(kind)
                }
                KeywordCompletionFilters::CONSTRUCTOR_PARAMETER_KEYWORDS => {
                    is_parameter_property_modifier(kind)
                }
                KeywordCompletionFilters::TYPE_ASSERTION_KEYWORDS => {
                    is_type_keyword(kind) || kind == SyntaxKind::ConstKeyword
                }
                KeywordCompletionFilters::TYPE_KEYWORDS => is_type_keyword(kind),
                KeywordCompletionFilters::TYPE_KEYWORD => kind == SyntaxKind::TypeKeyword,
                _ => crate::core::go_panic(format!("Unknown keyword filter: {}", keyword_filter.0)),
            }
        })
        .collect();

    KEYWORD_COMPLETIONS_CACHE.with(|cache| {
        cache.borrow_mut().insert(keyword_filter.0, result.clone());
    });
    result
}

// Go: ls/completions.go:3931 isTypeScriptOnlyKeyword
pub fn is_type_script_only_keyword(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::AbstractKeyword
            | SyntaxKind::AnyKeyword
            | SyntaxKind::BigIntKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::EnumKeyword
            | SyntaxKind::GlobalKeyword
            | SyntaxKind::ImplementsKeyword
            | SyntaxKind::InferKeyword
            | SyntaxKind::InterfaceKeyword
            | SyntaxKind::IsKeyword
            | SyntaxKind::KeyOfKeyword
            | SyntaxKind::ModuleKeyword
            | SyntaxKind::NamespaceKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::ObjectKeyword
            | SyntaxKind::OverrideKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::PublicKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::TypeKeyword
            | SyntaxKind::UniqueKeyword
            | SyntaxKind::UnknownKeyword
    )
}

// Go: ls/completions.go:3966 isFunctionLikeBodyKeyword
pub fn is_function_like_body_keyword(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::AsyncKeyword
        || kind == SyntaxKind::AwaitKeyword
        || kind == SyntaxKind::UsingKeyword
        || kind == SyntaxKind::AsKeyword
        || kind == SyntaxKind::SatisfiesKeyword
        || kind == SyntaxKind::TypeKeyword
        || !is_contextual_keyword(kind) && !is_class_member_completion_keyword(kind)
}

// Go: ls/completions.go:3976 isClassMemberCompletionKeyword
pub fn is_class_member_completion_keyword(kind: SyntaxKind) -> bool {
    match kind {
        SyntaxKind::AbstractKeyword
        | SyntaxKind::AccessorKeyword
        | SyntaxKind::ConstructorKeyword
        | SyntaxKind::GetKeyword
        | SyntaxKind::SetKeyword
        | SyntaxKind::AsyncKeyword
        | SyntaxKind::DeclareKeyword
        | SyntaxKind::OverrideKeyword => true,
        _ => is_class_member_modifier(kind),
    }
}

// Go: ls/completions.go:3986 isInterfaceOrTypeLiteralCompletionKeyword
pub fn is_interface_or_type_literal_completion_keyword(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::ReadonlyKeyword
}

// Go: ls/completions.go:3990 isContextualKeywordInAutoImportableExpressionSpace
pub fn is_contextual_keyword_in_auto_importable_expression_space(keyword: &str) -> bool {
    keyword == "abstract"
        || keyword == "async"
        || keyword == "await"
        || keyword == "declare"
        || keyword == "module"
        || keyword == "namespace"
        || keyword == "type"
        || keyword == "satisfies"
        || keyword == "as"
}

// Go: ls/completions.go:4002 getContextualKeywords
pub fn get_contextual_keywords(
    file: Node,
    context_token: Node,
    position: i32,
) -> Vec<lsproto::CompletionItem> {
    let mut entries: Vec<lsproto::CompletionItem> = Vec::new();
    // An `AssertClause` can come after an import declaration:
    //  import * from "foo" |
    //  import "foo" |
    // or after a re-export declaration that has a module specifier:
    //  export { foo } from "foo" |
    // Source: https://tc39.es/proposal-import-assertions/
    if context_token.is_some() {
        let parent = context_token.parent();
        let token_line = get_ecma_line_of_position(file, context_token.end());
        let current_line = get_ecma_line_of_position(file, position);
        if (is_import_declaration(parent)
            || is_export_declaration(parent) && parent.module_specifier().is_some())
            && context_token == parent.module_specifier()
            && token_line == current_line
        {
            entries.push(lsproto::CompletionItem {
                label: token_to_string(SyntaxKind::AssertKeyword).to_string(),
                kind: Some(lsproto::CompletionItemKind::KEYWORD),
                sort_text: Some(SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string()),
                ..Default::default()
            });
        }
    }
    entries
}

impl LanguageService {
    // Go: ls/completions.go:4028 getJSCompletionEntries
    // PORT: Go ranges over the name table map in random order; the port
    // uses the map's own order. Clients sort by sort text and label.
    pub fn get_js_completion_entries(
        &self,
        ctx: &Context,
        file: Node,
        position: i32,
        unique_names: &mut FxHashSet<String>,
        sorted_entries: Vec<CompletionItem>,
    ) -> Vec<CompletionItem> {
        let mut sorted_entries = sorted_entries;
        let name_table = &*source_file_get_name_table(file);
        for (name, &pos) in name_table {
            // Skip identifiers produced only from the current location
            if pos == position {
                continue;
            }
            if !unique_names.contains(name) && is_identifier_text(name, LanguageVariant::STANDARD) {
                unique_names.insert(name.clone());
                sorted_entries.push(CompletionItem {
                    completion_item: lsproto::CompletionItem {
                        label: name.clone(),
                        kind: Some(lsproto::CompletionItemKind::TEXT),
                        sort_text: Some(SORT_TEXT_JAVASCRIPT_IDENTIFIERS.to_string()),
                        commit_characters: Some(Vec::new()),
                        ..Default::default()
                    },
                    symbol: SymbolId::NIL,
                });
            }
        }
        sorted_entries
    }

    // Go: ls/completions.go:4056 getOptionalReplacementSpan
    pub fn get_optional_replacement_span(
        &self,
        location: Node,
        file: Node,
    ) -> Option<lsproto::Range> {
        // StringLiteralLike locations are handled separately in stringCompletions.ts
        if location.is_some()
            && (location.kind() == SyntaxKind::Identifier
                || location.kind() == SyntaxKind::PrivateIdentifier)
        {
            let start = astnav::get_start_of_node(location, file, false /*includeJSDoc*/);
            let (lsp_range, fidelity) =
                self.create_lsp_range_from_bounds(start, location.end(), file);
            if fidelity.is_exact() {
                return Some(lsp_range);
            }
        }
        None
    }
}

// Go: ls/completions.go:4068 isMemberCompletionKind
pub fn is_member_completion_kind(kind: CompletionKind) -> bool {
    kind == CompletionKind::OBJECT_PROPERTY_DECLARATION
        || kind == CompletionKind::MEMBER_LIKE
        || kind == CompletionKind::PROPERTY_ACCESS
}

// Go: ls/completions.go:4074 tryGetFunctionLikeBodyCompletionContainer
pub fn try_get_function_like_body_completion_container(context_token: Node) -> Node {
    if context_token.is_nil() {
        return Node::NIL;
    }

    let mut prev = Node::NIL;
    find_ancestor_or_quit(context_token, |node: Node| -> FindAncestorResult {
        if is_class_like(node) {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }
        if is_function_like_declaration(node) && prev == node.body() {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        prev = node;
        FindAncestorResult::FIND_ANCESTOR_FALSE
    })
}

// ---------------------------------------------------------------------------
// Go `unicode` and `unicode/utf8` helpers used above (file-local).
// ---------------------------------------------------------------------------

/// Go `utf8.DecodeRuneInString(text[pos:end])` over a byte range of `text`.
// PORT: Go cuts the string at `end` first. A `&str` cannot be cut inside a
// character, so the decoder reads `text` and treats the bytes from `end` on
// as missing: an empty range is `(RuneError, 0)` and a rune cut at `end` is
// `(RuneError, 1)`, as in Go.
fn decode_rune_in_range(text: &str, pos: usize, end: usize) -> (i32, i32) {
    if pos >= end {
        return (RUNE_ERROR, 0);
    }
    let (r, size) = utf8_decode_rune_in_string(text, pos);
    if pos + size as usize > end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// A decoded Go rune as a `char` (`utf8.RuneError` is U+FFFD).
fn rune_to_char(r: i32) -> char {
    char::from_u32(r as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Go `unicode.IsSpace`.
// PORT: Go `unicode.IsSpace` is the Unicode White_Space property, the same
// set as Rust `char::is_whitespace` (U+0085 and U+00A0 are in both; U+180E is
// in neither).
fn unicode_is_space(r: i32) -> bool {
    char::from_u32(r as u32).is_some_and(char::is_whitespace)
}
