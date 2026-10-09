//! Port of Go `ls/documenthighlights.go`.
//!
//! PORT: Go `[]*lsproto.DocumentHighlight` and `[]*ast.Node` results are
//! `Vec`s; a Go nil slice is an empty `Vec` (every caller only tests the
//! length). Go `*ast.SourceFile` is the file root `Node`.

use crate::ls::prelude::*;

use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/documenthighlights.go:21 ProvideDocumentHighlights
    pub fn provide_document_highlights(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        document_position: lsproto::Position,
    ) -> Result<lsproto::DocumentHighlightResponse, GoError> {
        let result =
            self.provide_document_highlights_worker(ctx, document_uri, document_position, &[])?;
        // Extract highlights for the current file only.
        let mut document_highlights: Vec<lsproto::DocumentHighlight> = Vec::new();
        if let Some(multi_document_highlights) = &result.multi_document_highlights {
            for mh in multi_document_highlights {
                if mh.uri == *document_uri {
                    document_highlights.extend(mh.highlights.iter().copied());
                }
            }
        }
        Ok(lsproto::DocumentHighlightsOrNull {
            document_highlights: Some(document_highlights),
        })
    }

    // Go: ls/documenthighlights.go:38 ProvideMultiDocumentHighlights
    pub fn provide_multi_document_highlights(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        document_position: lsproto::Position,
        files_to_search: &[lsproto::DocumentUri],
    ) -> Result<lsproto::CustomMultiDocumentHighlightResponse, GoError> {
        self.provide_document_highlights_worker(
            ctx,
            document_uri,
            document_position,
            files_to_search,
        )
    }

    // Go: ls/documenthighlights.go:42 provideDocumentHighlightsWorker
    pub fn provide_document_highlights_worker(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        document_position: lsproto::Position,
        files_to_search: &[lsproto::DocumentUri],
    ) -> Result<lsproto::MultiDocumentHighlightsOrNull, GoError> {
        let (program, source_file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            source_file,
            document_position,
            Feature::DOCUMENT_HIGHLIGHTS,
        );
        let mut results: Vec<lsproto::MultiDocumentHighlightsOrNull> =
            Vec::with_capacity(positions.len());
        for mapped in positions {
            if mapped.fidelity.is_single_segment() {
                results.push(self.provide_document_highlights_at_position(
                    ctx,
                    document_uri,
                    mapped.position,
                    program,
                    mapped.script,
                    files_to_search,
                ));
            }
        }
        Ok(combine_multi_document_highlights(results))
    }

    // Go: ls/documenthighlights.go:54 provideDocumentHighlightsAtPosition
    pub fn provide_document_highlights_at_position(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: i32,
        program: &compiler::NewProgram,
        source_file: Node,
        files_to_search: &[lsproto::DocumentUri],
    ) -> lsproto::MultiDocumentHighlightsOrNull {
        let node = astnav::get_touching_property_name(source_file, position);

        // Cheap JSX check before resolving files to search.
        if node.parent().is_some()
            && (node.parent().kind() == SyntaxKind::JsxClosingElement
                || (node.parent().kind() == SyntaxKind::JsxOpeningElement
                    && node.parent().tag_name() == node))
        {
            let mut opening_element = Node::NIL;
            let mut closing_element = Node::NIL;
            if is_jsx_element(node.parent().parent()) {
                opening_element = node.parent().parent().opening_element();
                closing_element = node.parent().parent().closing_element();
            }
            let mut highlights: Vec<lsproto::DocumentHighlight> = Vec::new();
            let kind = lsproto::DocumentHighlightKind::READ;
            if opening_element.is_some() {
                let (lsp_range, fidelity) = self.create_lsp_range_from_node_for_feature(
                    opening_element,
                    source_file,
                    Feature::DOCUMENT_HIGHLIGHTS,
                );
                if !fidelity.is_none() {
                    highlights.push(lsproto::DocumentHighlight {
                        range: lsp_range,
                        kind: Some(kind),
                    });
                }
            }
            if closing_element.is_some() {
                let (lsp_range, fidelity) = self.create_lsp_range_from_node_for_feature(
                    closing_element,
                    source_file,
                    Feature::DOCUMENT_HIGHLIGHTS,
                );
                if !fidelity.is_none() {
                    highlights.push(lsproto::DocumentHighlight {
                        range: lsp_range,
                        kind: Some(kind),
                    });
                }
            }
            let multi_highlights = vec![lsproto::MultiDocumentHighlight {
                uri: document_uri.clone(),
                highlights,
            }];
            return lsproto::MultiDocumentHighlightsOrNull {
                multi_document_highlights: Some(multi_highlights),
            };
        }

        // Resolve the source files to search, deduplicating by file name.
        let mut source_files: Vec<Node> = Vec::new();
        let mut seen_files: FxHashSet<String> =
            FxHashSet::with_capacity_and_hasher(files_to_search.len(), Default::default());
        for uri in files_to_search {
            let file_name = uri.file_name();
            if !seen_files.insert(file_name.clone()) {
                continue;
            }
            // PORT: `NewProgram::get_source_file` returns the parsed file; the
            // Go `*ast.SourceFile` is its root node.
            if let Some(sf) = program.get_source_file(&file_name) {
                source_files.push(sf.root);
            }
        }
        if source_files.is_empty() {
            source_files = vec![source_file];
        }

        let mut multi_highlights =
            self.get_semantic_document_highlights(ctx, position, node, program, &source_files);
        if multi_highlights.is_empty() {
            // Fall back to syntactic highlights for the current file only.
            let syntactic_highlights = self.get_syntactic_document_highlights(node, source_file);
            if !syntactic_highlights.is_empty() {
                multi_highlights = vec![lsproto::MultiDocumentHighlight {
                    uri: document_uri.clone(),
                    highlights: syntactic_highlights,
                }];
            }
        }
        lsproto::MultiDocumentHighlightsOrNull {
            multi_document_highlights: Some(multi_highlights),
        }
    }
}

