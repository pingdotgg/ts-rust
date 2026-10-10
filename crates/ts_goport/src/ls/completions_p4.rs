use crate::ls::prelude::*;

// Port of Go `ls/completions.go` lines 4879-6286: completion item resolve
// and details, import statement completion info, JSDoc tag and parameter
// completions, exhaustive switch case snippets and the snippet printer.

use crate::frontend::core_textchange::{TextChange, apply_bulk_edits};

impl LanguageService {
    // Go: completions.go:5488 ResolveCompletionItem
    // PORT: Go takes `*lsproto.CompletionItem` and returns it after changing
    // it; Rust takes and returns the item by value. `data` is Go's nil-able
    // `*lsproto.CompletionItemData`. `file_name` is Go's
    // `tspath.RootedFilePath`: the server's rooted, normalized form of
    // `data.file_name` (server.go:2178). The file is found by it, and the
    // error text keeps `data.file_name` as Go does.
    pub fn resolve_completion_item(
        &self,
        ctx: &Context,
        item: lsproto::CompletionItem,
        data: Option<lsproto::CompletionItemData>,
        file_name: &str,
    ) -> Result<lsproto::CompletionItem, GoError> {
        let Some(data) = data else {
            return Err(gostd::errors::new("completion item data is nil"));
        };

        let (program, file) = self.try_get_program_and_file(file_name);
        if file.is_nil() {
            return Err(gostd::errors::errorf(
                format!("file not found: {}", data.file_name),
                Vec::new(),
            ));
        }
        let file = source_file_for_supplemental_file_index(file, data.supplemental_file_index);
        if file.is_nil() {
            // PORT: Go reads `*data.SupplementalFileIndex` here. A nil index
            // returns the file, so the index is set when the file is nil.
            let index = data
                .supplemental_file_index
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            return Err(gostd::errors::errorf(
                format!("supplemental source file index not found: {index}"),
                Vec::new(),
            ));
        }

        let (checker, done) = ls_program::get_type_checker_for_file(program, ctx, file);
        let mut checker_ref = checker.borrow_mut();
        // PORT: `data.position` is a Go byte offset (see `ensure_item_data`).
        let position =
            crate::scanner_util::port_byte_offset(&source_file_text(file), data.position);
        let result = self.get_completion_item_details(
            ctx,
            program,
            &mut checker_ref,
            position,
            file,
            item,
            &data,
        );
        drop(checker_ref);
        done.call();
        Ok(result)
    }
}

// Go: completions.go:5512 getCompletionDocumentationFormat
pub fn get_completion_documentation_format(ctx: &Context) -> lsproto::MarkupKind {
    lsproto::preferred_markup_kind(
        &lsproto::get_client_capabilities(ctx)
            .text_document
            .completion
            .completion_item
            .documentation_format,
    )
}

impl LanguageService {
    // Go: completions.go:5516 getCompletionItemDetails
    #[allow(clippy::too_many_arguments)]
    pub fn get_completion_item_details(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        checker: &mut Checker,
        position: i32,
        file: Node,
        item: lsproto::CompletionItem,
        data: &lsproto::CompletionItemData,
    ) -> lsproto::CompletionItem {
        let mut item = item;
        let doc_format = get_completion_documentation_format(ctx);
        let (context_token, previous_token) = get_relevant_tokens(position, file);
        if is_in_string(file, position, previous_token) {
            return self.get_string_literal_completion_details(
                ctx,
                checker,
                item,
                &data.name,
                file,
                position,
                context_token,
                doc_format,
            );
        }

        if let Some(auto_import) = &data.auto_import {
            if data.is_import_statement_completion {
                return item;
            }
            // Auto-imports in content-mapped files are evaluated eagerly so edits outside
            // of verbatim spans can cause the completion item to be filtered out entirely.
            // Only real files take this code path, so the final Edits() is guaranteed ok.
            let (edits, description, _) = autoimport::Fix {
                auto_import_fix: auto_import.clone(),
                ..Default::default()
            }
            .edits(
                ctx,
                file,
                program.options(),
                &self.format_options(),
                &self.converters,
                &self.user_preferences(),
            );
            // Go `&edits`: a `[]*lsproto.TextEdit` of non-nil edits.
            item.additional_text_edits = Some(edits.into_iter().map(Some).collect());
            item.detail = str_ptr_to(&description);
            return item;
        }

        // Compute all the completion symbols again.
        let symbol_completion =
            self.get_symbol_completion_from_item_data(ctx, checker, file, position, data);
        let preferences = self.user_preferences();

        if let Some(request) = symbol_completion.request {
            match request {
                CompletionData::JSDocTagName(_) => {
                    return create_simple_details(item, &data.name, doc_format);
                }
                CompletionData::JSDocTag(_) => {
                    return create_simple_details(item, &data.name, doc_format);
                }
                CompletionData::JSDocParameterName(_) => {
                    return create_simple_details(item, &data.name, doc_format);
                }
                CompletionData::Keyword(request) => {
                    if request
                        .keyword_completions
                        .iter()
                        .any(|c| c.completion_item.label == data.name)
                    {
                        return create_simple_details(item, &data.name, doc_format);
                    }
                    return item;
                }
                // PORT: Go prints the dynamic type with `%T`; the only other
                // variant is `*completionDataData`.
                _ => crate::core::go_panic(
                    "Unexpected completion data type: *ls.completionDataData".to_string(),
                ),
            }
        } else if let Some(symbol_details) = symbol_completion.symbol {
            return self.create_completion_details_for_symbol(
                item,
                symbol_details.symbol,
                checker,
                symbol_details.location,
                position,
                doc_format,
            );
        } else if let Some(literal) = symbol_completion.literal {
            return create_simple_details(
                item,
                &completion_name_for_literal(file, &preferences, &literal),
                doc_format,
            );
        } else if symbol_completion.cases.is_some() {
            return item;
        } else {
            // Didn't find a symbol with this name.  See if we can find a keyword instead.
            if all_keyword_completions()
                .iter()
                .any(|c| c.label == data.name)
            {
                return create_simple_details(item, &data.name, doc_format);
            }
            return item;
        }
    }
}

// Go: completions.go:5609 detailsData
// PORT: Go `request *completionData` is `Option<CompletionData>`, and
// `cases *struct{}` is `Option<()>`.
#[derive(Default)]
pub struct DetailsData {
    pub symbol: Option<SymbolDetails>,
    pub request: Option<CompletionData>,
    pub literal: Option<LiteralValue>,
    pub cases: Option<()>,
}

// Go: completions.go:5616 symbolDetails
// PORT: Go shares the `*symbolOriginInfo` pointer; Rust clones the origin.
#[derive(Clone, Debug, Default)]
pub struct SymbolDetails {
    pub symbol: SymbolId,
    pub location: Node,
    pub origin: Option<SymbolOriginInfo>,
    pub previous_token: Node,
    pub context_token: Node,
    pub jsx_initializer: JsxInitializer,
    pub is_type_only_location: bool,
}