// Go: ls/documenthighlights.go:113 combineMultiDocumentHighlights
// PORT: Go keeps pointers to the combined documents in `byURI` and appends
// through them; here `by_uri` holds the index into `combined_documents`.
// Go map order is not used for output.
pub fn combine_multi_document_highlights(
    results: Vec<lsproto::MultiDocumentHighlightsOrNull>,
) -> lsproto::MultiDocumentHighlightsOrNull {
    let mut by_uri: FxHashMap<lsproto::DocumentUri, usize> = FxHashMap::default();
    let mut seen: FxHashMap<lsproto::DocumentUri, FxHashSet<lsproto::Range>> = FxHashMap::default();
    let mut combined_documents: Vec<lsproto::MultiDocumentHighlight> = Vec::new();
    for result in results {
        let Some(documents) = result.multi_document_highlights else {
            continue;
        };
        for document in documents {
            let index = match by_uri.get(&document.uri) {
                Some(&index) => index,
                None => {
                    by_uri.insert(document.uri.clone(), combined_documents.len());
                    combined_documents.push(lsproto::MultiDocumentHighlight {
                        uri: document.uri.clone(),
                        ..Default::default()
                    });
                    combined_documents.len() - 1
                }
            };
            let ranges = seen.entry(document.uri.clone()).or_default();
            for highlight in document.highlights {
                if ranges.insert(highlight.range) {
                    combined_documents[index].highlights.push(highlight);
                }
            }
        }
    }
    lsproto::MultiDocumentHighlightsOrNull {
        multi_document_highlights: Some(combined_documents),
    }
}

impl LanguageService {
    // Go: ls/documenthighlights.go:140 getSemanticDocumentHighlights
    pub fn get_semantic_document_highlights(
        &self,
        ctx: &Context,
        position: i32,
        node: Node,
        program: &compiler::NewProgram,
        source_files: &[Node],
    ) -> Vec<lsproto::MultiDocumentHighlight> {
        let options = RefOptions {
            use_: ReferenceUse::NONE,
            ..Default::default()
        };
        let reference_entries = self.get_referenced_symbols_for_node(
            ctx,
            position,
            node,
            program,
            source_files,
            options,
        );
        if reference_entries.is_empty() {
            return Vec::new();
        }

        // Group highlights by file
        // PORT: Go map used for lookups only; the output order follows
        // `source_files`.
        let mut file_highlights: FxHashMap<String, Vec<lsproto::DocumentHighlight>> =
            FxHashMap::default();
        for entry in &reference_entries {
            let references = entry.borrow().references.clone();
            for ref_ in &references {
                let (file_name, highlight) = self.to_document_highlight(ref_);
                let Some(highlight) = highlight else {
                    continue;
                };
                file_highlights
                    .entry(file_name)
                    .or_default()
                    .push(highlight);
            }
        }

        let mut result: Vec<lsproto::MultiDocumentHighlight> = Vec::new();
        for &sf in source_files {
            let file_name = source_file_original_file_name(sf);
            if let Some(highlights) = file_highlights.get(file_name) {
                result.push(lsproto::MultiDocumentHighlight {
                    uri: lsconv::file_name_to_document_uri(file_name),
                    highlights: highlights.clone(),
                });
            }
        }
        result
    }

    // Go: ls/documenthighlights.go:172 toDocumentHighlight
    // PORT: Go returns `*lsproto.DocumentHighlight`; nil is `None`.
    pub fn to_document_highlight(
        &self,
        entry: &Rc<RefCell<ReferenceEntry>>,
    ) -> (String, Option<lsproto::DocumentHighlight>) {
        let entry = self.resolve_entry(entry);
        let file_name = source_file_original_file_name(entry.borrow().source_file).to_string();

        let mut kind = lsproto::DocumentHighlightKind::READ;
        let (lsp_range, ok) =
            self.get_range_of_entry_for_feature(&entry, Feature::DOCUMENT_HIGHLIGHTS);
        if !ok {
            return (file_name, None);
        }
        // PORT: copy the fields out; the entry is borrowed only here.
        let (entry_kind, entry_node) = {
            let e = entry.borrow();
            (e.kind, e.node)
        };
        if entry_kind == EntryKind::RANGE {
            return (
                file_name,
                Some(lsproto::DocumentHighlight {
                    range: lsp_range,
                    kind: Some(kind),
                }),
            );
        }

        // Determine write access for node references.
        if is_write_access_for_reference(entry_node) {
            kind = lsproto::DocumentHighlightKind::WRITE;
        }

        let dh = lsproto::DocumentHighlight {
            range: lsp_range,
            kind: Some(kind),
        };

        (file_name, Some(dh))
    }

    // Go: ls/documenthighlights.go:201 getSyntacticDocumentHighlights
    pub fn get_syntactic_document_highlights(
        &self,
        node: Node,
        source_file: Node,
    ) -> Vec<lsproto::DocumentHighlight> {
        match node.kind() {
            SyntaxKind::IfKeyword | SyntaxKind::ElseKeyword => {
                if is_if_statement(node.parent()) {
                    return self.get_if_else_occurrences(node.parent(), source_file);
                }
                Vec::new()
            }
            SyntaxKind::ReturnKeyword => self.use_parent(
                node.parent(),
                &is_return_statement,
                &get_return_occurrences,
                source_file,
            ),
            SyntaxKind::ThrowKeyword => self.use_parent(
                node.parent(),
                &is_throw_statement,
                &get_throw_occurrences,
                source_file,
            ),
            SyntaxKind::TryKeyword | SyntaxKind::CatchKeyword | SyntaxKind::FinallyKeyword => {
                let try_statement = if node.kind() == SyntaxKind::CatchKeyword {
                    node.parent().parent()
                } else {
                    node.parent()
                };
                self.use_parent(
                    try_statement,
                    &is_try_statement,
                    &get_try_catch_finally_occurrences,
                    source_file,
                )
            }
            SyntaxKind::SwitchKeyword => self.use_parent(
                node.parent(),
                &is_switch_statement,
                &get_switch_case_default_occurrences,
                source_file,
            ),
            SyntaxKind::CaseKeyword | SyntaxKind::DefaultKeyword => {
                if is_default_clause(node.parent()) || is_case_clause(node.parent()) {
                    return self.use_parent(
                        node.parent().parent().parent(),
                        &is_switch_statement,
                        &get_switch_case_default_occurrences,
                        source_file,
                    );
                }
                Vec::new()
            }
            SyntaxKind::BreakKeyword | SyntaxKind::ContinueKeyword => self.use_parent(
                node.parent(),
                &is_break_or_continue_statement,
                &get_break_or_continue_statement_occurrences,
                source_file,
            ),
            SyntaxKind::ForKeyword | SyntaxKind::WhileKeyword | SyntaxKind::DoKeyword => self
                .use_parent(
                    node.parent(),
                    &|n: Node| is_iteration_statement(n, true),
                    &get_loop_break_continue_occurrences,
                    source_file,
                ),
            SyntaxKind::ConstructorKeyword => self.get_from_all_declarations(
                &is_constructor_declaration,
                &[SyntaxKind::ConstructorKeyword],
                node,
                source_file,
            ),
            SyntaxKind::GetKeyword | SyntaxKind::SetKeyword => self.get_from_all_declarations(
                &is_accessor,
                &[SyntaxKind::GetKeyword, SyntaxKind::SetKeyword],
                node,
                source_file,
            ),
            SyntaxKind::AwaitKeyword => self.use_parent(
                node.parent(),
                &is_await_expression,
                &get_async_and_await_occurrences,
                source_file,
            ),
            SyntaxKind::AsyncKeyword => self.highlight_spans(
                &get_async_and_await_occurrences(node, source_file),
                source_file,
            ),
            SyntaxKind::YieldKeyword => {
                self.highlight_spans(&get_yield_occurrences(node, source_file), source_file)
            }
            SyntaxKind::InKeyword | SyntaxKind::OutKeyword => Vec::new(),
            _ => {
                if is_modifier_kind(node.kind())
                    && (is_declaration(node.parent()) || is_variable_statement(node.parent()))
                {
                    return self.highlight_spans(
                        &get_modifier_occurrences(node.kind(), node.parent(), source_file),
                        source_file,
                    );
                }
                Vec::new()
            }
        }
    }