impl LanguageService {
    // Go: completions.go:5626 getSymbolCompletionFromItemData
    pub fn get_symbol_completion_from_item_data(
        &self,
        ctx: &Context,
        ch: &mut Checker,
        file: Node,
        position: i32,
        item_data: &lsproto::CompletionItemData,
    ) -> DetailsData {
        if item_data.source == SOURCE_SWITCH_CASES {
            return DetailsData {
                cases: Some(()),
                ..Default::default()
            };
        }

        let completion_data = match self.get_completion_data(
            ctx,
            ch,
            file,
            position,
            &self.user_preferences(),
            true, /*forItemResolve*/
        ) {
            Ok(completion_data) => completion_data,
            Err(err) => crate::core::go_panic(err.error()),
        };

        let Some(completion_data) = completion_data else {
            return DetailsData::default();
        };

        let data = match completion_data {
            CompletionData::Data(data) => data,
            completion_data => {
                return DetailsData {
                    request: Some(completion_data),
                    ..Default::default()
                };
            }
        };

        let preferences = self.user_preferences();
        let mut literal: Option<LiteralValue> = None;
        for l in &data.literals {
            if completion_name_for_literal(file, &preferences, l) == item_data.name {
                literal = Some(l.clone());
                break;
            }
        }
        if literal.is_some() {
            return DetailsData {
                literal,
                ..Default::default()
            };
        }

        // Find the symbol with the matching entry name.
        // We don't need to perform character checks here because we're only comparing the
        // name against 'entryName' (which is known to be good), not building a new
        // completion entry.
        for (index, &symbol) in data.symbols.iter().enumerate() {
            let origin = data.symbol_to_origin_info_map.get(&(index as i32));
            let (display_name, _) = get_completion_entry_display_name_for_symbol(
                file,
                &preferences,
                &ch.symbols,
                symbol,
                origin,
                data.completion_kind,
                data.is_jsx_identifier_expected,
            );
            if display_name == item_data.name
                && (item_data.source == COMPLETION_SOURCE_CLASS_MEMBER_SNIPPET
                    && ch.sym(symbol).flags.intersects(SymbolFlags::CLASS_MEMBER)
                    || item_data.source == COMPLETION_SOURCE_OBJECT_LITERAL_METHOD_SNIPPET
                        && ch
                            .sym(symbol)
                            .flags
                            .intersects(SymbolFlags::PROPERTY | SymbolFlags::METHOD)
                    || get_source_from_origin(origin) == item_data.source
                    || item_data.source == COMPLETION_SOURCE_OBJECT_LITERAL_MEMBER_WITH_COMMA)
            {
                return DetailsData {
                    symbol: Some(SymbolDetails {
                        symbol,
                        location: data.location,
                        origin: origin.cloned(),
                        previous_token: data.previous_token,
                        context_token: data.context_token,
                        jsx_initializer: data.jsx_initializer,
                        is_type_only_location: data.is_type_only_location,
                    }),
                    ..Default::default()
                };
            }
        }
        DetailsData::default()
    }
}

// Go: completions.go:5698 createSimpleDetails
pub fn create_simple_details(
    item: lsproto::CompletionItem,
    name: &str,
    doc_format: lsproto::MarkupKind,
) -> lsproto::CompletionItem {
    create_completion_details(item, name, "" /*documentation*/, doc_format)
}

// Go: completions.go:5706 createCompletionDetails
pub fn create_completion_details(
    item: lsproto::CompletionItem,
    detail: &str,
    documentation: &str,
    doc_format: lsproto::MarkupKind,
) -> lsproto::CompletionItem {
    let mut item = item;
    // !!! fill in additionalTextEdits from code actions
    if item.detail.is_none() && !detail.is_empty() {
        item.detail = Some(detail.to_string());
    }
    if !documentation.is_empty() {
        item.documentation = Some(lsproto::StringOrMarkupContent {
            string: None,
            markup_content: Some(lsproto::MarkupContent {
                kind: doc_format,
                value: documentation.to_string(),
            }),
        });
    }
    item
}

// Go: completions.go:5727 codeAction
// PORT: Go's unexported `codeAction` is unused. It stays private because the
// exported `CodeAction` (codeactions.go) has the same Rust name in package `ls`.
#[allow(dead_code)]
struct CodeAction {
    // Description of the code action to display in the UI of the editor
    description: String,
    // Text changes to apply to each file as part of the code action
    changes: Vec<lsproto::TextEdit>,
}

impl LanguageService {
    // Go: completions.go:5734 createCompletionDetailsForSymbol
    pub fn create_completion_details_for_symbol(
        &self,
        item: lsproto::CompletionItem,
        symbol: SymbolId,
        checker: &mut Checker,
        location: Node,
        _position: i32,
        doc_format: lsproto::MarkupKind,
    ) -> lsproto::CompletionItem {
        let (quick_info, documentation, _, _) = self.get_quick_info_and_documentation_for_symbol(
            checker,
            symbol,
            location,
            doc_format.clone(),
            None,
            false, /*vsCapability*/
        );
        create_completion_details(item, &quick_info, &documentation, doc_format)
    }

    // Go: completions.go:5746 getImportStatementCompletionInfo
    pub fn get_import_statement_completion_info(
        &self,
        context_token: Node,
        source_file: Node,
    ) -> ImportStatementCompletionInfo {
        let mut result = ImportStatementCompletionInfo {
            is_keyword_only_completion: false,
            keyword_completion: SyntaxKind::Unknown,
            is_new_identifier_location: false,
            is_top_level_type_only: false,
            could_be_type_only_import_specifier: false,
            replacement_span: None,
        };
        let mut candidate = Node::NIL;
        let parent = context_token.parent();
        if is_import_equals_declaration(parent) {
            // import Foo |
            // import Foo f|
            let last_token = lsutil::get_last_token(parent, source_file);
            if context_token.kind() == SyntaxKind::Identifier && last_token != context_token {
                result.keyword_completion = SyntaxKind::FromKeyword;
                result.is_keyword_only_completion = true;
            } else {
                if context_token.kind() != SyntaxKind::TypeKeyword {
                    result.keyword_completion = SyntaxKind::TypeKeyword;
                }
                if is_module_specifier_missing_or_empty(parent.module_reference()) {
                    candidate = parent;
                }
            }
        } else if could_be_type_only_import_specifier(parent, context_token)
            && can_complete_from_named_bindings(parent.parent())
        {
            candidate = parent;
        } else if is_named_imports(parent) || is_namespace_import(parent) {
            if !parent.parent().is_type_only()
                && (context_token.kind() == SyntaxKind::OpenBraceToken
                    || context_token.kind() == SyntaxKind::ImportKeyword
                    || context_token.kind() == SyntaxKind::CommaToken)
            {
                result.keyword_completion = SyntaxKind::TypeKeyword;
            }
            if can_complete_from_named_bindings(parent) {
                // At `import { ... } |` or `import * as Foo |`, the only possible completion is `from`
                if context_token.kind() == SyntaxKind::CloseBraceToken
                    || context_token.kind() == SyntaxKind::Identifier
                {
                    result.is_keyword_only_completion = true;
                    result.keyword_completion = SyntaxKind::FromKeyword;
                } else {
                    candidate = parent.parent().parent();
                }
            }
        } else if is_export_declaration(parent) && context_token.kind() == SyntaxKind::AsteriskToken
            || is_named_exports(parent) && context_token.kind() == SyntaxKind::CloseBraceToken
        {
            result.is_keyword_only_completion = true;
            result.keyword_completion = SyntaxKind::FromKeyword;
        } else if context_token.kind() == SyntaxKind::ImportKeyword {
            if is_source_file(parent) {
                // A lone import keyword with nothing following it does not parse as a statement at all
                result.keyword_completion = SyntaxKind::TypeKeyword;
                candidate = context_token;
            } else if is_import_declaration(parent) {
                // `import s| from`
                result.keyword_completion = SyntaxKind::TypeKeyword;
                if is_module_specifier_missing_or_empty(parent.module_specifier()) {
                    candidate = parent;
                }
            }
        }

        if candidate.is_some() {
            result.is_new_identifier_location = true;
            result.replacement_span =
                self.get_single_line_replacement_span_for_import_completion_node(candidate);
            result.could_be_type_only_import_specifier =
                could_be_type_only_import_specifier(candidate, context_token);
            if is_import_declaration(candidate) {
                let import_clause = candidate.import_clause();
                if import_clause.is_some() {
                    result.is_top_level_type_only = import_clause.is_type_only();
                }
            } else if candidate.kind() == SyntaxKind::ImportEqualsDeclaration {
                result.is_top_level_type_only = candidate.is_type_only();
            }
        } else {
            result.is_new_identifier_location =
                result.keyword_completion == SyntaxKind::TypeKeyword;
        }
        result
    }