    // Go: ls/documenthighlights.go:253 useParent
    pub fn use_parent(
        &self,
        node: Node,
        node_test: &dyn Fn(Node) -> bool,
        get_nodes: &dyn Fn(Node, Node) -> Vec<Node>,
        source_file: Node,
    ) -> Vec<lsproto::DocumentHighlight> {
        if node_test(node) {
            return self.highlight_spans(&get_nodes(node, source_file), source_file);
        }
        Vec::new()
    }

    // Go: ls/documenthighlights.go:260 highlightSpans
    pub fn highlight_spans(
        &self,
        nodes: &[Node],
        source_file: Node,
    ) -> Vec<lsproto::DocumentHighlight> {
        if nodes.is_empty() {
            return Vec::new();
        }
        let mut highlights: Vec<lsproto::DocumentHighlight> = Vec::new();
        let kind = lsproto::DocumentHighlightKind::READ;
        for &node in nodes {
            if node.is_some() {
                let (lsp_range, fidelity) = self.create_lsp_range_from_node_for_feature(
                    node,
                    source_file,
                    Feature::DOCUMENT_HIGHLIGHTS,
                );
                if !fidelity.is_none() {
                    highlights.push(lsproto::DocumentHighlight {
                        range: lsp_range,
                        kind: Some(kind),
                    });
                }
            }
        }
        highlights
    }

    // Go: ls/documenthighlights.go:276 getFromAllDeclarations
    pub fn get_from_all_declarations(
        &self,
        node_test: &dyn Fn(Node) -> bool,
        keywords: &[SyntaxKind],
        node: Node,
        source_file: Node,
    ) -> Vec<lsproto::DocumentHighlight> {
        self.use_parent(
            node.parent(),
            node_test,
            &|decl: Node, _sf: Node| -> Vec<Node> {
                let mut symbol_decls: Vec<Node> = Vec::new();
                if can_have_symbol(decl) {
                    let symbol = decl.symbol();
                    if symbol.is_some() {
                        // PORT: Go reads the binder symbol through its pointer,
                        // without a checker. The binder symbols of the
                        // current program are `program::bound_symbols`.
                        let symbols = crate::program::bound_symbols();
                        let declarations: Vec<Node> = symbols.sym(symbol).declarations.to_vec();
                        for d in declarations {
                            if node_test(d) {
                                'outer: for c in get_children_from_non_js_doc_node(d, source_file) {
                                    for &k in keywords {
                                        if c.kind() == k {
                                            symbol_decls.push(c);
                                            break 'outer;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                symbol_decls
            },
            source_file,
        )
    }

    // Go: ls/documenthighlights.go:300 getIfElseOccurrences
    // PORT: Go takes `*ast.IfStatement`; here the IfStatement node.
    pub fn get_if_else_occurrences(
        &self,
        if_statement: Node,
        source_file: Node,
    ) -> Vec<lsproto::DocumentHighlight> {
        let keywords = get_if_else_keywords(if_statement, source_file);
        let kind = lsproto::DocumentHighlightKind::READ;
        let mut highlights: Vec<lsproto::DocumentHighlight> = Vec::new();

        // We'd like to highlight else/ifs together if they are only separated by whitespace
        // (i.e. the keywords are separated by no comments, no newlines).
        let mut i: usize = 0;
        while i < keywords.len() {
            if keywords[i].kind() == SyntaxKind::ElseKeyword && i < keywords.len() - 1 {
                let else_keyword = keywords[i];
                let if_keyword = keywords[i + 1]; // this *should* always be an 'if' keyword.
                let mut should_combine = true;

                // Avoid recalculating getStart() by iterating backwards.
                let mut if_token_start = get_token_pos_of_node(if_keyword, source_file, false);
                if if_token_start < 0 {
                    if_token_start = if_keyword.pos();
                }
                // PORT: Go `rune(sourceFile.Text()[j])` converts one byte.
                let text_text = source_file_text(source_file);
                let text = text_text.as_bytes();
                let mut j = if_token_start - 1;
                while j >= else_keyword.end() {
                    if !is_white_space_single_line(text[j as usize] as char) {
                        should_combine = false;
                        break;
                    }
                    j -= 1;
                }
                if should_combine {
                    let (lsp_range, fidelity) = self.create_lsp_range_from_bounds(
                        skip_trivia(&source_file_text(source_file), else_keyword.pos()),
                        if_keyword.end(),
                        source_file,
                    );
                    if !fidelity.is_none() {
                        highlights.push(lsproto::DocumentHighlight {
                            range: lsp_range,
                            kind: Some(kind),
                        });
                    }
                    i += 1; // skip the next keyword
                    i += 1;
                    continue;
                }
            }
            // Ordinary case: just highlight the keyword.
            let (lsp_range, fidelity) = self.create_lsp_range_from_node_for_feature(
                keywords[i],
                source_file,
                Feature::DOCUMENT_HIGHLIGHTS,
            );
            if !fidelity.is_none() {
                highlights.push(lsproto::DocumentHighlight {
                    range: lsp_range,
                    kind: Some(kind),
                });
            }
            i += 1;
        }
        highlights
    }
}

// Go: ls/documenthighlights.go:341 getIfElseKeywords
// PORT: Go takes `*ast.IfStatement`; here the IfStatement node.
pub fn get_if_else_keywords(if_statement: Node, source_file: Node) -> Vec<Node> {
    let mut if_statement = if_statement;
    // We may be at an if statement like those in the range below:
    //
    //   ```
    //   if (...) {
    //   } else [|if (...) {}|]
    //   ````
    //
    // Traverse upwards through all parent if-statements linked by their else-branches.
    while is_if_statement(if_statement.parent()) {
        // See if the parent's `else` is actually the current `if` statement.
        let parenting_if = if_statement.parent();
        let else_statement = parenting_if.else_statement();
        if else_statement != if_statement {
            break;
        }
        if_statement = parenting_if;
    }

    let mut keywords: Vec<Node> = Vec::new();

    // Traverse back down through the else branches, aggregating if/else keywords of if-statements.
    loop {
        let children = get_children_from_non_js_doc_node(if_statement, source_file);
        if !children.is_empty() && children[0].kind() == SyntaxKind::IfKeyword {
            keywords.push(children[0]);
        }
        // Generally the 'else' keyword is second-to-last, so traverse backwards.
        for i in (0..children.len()).rev() {
            if children[i].kind() == SyntaxKind::ElseKeyword {
                keywords.push(children[i]);
                break;
            }
        }
        let else_statement = if_statement.else_statement();
        if else_statement.is_nil() || !is_if_statement(else_statement) {
            break;
        }
        if_statement = else_statement;
    }
    keywords
}

// Go: ls/documenthighlights.go:384 getReturnOccurrences
pub fn get_return_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let func_node = find_ancestor(node.parent(), is_function_like);
    if func_node.is_nil() {
        return Vec::new();
    }

    let mut keywords: Vec<Node> = Vec::new();
    let body = func_node.body();
    if body.is_some() {
        for_each_return_statement(body, |ret| {
            let keyword = astnav::find_child_of_kind(ret, SyntaxKind::ReturnKeyword, source_file);
            if keyword.is_some() {
                keywords.push(keyword);
            }
            false // continue traversal
        });

        // Get all throw statements not in a try block
        let throw_statements = aggregate_owned_throw_statements(body, source_file);
        for throw in throw_statements {
            let keyword = astnav::find_child_of_kind(throw, SyntaxKind::ThrowKeyword, source_file);
            if keyword.is_some() {
                keywords.push(keyword);
            }
        }
    }
    keywords
}

// Go: ls/documenthighlights.go:413 aggregateOwnedThrowStatements
pub fn aggregate_owned_throw_statements(node: Node, source_file: Node) -> Vec<Node> {
    if is_throw_statement(node) {
        return vec![node];
    }
    if is_try_statement(node) {
        // Exceptions thrown within a try block lacking a catch clause are "owned" in the current context.
        let statement = node;
        let try_block = statement.try_block();
        let catch_clause = statement.catch_clause();
        let finally_block = statement.finally_block();

        let mut result: Vec<Node> = Vec::new();
        if catch_clause.is_some() {
            result = aggregate_owned_throw_statements(catch_clause, source_file);
        } else if try_block.is_some() {
            result = aggregate_owned_throw_statements(try_block, source_file);
        }
        if finally_block.is_some() {
            result.extend(aggregate_owned_throw_statements(finally_block, source_file));
        }
        return result;
    }
    // Do not cross function boundaries.
    if is_function_like(node) {
        return Vec::new();
    }
    flat_map_children(node, source_file, &aggregate_owned_throw_statements)
}

// Go: ls/documenthighlights.go:442 flatMapChildren
pub fn flat_map_children<T>(
    node: Node,
    source_file: Node,
    cb: &dyn Fn(Node, Node) -> Vec<T>,
) -> Vec<T> {
    let mut result: Vec<T> = Vec::new();

    node.for_each_child(|child| {
        let value = cb(child, source_file);
        result.extend(value);
        false // continue traversal
    });
    result
}

// Go: ls/documenthighlights.go:455 getThrowOccurrences
pub fn get_throw_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let owner = get_throw_statement_owner(node);
    if owner.is_nil() {
        return Vec::new();
    }

    let mut keywords: Vec<Node> = Vec::new();

    // Aggregate all throw statements "owned" by this owner.
    let throw_statements = aggregate_owned_throw_statements(owner, source_file);
    for throw in throw_statements {
        let keyword = astnav::find_child_of_kind(throw, SyntaxKind::ThrowKeyword, source_file);
        if keyword.is_some() {
            keywords.push(keyword);
        }
    }

    // If the "owner" is a function, then we equate 'return' and 'throw' statements in their
    // ability to "jump out" of the function, and include occurrences for both
    if is_function_block(owner) {
        for_each_return_statement(owner, |ret| {
            let keyword = astnav::find_child_of_kind(ret, SyntaxKind::ReturnKeyword, source_file);
            if keyword.is_some() {
                keywords.push(keyword);
            }
            false // continue traversal
        });
    }

    keywords
}

// Go: ls/documenthighlights.go:490 getThrowStatementOwner
// For lack of a better name, this function takes a throw statement and returns the
// nearest ancestor that is a try-block (whose try statement has a catch clause),
// function-block, or source file.
pub fn get_throw_statement_owner(throw_statement: Node) -> Node {
    let mut child = throw_statement;
    while child.parent().is_some() {
        let parent = child.parent();

        if is_function_block(parent) || parent.kind() == SyntaxKind::SourceFile {
            return parent;
        }

        // A throw-statement is only owned by a try-statement if the try-statement has
        // a catch clause, and if the throw-statement occurs within the try block.
        if is_try_statement(parent) {
            let try_statement = parent;
            if try_statement.try_block() == child && try_statement.catch_clause().is_some() {
                return child;
            }
        }

        child = parent;
    }
    Node::NIL
}

// Go: ls/documenthighlights.go:513 getTryCatchFinallyOccurrences
pub fn get_try_catch_finally_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let try_statement = node;

    let mut keywords: Vec<Node> = Vec::new();
    let token = lsutil::get_first_token(node, source_file);
    if token.is_some() && token.kind() == SyntaxKind::TryKeyword {
        keywords.push(token);
    }

    if try_statement.catch_clause().is_some() {
        let catch_token = astnav::find_child_of_kind(node, SyntaxKind::CatchKeyword, source_file);
        if catch_token.is_some() {
            keywords.push(catch_token);
        }
    }

    if try_statement.finally_block().is_some() {
        let finally_keyword =
            astnav::find_child_of_kind(node, SyntaxKind::FinallyKeyword, source_file);
        if finally_keyword.is_some() {
            keywords.push(finally_keyword);
        }
    }

    keywords
}

// Go: ls/documenthighlights.go:537 getSwitchCaseDefaultOccurrences
pub fn get_switch_case_default_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let switch_statement = node;

    let mut keywords: Vec<Node> = Vec::new();
    let token = lsutil::get_first_token(node, source_file);
    if token.kind() == SyntaxKind::SwitchKeyword {
        keywords.push(token);
    }

    let clauses = switch_statement.case_block().clauses();
    for clause in clauses.nodes() {
        let clause_token = lsutil::get_first_token(clause, source_file);
        if clause_token.kind() == SyntaxKind::CaseKeyword
            || clause_token.kind() == SyntaxKind::DefaultKeyword
        {
            keywords.push(clause_token);
        }

        let break_and_continue_statements =
            aggregate_all_break_and_continue_statements(clause, source_file);
        for statement in break_and_continue_statements {
            if statement.kind() == SyntaxKind::BreakStatement
                && owns_break_or_continue_statement(switch_statement, statement)
            {
                keywords.push(lsutil::get_first_token(statement, source_file));
            }
        }
    }

    keywords
}

// Go: ls/documenthighlights.go:564 aggregateAllBreakAndContinueStatements
pub fn aggregate_all_break_and_continue_statements(node: Node, source_file: Node) -> Vec<Node> {
    if is_break_or_continue_statement(node) {
        return vec![node];
    }
    if is_function_like(node) {
        return Vec::new();
    }
    flat_map_children(
        node,
        source_file,
        &aggregate_all_break_and_continue_statements,
    )
}

// Go: ls/documenthighlights.go:574 ownsBreakOrContinueStatement
pub fn owns_break_or_continue_statement(owner: Node, statement: Node) -> bool {
    let actual_owner = get_break_or_continue_owner(statement);
    if actual_owner.is_nil() {
        return false;
    }
    actual_owner == owner
}

// Go: ls/documenthighlights.go:582 getBreakOrContinueOwner
pub fn get_break_or_continue_owner(statement: Node) -> Node {
    find_ancestor_or_quit(statement, |node| match node.kind() {
        SyntaxKind::SwitchStatement
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::WhileStatement
        | SyntaxKind::DoStatement => {
            // PORT: Go `case KindSwitchStatement` returns false for a continue
            // statement, else falls through to the loop cases.
            if node.kind() == SyntaxKind::SwitchStatement
                && statement.kind() == SyntaxKind::ContinueStatement
            {
                return FindAncestorResult::FIND_ANCESTOR_FALSE;
            }
            // If the statement is labeled, check if the node is labeled by the statement's label.
            if statement.label().is_nil() || is_labeled_by(node, statement.label().text()) {
                return FindAncestorResult::FIND_ANCESTOR_TRUE;
            }
            FindAncestorResult::FIND_ANCESTOR_FALSE
        }
        _ => {
            // Don't cross function boundaries.
            if is_function_like(node) {
                return FindAncestorResult::FIND_ANCESTOR_QUIT;
            }
            FindAncestorResult::FIND_ANCESTOR_FALSE
        }
    })
}

// Go: ls/documenthighlights.go:612 isLabeledBy
// Whether or not a 'node' is preceded by a label of the given string.
// Note: 'node' cannot be a SourceFile.
pub fn is_labeled_by(node: Node, label_name: &str) -> bool {
    find_ancestor_or_quit(node.parent(), |owner| {
        if !is_labeled_statement(owner) {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }
        if owner.label().text() == label_name {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        FindAncestorResult::FIND_ANCESTOR_FALSE
    })
    .is_some()
}

// Go: ls/documenthighlights.go:624 getBreakOrContinueStatementOccurrences
pub fn get_break_or_continue_statement_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let owner = get_break_or_continue_owner(node);
    if owner.is_some() {
        match owner.kind() {
            SyntaxKind::ForStatement
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement => {
                return get_loop_break_continue_occurrences(owner, source_file);
            }
            SyntaxKind::SwitchStatement => {
                return get_switch_case_default_occurrences(owner, source_file);
            }
            _ => {}
        }
    }
    Vec::new()
}

// Go: ls/documenthighlights.go:636 getLoopBreakContinueOccurrences
pub fn get_loop_break_continue_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let mut keywords: Vec<Node> = Vec::new();

    let token = lsutil::get_first_token(node, source_file);
    if token.kind() == SyntaxKind::ForKeyword
        || token.kind() == SyntaxKind::DoKeyword
        || token.kind() == SyntaxKind::WhileKeyword
    {
        keywords.push(token);
        if node.kind() == SyntaxKind::DoStatement {
            let loop_tokens = get_children_from_non_js_doc_node(node, source_file);
            for i in (0..loop_tokens.len()).rev() {
                if loop_tokens[i].kind() == SyntaxKind::WhileKeyword {
                    keywords.push(loop_tokens[i]);
                    break;
                }
            }
        }
    }

    let break_and_continue_statements =
        aggregate_all_break_and_continue_statements(node, source_file);
    for statement in break_and_continue_statements {
        let token = lsutil::get_first_token(statement, source_file);
        if owns_break_or_continue_statement(node, statement)
            && (token.kind() == SyntaxKind::BreakKeyword
                || token.kind() == SyntaxKind::ContinueKeyword)
        {
            keywords.push(token);
        }
    }

    keywords
}

// Go: ls/documenthighlights.go:664 getAsyncAndAwaitOccurrences
pub fn get_async_and_await_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let fun = get_containing_function(node);
    if fun.is_nil() {
        return Vec::new();
    }