    // Go: completions.go:5821 getSingleLineReplacementSpanForImportCompletionNode
    pub fn get_single_line_replacement_span_for_import_completion_node(
        &self,
        node: Node,
    ) -> Option<lsproto::Range> {
        let mut node = node;
        // node is ImportDeclaration | ImportEqualsDeclaration | ImportSpecifier | JSDocImportTag | Token<SyntaxKind.ImportKeyword>
        let ancestor = find_ancestor(node, |n| {
            is_import_declaration(n) || is_import_equals_declaration(n) || is_js_doc_import_tag(n)
        });
        if ancestor.is_some() {
            node = ancestor;
        }
        let source_file = get_source_file_of_node(node);
        // Use token position (excluding JSDoc/trivia) instead of node.Pos() to avoid including JSDoc comments
        let token_pos = get_token_pos_of_node(node, source_file, false /*includeJSDoc*/);
        if get_lines_between_positions(source_file, token_pos, node.end()) == 0 {
            let (lsp_range, fidelity) = self.create_lsp_range_from_node(node, source_file);
            if !fidelity.is_exact() {
                return None;
            }
            return Some(lsp_range);
        }

        if node.kind() == SyntaxKind::ImportKeyword || node.kind() == SyntaxKind::ImportSpecifier {
            crate::core::go_panic("ImportKeyword was necessarily on one line; ImportSpecifier was necessarily parented in an ImportDeclaration".to_string());
        }

        // Guess which point in the import might actually be a later statement parsed as part of the import
        // during parser recovery - either in the middle of named imports, or the module specifier.
        let potential_split_point: Node;
        if node.kind() == SyntaxKind::ImportDeclaration || node.kind() == SyntaxKind::JsDocImportTag
        {
            let mut specifier = Node::NIL;
            let import_clause = node.import_clause();
            if import_clause.is_some() {
                specifier =
                    get_potentially_invalid_import_specifier(import_clause.named_bindings());
            }

            if specifier.is_some() {
                potential_split_point = specifier;
            } else {
                potential_split_point = node.module_specifier();
            }
        } else {
            potential_split_point = node.module_reference();
        }

        let without_module_specifier = TextRange::new(
            get_token_pos_of_node(
                lsutil::get_first_token(node, source_file),
                source_file,
                false,
            ),
            potential_split_point.pos(),
        );
        // The module specifier/reference was previously found to be missing, empty, or
        // not a string literal - in this last case, it's likely that statement on a following
        // line was parsed as the module specifier of a partially-typed import, e.g.
        //   import Foo|
        //   interface Blah {}
        // This appears to be a multiline-import, and editors can't replace multiple lines.
        // But if everything but the "module specifier" is on one line, by this point we can
        // assume that the "module specifier" is actually just another statement, and return
        // the single-line range of the import excluding that probable statement.
        if get_lines_between_positions(
            source_file,
            without_module_specifier.pos(),
            without_module_specifier.end(),
        ) == 0
        {
            let (lsp_range, fidelity) = self.create_lsp_range_from_bounds(
                without_module_specifier.pos(),
                without_module_specifier.end(),
                source_file,
            );
            if !fidelity.is_exact() {
                return None;
            }
            return Some(lsp_range);
        }
        None
    }
}

// Go: completions.go:5879 couldBeTypeOnlyImportSpecifier
pub fn could_be_type_only_import_specifier(import_specifier: Node, context_token: Node) -> bool {
    is_import_specifier(import_specifier)
        && (import_specifier.is_type_only()
            || context_token == import_specifier.name()
                && is_type_keyword_token_or_identifier(context_token))
}

// Go: completions.go:5883 canCompleteFromNamedBindings
pub fn can_complete_from_named_bindings(named_bindings: Node) -> bool {
    if !is_module_specifier_missing_or_empty(named_bindings.parent().parent().module_specifier())
        || named_bindings.parent().name().is_some()
    {
        return false;
    }
    if is_named_imports(named_bindings) {
        // We can only complete on named imports if there are no other named imports already,
        // but parser recovery sometimes puts later statements in the named imports list, so
        // we try to only consider the probably-valid ones.
        let invalid_named_import = get_potentially_invalid_import_specifier(named_bindings);
        let elements = named_bindings.elements();
        let mut valid_imports = elements.len() as i32;
        if invalid_named_import.is_some() {
            // Go: slices.Index(elements, invalidNamedImport)
            valid_imports = elements
                .iter()
                .position(|e| e == invalid_named_import)
                .map_or(-1, |i| i as i32);
        }

        return valid_imports < 2 && valid_imports > -1;
    }
    true
}

// Go: completions.go:5911 getPotentiallyInvalidImportSpecifier
// Tries to identify the first named import that is not really a named import, but rather
// just parser recovery for a situation like:
//
//	import { Foo|
//	interface Bar {}
//
// in which `Foo`, `interface`, and `Bar` are all parsed as import specifiers. The caller
// will also check if this token is on a separate line from the rest of the import.
pub fn get_potentially_invalid_import_specifier(named_bindings: Node) -> Node {
    if named_bindings.is_nil() || named_bindings.kind() != SyntaxKind::NamedImports {
        return Node::NIL;
    }
    named_bindings
        .elements()
        .iter()
        .find(|&e| {
            e.property_name().is_nil()
                && lsutil::is_non_contextual_keyword(string_to_token(e.name().text()))
                && astnav::find_preceding_token(
                    get_source_file_of_node(named_bindings),
                    e.name().pos(),
                )
                .kind()
                    != SyntaxKind::CommaToken
        })
        .unwrap_or(Node::NIL)
}

// Go: completions.go:5921 isModuleSpecifierMissingOrEmpty
pub fn is_module_specifier_missing_or_empty(specifier: Node) -> bool {
    if node_is_missing(specifier) {
        return true;
    }
    let mut node = specifier;
    if is_external_module_reference(node) {
        node = node.expression();
    }
    if !is_string_literal_like(node) {
        return true;
    }
    node.text().is_empty()
}

// Go: completions.go:5935 hasDocComment
pub fn has_doc_comment(file: Node, position: i32) -> bool {
    let token = astnav::get_token_at_position(file, position);
    find_ancestor(token, is_js_doc).is_some()
}

// Go: completions.go:5941 getJSDocTagAtPosition
// Get the corresponding JSDocTag node if the position is in a JSDoc comment
pub fn get_js_doc_tag_at_position(node: Node, position: i32) -> Node {
    find_ancestor_or_quit(node, |n| {
        if is_js_doc_tag(n) && n.loc().contains_inclusive(position) {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        if is_js_doc(n) {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }
        FindAncestorResult::FIND_ANCESTOR_FALSE
    })
}

// Go: completions.go:5953 tryGetTypeExpressionFromTag
pub fn try_get_type_expression_from_tag(tag: Node) -> Node {
    if is_tag_with_type_expression(tag) {
        let type_expression = if is_js_doc_template_tag(tag) {
            tag.constraint()
        } else {
            tag.type_expression()
        };
        if type_expression.is_some() && type_expression.kind() == SyntaxKind::JsDocTypeExpression {
            return type_expression;
        }
    }
    if is_js_doc_augments_tag(tag) || is_js_doc_implements_tag(tag) {
        return tag.class_name();
    }
    Node::NIL
}

// Go: completions.go:5971 isTagWithTypeExpression
pub fn is_tag_with_type_expression(tag: Node) -> bool {
    match tag.kind() {
        SyntaxKind::JsDocParameterTag
        | SyntaxKind::JsDocPropertyTag
        | SyntaxKind::JsDocReturnTag
        | SyntaxKind::JsDocTypeTag
        | SyntaxKind::JsDocTypedefTag
        | SyntaxKind::JsDocThrowsTag
        | SyntaxKind::JsDocSatisfiesTag => true,
        SyntaxKind::JsDocTemplateTag => tag.constraint().is_some(),
        _ => false,
    }
}

impl LanguageService {
    // Go: completions.go:5983 jsDocCompletionInfo
    pub fn js_doc_completion_info(
        &self,
        ctx: &Context,
        position: i32,
        file: Node,
        items: Vec<CompletionItem>,
    ) -> Option<CompletionList> {
        let mut items = items;
        let default_commit_characters =
            get_default_commit_characters(false /*isNewIdentifierLocation*/);
        let item_defaults = self.set_item_defaults(
            ctx,
            position,
            file,
            &mut items,
            Some(&default_commit_characters),
            None, /*optionalReplacementSpan*/
        );
        Some(CompletionList {
            is_incomplete: false,
            item_defaults,
            items,
            ..Default::default()
        })
    }
}

// Go: completions.go:6005 jsDocTagNames
pub static JS_DOC_TAG_NAMES: &[&str] = &[
    "abstract",
    "access",
    "alias",
    "argument",
    "async",
    "augments",
    "author",
    "borrows",
    "callback",
    "class",
    "classdesc",
    "constant",
    "constructor",
    "constructs",
    "copyright",
    "default",
    "deprecated",
    "description",
    "emits",
    "enum",
    "event",
    "example",
    "exports",
    "extends",
    "external",
    "field",
    "file",
    "fileoverview",
    "fires",
    "function",
    "generator",
    "global",
    "hideconstructor",
    "host",
    "ignore",
    "implements",
    "import",
    "inheritdoc",
    "inner",
    "instance",
    "interface",
    "kind",
    "lends",
    "license",
    "link",
    "linkcode",
    "linkplain",
    "listens",
    "member",
    "memberof",
    "method",
    "mixes",
    "module",
    "name",
    "namespace",
    "overload",
    "override",
    "package",
    "param",
    "private",
    "prop",
    "property",
    "protected",
    "public",
    "readonly",
    "requires",
    "returns",
    "satisfies",
    "see",
    "since",
    "static",
    "summary",
    "template",
    "this",
    "throws",
    "todo",
    "tutorial",
    "type",
    "typedef",
    "var",
    "variation",
    "version",
    "virtual",
    "yields",
];

// PORT: Go `sync.OnceValue` package vars become lazily built thread-local
// values (all language-service work runs on the dispatch thread).
thread_local! {
    // Go: completions.go:6092 jsDocTagNameCompletionItems
    static JS_DOC_TAG_NAME_COMPLETION_ITEMS: Vec<lsproto::CompletionItem> = {
        let mut items: Vec<lsproto::CompletionItem> = Vec::with_capacity(JS_DOC_TAG_NAMES.len());
        for tag_name in JS_DOC_TAG_NAMES {
            let item = lsproto::CompletionItem {
                label: tag_name.to_string(),
                kind: Some(lsproto::CompletionItemKind::KEYWORD),
                sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
                ..Default::default()
            };
            items.push(item);
        }
        items
    };

    // Go: completions.go:6105 jsDocTagCompletionItems
    static JS_DOC_TAG_COMPLETION_ITEMS: Vec<lsproto::CompletionItem> = {
        let mut items: Vec<lsproto::CompletionItem> = Vec::with_capacity(JS_DOC_TAG_NAMES.len());
        for tag_name in JS_DOC_TAG_NAMES {
            let item = lsproto::CompletionItem {
                label: format!("@{tag_name}"),
                kind: Some(lsproto::CompletionItemKind::KEYWORD),
                sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
                ..Default::default()
            };
            items.push(item);
        }
        items
    };
}

// Go: completions.go:6118 getJSDocTagNameCompletions
pub fn get_js_doc_tag_name_completions() -> Vec<CompletionItem> {
    JS_DOC_TAG_NAME_COMPLETION_ITEMS.with(|items| clone_items(items))
}

// Go: completions.go:6122 getJSDocTagCompletions
pub fn get_js_doc_tag_completions() -> Vec<CompletionItem> {
    JS_DOC_TAG_COMPLETION_ITEMS.with(|items| clone_items(items))
}

// Go: completions.go:6126 getJSDocParameterCompletions
pub fn get_js_doc_parameter_completions(
    _ctx: &Context,
    file: Node,
    position: i32,
    type_checker: &mut Checker,
    options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
    tag_name_only: bool,
) -> Vec<CompletionItem> {
    let current_token = astnav::get_token_at_position(file, position);
    if !is_js_doc_tag(current_token) && !is_js_doc(current_token) {
        return Vec::new();
    }
    let js_doc = if is_js_doc(current_token) {
        current_token
    } else {
        current_token.parent()
    };
    if !is_js_doc(js_doc) {
        return Vec::new();
    }
    let fun = js_doc.parent();
    if !is_function_like(fun) {
        return Vec::new();
    }

    let is_js = is_source_file_js(file);
    // isSnippet := clientSupportsItemSnippet(clientOptions)
    let is_snippet = false; // !!! need snippet printer
    let mut param_tag_count = 0;
    let mut tags: Vec<Node> = Vec::new();
    if !js_doc.tags().is_nil() {
        tags = js_doc.tags().nodes().to_vec();
    }
    for &tag in &tags {
        if is_js_doc_parameter_tag(tag)
            && astnav::get_start_of_node(tag, file, false /*includeJSDoc*/) < position
            && is_identifier(tag.name())
        {
            param_tag_count += 1;
        }
    }
    let mut param_index = -1;
    // ts#64649: one emit context for all the annotations, made on first use.
    let mut emit_context: Option<Rc<EmitContext>> = None;
    // Go: core.MapNonNil(fun.Parameters(), func(param) *CompletionItem { ... })
    let mut result: Vec<CompletionItem> = Vec::new();
    for param in fun.parameters().iter() {
        param_index += 1;
        if param_index < param_tag_count {
            // This parameter is already annotated.
            continue;
        }
        if is_identifier(param.name()) {
            // Named parameter
            let mut tabstop_counter = 1;
            let param_name = param.name().text();
            let mut display_text = get_js_doc_param_annotation(
                &mut emit_context,
                param_name,
                param.initializer(),
                param.dot_dot_dot_token(),
                is_js,
                false, /*isObject*/
                false, /*isSnippet*/
                type_checker,
                options,
                preferences,
                &mut tabstop_counter,
            );
            let mut snippet_text = String::new();
            if is_snippet {
                snippet_text = get_js_doc_param_annotation(
                    &mut emit_context,
                    param_name,
                    param.initializer(),
                    param.dot_dot_dot_token(),
                    is_js,
                    false, /*isObject*/
                    true,  /*isSnippet*/
                    type_checker,
                    options,
                    preferences,
                    &mut tabstop_counter,
                );
            }
            if tag_name_only {
                // Remove `@`
                display_text = display_text[1..].to_string();
                if !snippet_text.is_empty() {
                    snippet_text = snippet_text[1..].to_string();
                }
            }

            result.push(CompletionItem {
                completion_item: lsproto::CompletionItem {
                    label: display_text,
                    kind: Some(lsproto::CompletionItemKind::VARIABLE),
                    sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
                    insert_text: str_ptr_to(&snippet_text),
                    insert_text_format: if is_snippet {
                        Some(lsproto::InsertTextFormat::SNIPPET)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                ..Default::default()
            });
        } else if param_index == param_tag_count {
            // Destructuring parameter; do it positionally
            let param_path = format!("param{param_index}");
            let display_text_result = generate_js_doc_param_tags_for_destructuring(
                &mut emit_context,
                &param_path,
                param.name(),
                param.initializer(),
                param.dot_dot_dot_token(),
                is_js,
                false, /*isSnippet*/
                type_checker,
                options,
                preferences,
            );
            let mut snippet_text = String::new();
            if is_snippet {
                let snippet_text_result = generate_js_doc_param_tags_for_destructuring(
                    &mut emit_context,
                    &param_path,
                    param.name(),
                    param.initializer(),
                    param.dot_dot_dot_token(),
                    is_js,
                    true, /*isSnippet*/
                    type_checker,
                    options,
                    preferences,
                );
                snippet_text = snippet_text_result
                    .join((options.new_line.get_new_line_character().to_string() + "* ").as_str());
            }
            let mut display_text = display_text_result
                .join((options.new_line.get_new_line_character().to_string() + "* ").as_str());
            if tag_name_only {
                // Remove `@`
                display_text = display_text
                    .strip_prefix('@')
                    .unwrap_or(&display_text)
                    .to_string();
                snippet_text = snippet_text
                    .strip_prefix('@')
                    .unwrap_or(&snippet_text)
                    .to_string();
            }
            result.push(CompletionItem {
                completion_item: lsproto::CompletionItem {
                    label: display_text,
                    kind: Some(lsproto::CompletionItemKind::VARIABLE),
                    sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
                    insert_text: str_ptr_to(&snippet_text),
                    insert_text_format: if is_snippet {
                        Some(lsproto::InsertTextFormat::SNIPPET)
                    } else {
                        None
                    },
                    ..Default::default()
                },
                ..Default::default()
            });
        }
    }
    result
}

// Go: completions.go:6274 getJSDocParamAnnotation
// PORT: Go `tabstopCounter *int` is never nil at a call site, so it is
// `&mut i32`; the Go `debug.Assert(tabstopCounter != nil)` always holds.
#[allow(clippy::too_many_arguments)]
pub fn get_js_doc_param_annotation(
    emit_context: &mut Option<Rc<EmitContext>>,
    param_name: &str,
    initializer: Node,
    dot_dot_dot_token: Node,
    is_js: bool,
    is_object: bool,
    is_snippet: bool,
    type_checker: &mut Checker,
    _options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
    tabstop_counter: &mut i32,
) -> String {
    let mut param_name = param_name.to_string();
    if initializer.is_some() {
        param_name = get_js_doc_param_name_with_initializer(&param_name, initializer);
    }
    if is_snippet {
        param_name = escape_snippet_text(&param_name);
    }
    if is_js {
        let mut t = "*".to_string();
        if is_object {
            debug_assert!(
                dot_dot_dot_token.is_nil(),
                "Cannot annotate a rest parameter with type 'object'."
            );
            t = "object".to_string();
        } else {
            if initializer.is_some() {
                let inferred_type = type_checker.get_type_at_location(initializer.parent());
                if !type_checker
                    .ty(inferred_type)
                    .flags
                    .intersects(TypeFlags::ANY | TypeFlags::VOID)
                {
                    let file = get_source_file_of_node(initializer);
                    let quote_preference = lsutil::get_quote_preference(file, preferences);
                    let builder_flags = if quote_preference == lsutil::QuotePreference::SINGLE {
                        NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
                    } else {
                        NodeBuilderFlags::NONE
                    };
                    let type_node = type_checker.type_to_type_node_exported(
                        inferred_type,
                        find_ancestor(initializer, is_function_like),
                        builder_flags,
                        None, /*idToSymbol*/
                    );
                    if type_node.is_some() {
                        // ts#64649: the caller's emit context, made on first use.
                        let emit_context =
                            emit_context.get_or_insert_with(new_emit_context).clone();
                        // !!! snippet p
                        let mut p = new_printer(
                            PrinterOptions {
                                remove_comments: true,
                                // !!!
                                // Module: options.Module,
                                // ModuleResolution: options.ModuleResolution,
                                // Target: options.Target,
                                ..Default::default()
                            },
                            PrintHandlers::default(),
                            Some(Rc::clone(&emit_context)),
                        );
                        emit_context.set_emit_flags(type_node, EmitFlags::SINGLE_LINE);
                        t = p.emit(type_node, file);
                    }
                }
            }
            if is_snippet && t == "*" {
                let tabstop = *tabstop_counter;
                *tabstop_counter += 1;
                t = format!("${{{tabstop}:{t}}}");
            }
        }
        let dot_dot_dot = if !is_object && dot_dot_dot_token.is_some() {
            "..."
        } else {
            ""
        };
        let mut description = String::new();
        if is_snippet {
            let tabstop = *tabstop_counter;
            *tabstop_counter += 1;
            description = format!("${{{tabstop}}}");
        }
        format!("@param {{{dot_dot_dot}{t}}} {param_name} {description}")
    } else {
        let mut description = String::new();
        if is_snippet {
            let tabstop = *tabstop_counter;
            *tabstop_counter += 1;
            description = format!("${{{tabstop}}}");
        }
        format!("@param {param_name} {description}")
    }
}

// Go: completions.go:6360 getJSDocParamNameWithInitializer
// PORT: Go `strings.TrimSpace` and Rust `str::trim` both trim Unicode
// White_Space.
pub fn get_js_doc_param_name_with_initializer(param_name: &str, initializer: Node) -> String {
    let initializer_text = get_text_of_node(initializer).trim().to_string();
    if initializer_text.contains('\n') || initializer_text.len() > 80 {
        return format!("[{param_name}]");
    }
    format!("[{param_name}={initializer_text}]")
}

// Go: completions.go:6368 generateJSDocParamTagsForDestructuring
#[allow(clippy::too_many_arguments)]
pub fn generate_js_doc_param_tags_for_destructuring(
    emit_context: &mut Option<Rc<EmitContext>>,
    path: &str,
    pattern: Node,
    initializer: Node,
    dot_dot_dot_token: Node,
    is_js: bool,
    is_snippet: bool,
    type_checker: &mut Checker,
    options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
) -> Vec<String> {
    let mut tabstop_counter = 1;
    if !is_js {
        return vec![get_js_doc_param_annotation(
            emit_context,
            path,
            initializer,
            dot_dot_dot_token,
            is_js,
            false, /*isObject*/
            is_snippet,
            type_checker,
            options,
            preferences,
            &mut tabstop_counter,
        )];
    }
    js_doc_param_pattern_worker(
        emit_context,
        path,
        pattern,
        initializer,
        dot_dot_dot_token,
        is_js,
        is_snippet,
        type_checker,
        options,
        preferences,
        &mut tabstop_counter,
    )
}

// Go: completions.go:6411 jsDocParamPatternWorker
#[allow(clippy::too_many_arguments)]
pub fn js_doc_param_pattern_worker(
    emit_context: &mut Option<Rc<EmitContext>>,
    path: &str,
    pattern: Node,
    initializer: Node,
    dot_dot_dot_token: Node,
    is_js: bool,
    is_snippet: bool,
    type_checker: &mut Checker,
    options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
    counter: &mut i32,
) -> Vec<String> {
    if is_object_binding_pattern(pattern) && dot_dot_dot_token.is_nil() {
        let mut child_counter = *counter;
        let root_param = get_js_doc_param_annotation(
            emit_context,
            path,
            initializer,
            dot_dot_dot_token,
            is_js,
            true, /*isObject*/
            is_snippet,
            type_checker,
            options,
            preferences,
            &mut child_counter,
        );
        let mut child_tags: Vec<String> = Vec::new();
        for element in pattern.elements().iter() {
            let element_tags = js_doc_param_element_worker(
                emit_context,
                path,
                element,
                initializer,
                dot_dot_dot_token,
                is_js,
                is_snippet,
                type_checker,
                options,
                preferences,
                &mut child_counter,
            );
            if element_tags.is_empty() {
                child_tags = Vec::new();
                break;
            }
            child_tags.extend(element_tags);
        }
        if !child_tags.is_empty() {
            *counter = child_counter;
            let mut result = vec![root_param];
            result.extend(child_tags);
            return result;
        }
    }
    vec![get_js_doc_param_annotation(
        emit_context,
        path,
        initializer,
        dot_dot_dot_token,
        is_js,
        false, /*isObject*/
        is_snippet,
        type_checker,
        options,
        preferences,
        counter,
    )]
}

// Go: completions.go:6484 jsDocParamElementWorker
// Assumes binding element is inside object binding pattern.
// We can't deeply annotate an array binding pattern.
#[allow(clippy::too_many_arguments)]
pub fn js_doc_param_element_worker(
    emit_context: &mut Option<Rc<EmitContext>>,
    path: &str,
    element: Node,
    _initializer: Node,
    _dot_dot_dot_token: Node,
    is_js: bool,
    is_snippet: bool,
    type_checker: &mut Checker,
    options: &CompilerOptions,
    preferences: &lsutil::UserPreferences,
    counter: &mut i32,
) -> Vec<String> {
    if is_identifier(element.name()) {
        // `{ b }` or `{ b: newB }`
        let property_name = if element.property_name().is_some() {
            try_get_text_of_property_name(element.property_name()).0
        } else {
            element.name().text().to_string()
        };
        if property_name.is_empty() {
            return Vec::new();
        }
        let param_name = format!("{path}.{property_name}");
        return vec![get_js_doc_param_annotation(
            emit_context,
            &param_name,
            element.initializer(),
            element.dot_dot_dot_token(),
            is_js,
            false, /*isObject*/
            is_snippet,
            type_checker,
            options,
            preferences,
            counter,
        )];
    } else if element.property_name().is_some() {
        // `{ b: {...} }` or `{ b: [...] }`
        let (property_name, _) = try_get_text_of_property_name(element.property_name());
        if property_name.is_empty() {
            return Vec::new();
        }
        return js_doc_param_pattern_worker(
            emit_context,
            &format!("{path}.{property_name}"),
            element.name(),
            element.initializer(),
            element.dot_dot_dot_token(),
            is_js,
            is_snippet,
            type_checker,
            options,
            preferences,
            counter,
        );
    }
    Vec::new()
}

// Go: completions.go:6545 getJSDocParameterNameCompletions
pub fn get_js_doc_parameter_name_completions(tag: Node) -> Vec<CompletionItem> {
    if !is_identifier(tag.name()) {
        return Vec::new();
    }
    let name_thus_far = tag.name().text();
    let js_doc = tag.parent();
    let fn_ = js_doc.parent();
    if !is_function_like(fn_) {
        return Vec::new();
    }

    let mut tags: Vec<Node> = Vec::new();
    if !js_doc.tags().is_nil() {
        tags = js_doc.tags().nodes().to_vec();
    }

    // Go: core.MapNonNil(fn.Parameters(), func(param) *CompletionItem { ... })
    let mut result: Vec<CompletionItem> = Vec::new();
    for param in fn_.parameters().iter() {
        if !is_identifier(param.name()) {
            continue;
        }

        let name = param.name().text();
        if tags.iter().any(|&t| {
            t != tag
                && is_js_doc_parameter_tag(t)
                && is_identifier(t.name())
                && t.name().text() == name
        }) || !name_thus_far.is_empty() && !name.starts_with(name_thus_far)
        {
            continue;
        }

        result.push(CompletionItem {
            completion_item: lsproto::CompletionItem {
                label: name.to_string(),
                kind: Some(lsproto::CompletionItemKind::VARIABLE),
                sort_text: Some(SORT_TEXT_LOCATION_PRIORITY.to_string()),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    result
}

impl LanguageService {
    // Go: completions.go:6586 getExhaustiveCaseSnippets
    #[allow(clippy::too_many_arguments)]
    pub fn get_exhaustive_case_snippets(
        &self,
        ctx: &Context,
        case_block: Node,
        file: Node,
        position: i32,
        options: &CompilerOptions,
        program: &compiler::NewProgram,
        c: &mut Checker,
    ) -> Result<Option<lsproto::CompletionItem>, GoError> {
        let clauses = case_block.clauses().nodes().to_vec();
        let switch_type = c.get_type_at_location(case_block.parent().expression());
        if switch_type.is_some()
            && c.ty(switch_type).is_union()
            && c.ty(switch_type)
                .types()
                .iter()
                .all(|&t| is_literal(&*c, t))
        {
            // Collect constant values in existing clauses.
            let mut tracker = new_case_clause_tracker(c, &clauses);
            let target = options.get_emit_script_target();
            let quote_preference = lsutil::get_quote_preference(file, &self.user_preferences());
            // Tolerate a nil import adder in untitled files.
            let mut import_adder: Option<Box<dyn autoimport::ImportAdder>> = None;
            if !tspath::is_dynamic_file_name(source_file_file_name(file)) {
                let view = self.get_prepared_auto_import_view(file)?;
                if let Some(view) = view {
                    // PORT: Go passes the checker `c`; the Rust adder does not
                    // store it (map-ls-completions.md 2.5).
                    import_adder = Some(autoimport::new_import_adder(
                        ctx,
                        program,
                        file,
                        view,
                        self.format_options(),
                        self.converters.clone(),
                        self.user_preferences(),
                    ));
                }
            }

            let mut elements: Vec<Node> = Vec::new();
            let factory = NodeFactory::new_with_hooks(NodeFactoryHooks::default());
            let switch_types = c.ty(switch_type).types().to_vec();
            for t in switch_types {
                // Enums
                if c.ty(t).is_enum_literal() {
                    debug_assert!(
                        c.ty(t).symbol().is_some(),
                        "An enum member type should have a symbol"
                    );
                    debug_assert!(
                        c.sym(c.ty(t).symbol()).parent.is_some(),
                        "An enum member type should have a parent symbol (the enum symbol)"
                    );
                    // Filter existing enums by their values
                    let mut enum_value: Option<LiteralValue> = None;
                    let value_declaration = c.sym(c.ty(t).symbol()).value_declaration;
                    if value_declaration.is_some() {
                        enum_value = c.get_constant_value(value_declaration);
                    }
                    if let Some(enum_value) = &enum_value {
                        if tracker.has_value(enum_value) {
                            continue;
                        }
                        tracker.add_value(enum_value);
                    }
                    // PORT: Go passes the nil-able interface; the boxed adder is
                    // lent as `&mut dyn ImportAdder` (explicit coercion).
                    let import_adder_ref: Option<&mut dyn autoimport::ImportAdder> =
                        match import_adder.as_mut() {
                            Some(adder) => {
                                let adder: &mut dyn autoimport::ImportAdder = &mut **adder;
                                Some(adder)
                            }
                            None => None,
                        };
                    let type_node = autoimport::type_to_auto_importable_type_node(
                        c,
                        import_adder_ref,
                        t,
                        case_block,
                    );
                    if type_node.is_nil() {
                        return Ok(None);
                    }
                    let expr =
                        type_node_to_expression(type_node, target, quote_preference, &factory);
                    if expr.is_nil() {
                        return Ok(None);
                    }
                    elements.push(expr);
                } else {
                    // Literals
                    // PORT: Go passes a nil value to `hasValue`, which panics
                    // with "Unsupported type"; a non-enum literal always has one.
                    let value = c
                        .ty(t)
                        .as_literal_type()
                        .value()
                        .cloned()
                        .unwrap_or_else(|| {
                            crate::core::go_panic("Unsupported type: <nil>".to_string())
                        });
                    if !tracker.has_value(&value) {
                        match value {
                            LiteralValue::PseudoBigInt(mut v) => {
                                let big_int: Node;
                                if v.negative {
                                    v.negative = false;
                                    big_int = factory.new_prefix_unary_expression(
                                        SyntaxKind::MinusToken,
                                        factory
                                            .new_big_int_literal(format!("{v}n"), TokenFlags::NONE),
                                    );
                                } else {
                                    big_int = factory
                                        .new_big_int_literal(format!("{v}n"), TokenFlags::NONE);
                                }
                                elements.push(big_int);
                            }
                            LiteralValue::Number(v) => {
                                let number: Node;
                                if v.0 < 0.0 {
                                    number = factory.new_prefix_unary_expression(
                                        SyntaxKind::MinusToken,
                                        factory.new_numeric_literal(
                                            v.abs().to_string(),
                                            TokenFlags::NONE,
                                        ),
                                    );
                                } else {
                                    number = factory
                                        .new_numeric_literal(v.to_string(), TokenFlags::NONE);
                                }
                                elements.push(number);
                            }
                            LiteralValue::String(v) => {
                                let literal = factory.new_string_literal(
                                    v,
                                    if quote_preference == lsutil::QuotePreference::SINGLE {
                                        TokenFlags::SINGLE_QUOTE
                                    } else {
                                        TokenFlags::NONE
                                    },
                                );
                                elements.push(literal);
                            }
                            _ => {}
                        }
                    }
                }
            }
            if elements.is_empty() {
                return Ok(None);
            }

            let new_clauses: Vec<Node> = elements
                .iter()
                .map(|&element| {
                    factory.new_case_or_default_clause(
                        SyntaxKind::CaseClause,
                        element,
                        factory.new_node_list(&[]),
                    )
                })
                .collect();
            let new_line_char = self.format_options().editor_settings.new_line_character;
            let mut printer = create_snippet_printer(
                PrinterOptions {
                    remove_comments: true,
                    new_line: get_new_line_kind(&new_line_char),
                    ..Default::default()
                },
                None, /*emitContext*/
            );
            // Go: core.MapIndex(newClauses, func(clause, i) string { ... })
            let mut clause_texts: Vec<String> = Vec::with_capacity(new_clauses.len());
            for (i, &clause) in new_clauses.iter().enumerate() {
                if client_supports_item_snippet(ctx) {
                    // Go: printNode(clause)
                    let printed = printer.print_and_format_node(ctx, clause, file);
                    clause_texts.push(format!("{}${}", printed, i + 1));
                } else {
                    clause_texts.push(printer.print_unescaped_node(clause));
                }
            }
            let insert_text = clause_texts.join(new_line_char.as_str());

            let first_clause = printer.print_unescaped_node(new_clauses[0]);
            let name = first_clause + " ...";

            let mut additional_text_edits: Option<Vec<Option<lsproto::TextEdit>>> = None;
            if let Some(import_adder) = import_adder.as_mut() {
                let edits = import_adder.edits();
                if !edits.is_empty() {
                    additional_text_edits = Some(edits.into_iter().map(Some).collect());
                }
            }

            return Ok(Some(lsproto::CompletionItem {
                label: name.clone(),
                kind: Some(lsproto::CompletionItemKind::SNIPPET),
                sort_text: Some(SORT_TEXT_GLOBALS_OR_KEYWORDS.to_string()),
                insert_text: str_ptr_to(&insert_text),
                additional_text_edits,
                insert_text_format: if client_supports_item_snippet(ctx) {
                    Some(lsproto::InsertTextFormat::SNIPPET)
                } else {
                    None
                },
                data: Some(lsproto::CompletionItemData {
                    file_name: source_file_original_file_name(file).to_string(),
                    position,
                    supplemental_file_index: supplemental_file_index(file),
                    name,
                    source: COMPLETION_SOURCE_SWITCH_CASES.to_string(),
                    auto_import: None,
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }
        Ok(None)
    }
}

// Go: completions.go:6724 typeNodeToExpression
pub fn type_node_to_expression(
    type_node: Node,
    target: ScriptTarget,
    quote_preference: lsutil::QuotePreference,
    factory: &NodeFactory,
) -> Node {
    match type_node.kind() {
        SyntaxKind::TypeReference => {
            let type_name = type_node.type_name();
            entity_name_to_expression(type_name, target, quote_preference, factory)
        }
        SyntaxKind::IndexedAccessType => {
            let object_expression =
                type_node_to_expression(type_node.object_type(), target, quote_preference, factory);
            let index_expression =
                type_node_to_expression(type_node.index_type(), target, quote_preference, factory);
            if object_expression.is_some() && index_expression.is_some() {
                return factory.new_element_access_expression(
                    object_expression,
                    Node::NIL, /*questionDotToken*/
                    index_expression,
                    NodeFlags::NONE,
                );
            }
            Node::NIL
        }
        SyntaxKind::LiteralType => {
            let literal = type_node.literal();
            match literal.kind() {
                SyntaxKind::StringLiteral => factory.new_string_literal(
                    literal.text(),
                    if quote_preference == lsutil::QuotePreference::SINGLE {
                        TokenFlags::SINGLE_QUOTE
                    } else {
                        TokenFlags::NONE
                    },
                ),
                SyntaxKind::NumericLiteral => {
                    factory.new_numeric_literal(literal.text(), literal.token_flags())
                }
                _ => Node::NIL,
            }
        }
        SyntaxKind::ParenthesizedType => {
            let expr =
                type_node_to_expression(type_node.type_(), target, quote_preference, factory);
            if expr.is_nil() {
                return Node::NIL;
            }
            if is_identifier(expr) {
                return expr;
            }
            factory.new_parenthesized_expression(expr)
        }
        SyntaxKind::TypeQuery => {
            entity_name_to_expression(type_node.expr_name(), target, quote_preference, factory)
        }
        SyntaxKind::ImportType => {
            crate::gostd::debug::fail(
                "We should not get an import type after calling 'typeToAutoImportableTypeNode'.",
            );
        }
        _ => Node::NIL,
    }
}

// Go: completions.go:6786 entityNameToExpression
pub fn entity_name_to_expression(
    entity_name: Node,
    target: ScriptTarget,
    quote_preference: lsutil::QuotePreference,
    factory: &NodeFactory,
) -> Node {
    if is_identifier(entity_name) {
        return entity_name;
    }
    factory.new_property_access_expression(
        entity_name_to_expression(entity_name.left(), target, quote_preference, factory),
        Node::NIL, /*questionDotToken*/
        entity_name.right(),
        NodeFlags::NONE,
    )
}

// Go: completions.go:6803 snippetPrinter
// PORT: Go keeps `baseWriter` and the writer that embeds it as two pointers
// to one `ChangeTrackerWriter`. In Rust the snippet writer owns it, so Go
// `p.baseWriter` is `p.writer.borrow().change_tracker_writer`. The printer
// writes through `Rc<RefCell<dyn EmitTextWriter>>`, so the writer is shared.
// Go `factory` is `emitContext.Factory.AsNodeFactory()`, a pointer into the
// context; here it is the method `factory()`.
pub struct SnippetPrinter {
    pub emit_context: Rc<EmitContext>,
    pub printer: Printer,
    pub writer: Rc<RefCell<SnippetEmitTextWriter>>,
}

impl SnippetPrinter {
    /// Go `p.factory` (`emitContext.Factory.AsNodeFactory()`).
    #[must_use]
    pub fn factory(&self) -> &NodeFactory {
        self.emit_context.factory().as_node_factory()
    }

    // Go: completions.go:6794 printNode
    /** Snippet-escaping version of `printer.printNode`. */
    pub fn print_node(&mut self, node: Node) -> String {
        let unescaped = self.print_unescaped_node(node);
        let writer = self.writer.borrow();
        if !writer.escapes.is_empty() {
            return apply_bulk_edits(&unescaped, &writer.escapes);
        }
        unescaped
    }

    // Go: completions.go:6820 printUnescapedNode
    pub fn print_unescaped_node(&mut self, node: Node) -> String {
        {
            let mut writer = self.writer.borrow_mut();
            writer.escapes = Vec::new();
            writer.clear();
        }
        let writer: Rc<RefCell<dyn EmitTextWriter>> = self.writer.clone();
        self.printer.write_exported(
            node,
            Node::NIL, /*sourceFile*/
            writer,
            None, /*sourceMapGenerator*/
        );
        self.writer.borrow().string()
    }

    // Go: completions.go:6827 printAndFormatNode
    pub fn print_and_format_node(
        &mut self,
        ctx: &Context,
        node: Node,
        source_file: Node,
    ) -> String {
        let format_options = crate::format::get_format_code_settings_from_context(ctx);
        self.print_and_format_node_with_settings(ctx, node, source_file, &format_options)
    }

    // Go: completions.go:6831 printAndFormatNodeWithSettings
    pub fn print_and_format_node_with_settings(
        &mut self,
        ctx: &Context,
        node: Node,
        source_file: Node,
        format_options: &lsutil::FormatCodeSettings,
    ) -> String {
        let text = self.print_unescaped_node(node);
        let node_with_pos = self
            .writer
            .borrow()
            .change_tracker_writer
            .assign_positions_to_node(node, self.factory());
        // PORT: Go passes `sourceFile.ParseOptions()`; the Rust function
        // takes the file name and path (see its PORT note).
        let synthetic_file = create_synthetic_source_file(
            self.factory(),
            node_with_pos,
            &text,
            source_file_file_name(source_file),
            &source_file_info(source_file).path,
        );
        let ctx = crate::format::with_format_code_settings(
            ctx,
            format_options,
            &format_options.editor_settings.new_line_character,
        );
        let changes = crate::format::format_node_given_indentation(
            &ctx,
            node_with_pos,
            synthetic_file,
            source_file_info(source_file).language_variant,
            0, /*initialIndentation*/
            0, /*delta*/
        );

        let mut all_changes = changes;
        let writer = self.writer.borrow();
        if !writer.escapes.is_empty() {
            all_changes.extend(writer.escapes.iter().cloned());
            gostd::slices::sort_func(&mut all_changes, |a: &TextChange, b: &TextChange| {
                compare_text_ranges(a.text_range, b.text_range)
            });
        }

        apply_bulk_edits(&source_file_text(synthetic_file), &all_changes)
    }
}

// Go: completions.go:6854 createSnippetPrinter
// PORT: a nil Go `emitContext` is `None`.
pub fn create_snippet_printer(
    options: PrinterOptions,
    emit_context: Option<Rc<EmitContext>>,
) -> SnippetPrinter {
    let emit_context = match emit_context {
        Some(emit_context) => emit_context,
        None => new_emit_context(),
    };
    let base_writer = new_change_tracker_writer(options.new_line.get_new_line_character(), -1);
    let printer = new_printer(
        options,
        base_writer.get_print_handlers(),
        Some(Rc::clone(&emit_context)),
    );
    let writer = Rc::new(RefCell::new(SnippetEmitTextWriter {
        change_tracker_writer: base_writer,
        escapes: Vec::new(),
    }));
    SnippetPrinter {
        emit_context,
        printer,
        writer,
    }
}

// Go: completions.go:6873 snippetEmitTextWriter
// Override base writer methods to perform snippet escaping.
// PORT: Go embeds `*printer.ChangeTrackerWriter`; here it is the owned field
// `change_tracker_writer`. The `EmitTextWriter` impl below forwards every
// method that Go does not override.
pub struct SnippetEmitTextWriter {
    pub change_tracker_writer: ChangeTrackerWriter,
    pub escapes: Vec<TextChange>,
}

impl SnippetEmitTextWriter {
    // Go: completions.go:6906 escapingWrite
    // The formatter/scanner will have issues with snippet-escaped text,
    // so instead of writing the escaped text directly to the writer,
    // generate a set of changes that can be applied to the unescaped text
    // to escape it post-formatting.
    pub fn escaping_write(&mut self, s: &str, write: impl FnOnce(&mut ChangeTrackerWriter)) {
        let escaped = escape_snippet_text(s);
        if escaped != s {
            let start = self.get_text_pos();
            write(&mut self.change_tracker_writer);
            let end = self.get_text_pos();
            self.escapes.push(TextChange {
                new_text: escaped,
                text_range: TextRange::new(start, end),
            });
        } else {
            write(&mut self.change_tracker_writer);
        }
    }
}

impl EmitTextWriter for SnippetEmitTextWriter {
    // Go: completions.go:6878 Write
    fn write(&mut self, s: &str) {
        self.escaping_write(s, |w| w.write(s));
    }

    fn write_trailing_semicolon(&mut self, text: &str) {
        self.change_tracker_writer.write_trailing_semicolon(text);
    }

    // Go: completions.go:6882 WriteComment
    fn write_comment(&mut self, text: &str) {
        self.escaping_write(text, |w| w.write_comment(text));
    }

    fn write_keyword(&mut self, text: &str) {
        self.change_tracker_writer.write_keyword(text);
    }

    fn write_operator(&mut self, text: &str) {
        self.change_tracker_writer.write_operator(text);
    }

    fn write_punctuation(&mut self, text: &str) {
        self.change_tracker_writer.write_punctuation(text);
    }

    fn write_space(&mut self, text: &str) {
        self.change_tracker_writer.write_space(text);
    }

    // Go: completions.go:6886 WriteStringLiteral
    fn write_string_literal(&mut self, text: &str) {
        self.escaping_write(text, |w| w.write_string_literal(text));
    }

    // Go: completions.go:6890 WriteParameter
    fn write_parameter(&mut self, text: &str) {
        self.escaping_write(text, |w| w.write_parameter(text));
    }

    // Go: completions.go:6894 WriteProperty
    fn write_property(&mut self, text: &str) {
        self.escaping_write(text, |w| w.write_property(text));
    }

    // Go: completions.go:6898 WriteSymbol
    fn write_symbol(&mut self, text: &str, symbol: SymbolId) {
        self.escaping_write(text, |w| w.write_symbol(text, symbol));
    }

    fn write_line(&mut self) {
        self.change_tracker_writer.write_line();
    }

    fn write_line_force(&mut self, force: bool) {
        self.change_tracker_writer.write_line_force(force);
    }

    fn increase_indent(&mut self) {
        self.change_tracker_writer.increase_indent();
    }

    fn decrease_indent(&mut self) {
        self.change_tracker_writer.decrease_indent();
    }

    fn clear(&mut self) {
        self.change_tracker_writer.clear();
    }

    fn string(&self) -> String {
        self.change_tracker_writer.string()
    }

    fn raw_write(&mut self, s: &str) {
        self.change_tracker_writer.raw_write(s);
    }

    fn write_literal(&mut self, s: &str) {
        self.change_tracker_writer.write_literal(s);
    }

    fn get_text_pos(&self) -> i32 {
        self.change_tracker_writer.get_text_pos()
    }

    fn get_line(&self) -> i32 {
        self.change_tracker_writer.get_line()
    }

    fn get_column(&self) -> i32 {
        self.change_tracker_writer.get_column()
    }

    fn get_indent(&self) -> i32 {
        self.change_tracker_writer.get_indent()
    }

    fn is_at_start_of_line(&self) -> bool {
        self.change_tracker_writer.is_at_start_of_line()
    }

    fn has_trailing_comment(&self) -> bool {
        self.change_tracker_writer.has_trailing_comment()
    }

    fn has_trailing_whitespace(&self) -> bool {
        self.change_tracker_writer.has_trailing_whitespace()
    }
}