    let mut keywords: Vec<Node> = Vec::new();

    for modifier in fun.modifier_nodes() {
        if modifier.kind() == SyntaxKind::AsyncKeyword {
            keywords.push(modifier);
        }
    }

    fun.for_each_child(|child| {
        traverse_without_crossing_function(child, source_file, &mut |child| {
            if is_await_expression(child) {
                let token = lsutil::get_first_token(child, source_file);
                if token.kind() == SyntaxKind::AwaitKeyword {
                    keywords.push(token);
                }
            }
        });
        false // continue traversal
    });

    keywords
}

// Go: ls/documenthighlights.go:693 getYieldOccurrences
pub fn get_yield_occurrences(node: Node, source_file: Node) -> Vec<Node> {
    let parent_func = find_ancestor(node.parent(), is_function_like);
    if parent_func.is_nil() {
        return Vec::new();
    }

    let mut keywords: Vec<Node> = Vec::new();

    parent_func.for_each_child(|child| {
        traverse_without_crossing_function(child, source_file, &mut |child| {
            if is_yield_expression(child) {
                let token = lsutil::get_first_token(child, source_file);
                if token.kind() == SyntaxKind::YieldKeyword {
                    keywords.push(token);
                }
            }
        });
        false // continue traversal
    });

    keywords
}

// Go: ls/documenthighlights.go:716 traverseWithoutCrossingFunction
pub fn traverse_without_crossing_function(node: Node, source_file: Node, cb: &mut dyn FnMut(Node)) {
    cb(node);
    if !is_function_like(node)
        && !is_class_like(node)
        && !is_interface_declaration(node)
        && !is_module_declaration(node)
        && !is_type_alias_declaration(node)
        && !is_type_node(node)
    {
        node.for_each_child(|child| {
            traverse_without_crossing_function(child, source_file, &mut *cb);
            false // continue traversal
        });
    }
}

// Go: ls/documenthighlights.go:726 getModifierOccurrences
pub fn get_modifier_occurrences(kind: SyntaxKind, node: Node, source_file: Node) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();

    let nodes_to_search = get_nodes_to_search_for_modifier(node, modifier_to_flag(kind));
    for n in nodes_to_search {
        let modifier = find_modifier(n, kind);
        if modifier.is_some() {
            result.push(modifier);
        }
    }
    result
}

// Go: ls/documenthighlights.go:739 getNodesToSearchForModifier
pub fn get_nodes_to_search_for_modifier(
    declaration: Node,
    modifier_flag: ModifierFlags,
) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();

    let container = declaration.parent();
    if container.is_nil() {
        return Vec::new();
    }

    // Types of node whose children might have modifiers.
    match container.kind() {
        SyntaxKind::ModuleBlock
        | SyntaxKind::SourceFile
        | SyntaxKind::Block
        | SyntaxKind::CaseClause
        | SyntaxKind::DefaultClause => {
            // Container is either a class declaration or the declaration is a classDeclaration
            if modifier_flag.intersects(ModifierFlags::ABSTRACT)
                && is_class_declaration(declaration)
            {
                result.extend(declaration.members());
                result.push(declaration);
                result
            } else {
                result.extend(container.statements());
                result
            }
        }
        SyntaxKind::Constructor
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::FunctionDeclaration => {
            // Parameters and, if inside a class, also class members
            result.extend(container.parameters());
            if is_class_like(container.parent()) {
                result.extend(container.parent().members());
            }
            result
        }
        SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeLiteral => {
            let nodes = container.members();
            result.extend(nodes);
            // If we're an accessibility modifier, we're in an instance member and should search
            // the constructor's parameter list for instance members as well.
            if modifier_flag
                .intersects(ModifierFlags::ACCESSIBILITY_MODIFIER | ModifierFlags::READONLY)
            {
                let mut constructor = Node::NIL;

                for member in nodes {
                    if is_constructor_declaration(member) {
                        constructor = member;
                        break;
                    }
                }
                if constructor.is_some() {
                    result.extend(constructor.parameters());
                }
            } else if modifier_flag.intersects(ModifierFlags::ABSTRACT) {
                result.push(container);
            }
            result
        }
        _ => {
            // Syntactically invalid positions or unsupported containers
            Vec::new()
        }
    }
}

// Go: ls/documenthighlights.go:790 findModifier
pub fn find_modifier(node: Node, kind: SyntaxKind) -> Node {
    for modifier in node.modifier_nodes() {
        if modifier.kind() == kind {
            return modifier;
        }
    }
    Node::NIL
}
