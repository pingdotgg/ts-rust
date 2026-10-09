use crate::ls::prelude::*;

use crate::spanmap::{Feature, Fidelity};

// Go: ls/hover.go:20 symbolFormatFlags
pub const SYMBOL_FORMAT_FLAGS: SymbolFormatFlags =
    SymbolFormatFlags::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS
        .union(SymbolFormatFlags::USE_ONLY_EXTERNAL_ALIASING)
        .union(SymbolFormatFlags::ALLOW_ANY_NODE_KIND)
        .union(SymbolFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE);

// Go: ls/hover.go:24 typeFormatFlags
pub const TYPE_FORMAT_FLAGS: TypeFormatFlags =
    TypeFormatFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
        .union(TypeFormatFlags::USE_INSTANTIATION_EXPRESSIONS);

impl LanguageService {
    // Go: ls/hover.go:27 ProvideHover
    pub fn provide_hover(
        &self,
        ctx: &Context,
        params: &lsproto::HoverParams,
    ) -> Result<lsproto::HoverResponse, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        let content_format =
            lsproto::preferred_markup_kind(&caps.text_document.hover.content_format);

        let mut verbosity_level = 0;
        if let Some(level) = params.verbosity_level {
            verbosity_level = level;
        }

        let (program, file) = self.get_program_and_file(&params.text_document.uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            params.position,
            Feature::HOVER,
        );
        let mut hovers: Vec<lsproto::Hover> = Vec::new();
        for projection in &positions {
            if !projection.fidelity.is_single_segment() {
                continue;
            }
            let file = projection.script;
            let position = projection.position;
            let node = astnav::get_touching_property_name(file, position);
            if is_source_file(node)
                || is_property_access_or_qualified_name(node)
                    && is_in_comment(file, position, node).is_none()
            {
                // Avoid giving quickInfo for the sourceFile as a whole or inside the comment of a/**/.b
                continue;
            }
            // PORT: Go calls `done()` before each `continue` and at the end of
            // the iteration. `done` releases the checker when it drops at the
            // end of the iteration.
            let (checker, done) = ls_program::get_type_checker_for_file(program, ctx, file);
            let c = &mut *checker.borrow_mut();
            let range_node = get_node_for_quick_info(node);
            let symbol = get_symbol_at_location_for_quick_info(c, range_node);

            // Always create VerbosityContext for hover so that canExpandSymbol can signal
            // canIncreaseVerbosity even at Level 0. The nodebuilder also detects expandable
            // types at Level 0 via shouldExpandType (maxExpansionDepth = 0).
            let mut max_trunc_len = self.user_preferences().maximum_hover_length;
            if max_trunc_len <= 0 {
                max_trunc_len = 500;
            }
            let vc = VerbosityContext {
                level: verbosity_level,
                max_truncation_length: max_trunc_len,
                ..Default::default()
            };

            let vs_capability = caps.vs_supports_visual_studio_extensions;
            let (quick_info, documentation, vs_documentation, quick_info_runs) = self
                .get_quick_info_and_documentation_for_symbol(
                    c,
                    symbol,
                    range_node,
                    content_format.clone(),
                    Some(&vc),
                    vs_capability,
                );
            if quick_info.is_empty() {
                continue;
            }
            let range_file = get_source_file_of_node(range_node);
            let text_range = get_range_of_node(range_node, range_file, Node::NIL /*endNode*/);
            let (hover_range, hover_fidelity) =
                self.converters
                    .to_lsp_range_for_feature(&range_file, text_range, Feature::HOVER);

            let content = if content_format == lsproto::MarkupKind::MARKDOWN {
                format_quick_info(&quick_info) + &documentation
            } else {
                quick_info + &documentation
            };

            let mut hover = lsproto::Hover {
                contents: lsproto::MarkupContentOrStringOrMarkedStringWithLanguageOrMarkedStrings {
                    markup_content: Some(lsproto::MarkupContent {
                        kind: content_format.clone(),
                        value: content,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            };
            if hover_fidelity.is_single_segment() {
                hover.range = Some(hover_range);
            }

            if caps.experimental.hover_verbosity_level {
                hover.can_increase_verbosity =
                    vc.can_increase_verbosity.get() && !vc.truncated.get();
            }

            // Clients that support Visual Studio extensions (e.g. VS itself, when Corsa/Native TS Preview is
            // enabled) render `_vs_rawContent` in place of `contents`. Without it, VS shows plain markdown
            // with no symbol icon and no syntax coloring, unlike the legacy TSServer-backed hover path.
            if vs_capability && !quick_info_runs.is_empty() {
                let mut kind = lsutil::ScriptElementKind::KEYWORD;
                let mut modifiers = lsutil::ScriptElementKindModifier::NONE;
                if symbol.is_some() {
                    // Resolve aliases to their target before computing the icon kind, so e.g. `import { x }`
                    // shows the icon for whatever `x` actually is (const, function, ...) rather than a
                    // generic alias icon. GetSymbolModifiers already accounts for the alias target itself.
                    let mut icon_symbol = symbol;
                    if c.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
                        let resolved = c.get_aliased_symbol(symbol);
                        if resolved.is_some() && resolved != symbol {
                            icon_symbol = resolved;
                        }
                    }
                    kind = lsutil::get_symbol_kind(Some(&mut *c), icon_symbol, range_node);
                    modifiers = lsutil::get_symbol_modifiers(Some(&mut *c), symbol);
                }
                let image_id = get_vs_hover_image_id(kind, modifiers);
                let mut documentation_runs: Vec<lsproto::VSClassifiedTextRun> = Vec::new();
                let doc_text = vs_documentation.trim_start_matches('\n');
                if !doc_text.is_empty() {
                    documentation_runs = vec![lsproto::VSClassifiedTextRun {
                        classification_type_name: lsproto::ClassificationTypeName::TEXT
                            .0
                            .to_string(),
                        text: doc_text.to_string(),
                        ..Default::default()
                    }];
                }
                hover.vs_raw_content =
                    build_vs_hover_raw_content(image_id, quick_info_runs, documentation_runs);
            }

            hovers.push(hover);
        }
        if hovers.is_empty() {
            return Ok(lsproto::HoverOrNull::default());
        }
        if hovers.len() == 1 {
            return Ok(lsproto::HoverOrNull {
                hover: hovers.pop(),
            });
        }

        let mut contents: Vec<String> = Vec::with_capacity(hovers.len());
        let mut seen_contents: FxHashSet<String> = FxHashSet::default();
        let mut raw_contents: Vec<
            lsproto::VSImageElementOrClassifiedTextElementOrContainerElement,
        > = Vec::new();
        let mut can_increase_verbosity = hovers[0].can_increase_verbosity;
        let mut common_range = hovers[0].range;
        for hover in &hovers {
            // PORT: Go dereferences `hover.Contents.MarkupContent`; every hover
            // made above sets it.
            let content = hover
                .contents
                .markup_content
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .value
                .trim_end_matches('\n')
                .to_string();
            if seen_contents.insert(content.clone()) {
                contents.push(content);
                if let Some(vs_raw_content) = &hover.vs_raw_content {
                    raw_contents.push(
                        lsproto::VSImageElementOrClassifiedTextElementOrContainerElement {
                            container_element: Some(vs_raw_content.clone()),
                            ..Default::default()
                        },
                    );
                }
            }
            can_increase_verbosity = can_increase_verbosity || hover.can_increase_verbosity;
            if common_range.is_none() || hover.range.is_none() || common_range != hover.range {
                common_range = None;
            }
        }
        // PORT: Go `combined := hovers[0]` is a pointer; the fields it sets
        // above and below are set on the first hover here.
        let mut combined = hovers.swap_remove(0);
        combined.can_increase_verbosity = can_increase_verbosity;
        let mut separator = "\n\n";
        if content_format == lsproto::MarkupKind::MARKDOWN {
            separator = "\n\n---\n\n";
        }
        combined
            .contents
            .markup_content
            .as_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .value = contents.join(separator);
        combined.range = common_range;
        match raw_contents.len() {
            0 => combined.vs_raw_content = None,
            1 => combined.vs_raw_content = raw_contents.pop().unwrap().container_element,
            _ => {
                combined.vs_raw_content = Some(lsproto::VSContainerElement {
                    style: lsproto::VSContainerElementStyle::STACKED,
                    elements: raw_contents,
                    ..Default::default()
                });
            }
        }
        Ok(lsproto::HoverOrNull {
            hover: Some(combined),
        })
    }

    // Go: ls/hover.go:174 getQuickInfoAndDocumentationForSymbol
    pub fn get_quick_info_and_documentation_for_symbol(
        &self,
        c: &mut Checker,
        symbol: SymbolId,
        node: Node,
        content_format: lsproto::MarkupKind,
        vc: Option<&VerbosityContext>,
        vs_capability: bool,
    ) -> (String, String, String, Vec<lsproto::VSClassifiedTextRun>) {
        let content_format = &content_format;
        let meaning = get_meaning_from_location(node);
        let info =
            get_quick_info_and_declaration_at_location(c, symbol, node, vc, vs_capability, meaning);
        let quick_info = info.display_parts.borrow().string();
        if quick_info.is_empty() {
            return (String::new(), String::new(), String::new(), Vec::new());
        }
        let quick_info_runs = info.display_parts.borrow().get_runs(c);

        let documentation = get_documentation_for_symbol(
            &self.documentation_location_mapper(Feature::HOVER),
            c,
            symbol,
            node,
            info.declaration,
            content_format,
            false, /*commentOnly*/
        );

        // VS's rich hover (_vs_rawContent) renders documentation as plain colorized text with no Markdown
        // parser, so it can't use the tag section (@param/@returns/@example/@see, etc.) that
        // getDocumentationFromDeclaration renders with '*@tag*' bolding and ```-fenced @example blocks --
        // those would show up as literal asterisks/backticks. This also matches the legacy TSServer-backed
        // VS hover (TypeScript-VS's HoverService.cs), which only ever surfaced the JSDoc summary
        // (TSServer's quickinfo `documentation`) and never included the tag section at all (TSServer
        // exposes tags via a separate `tags` field that legacy VS hover never read). So request
        // comment-only, plain-text documentation for the VS path instead of reusing `documentation`.
        let mut vs_documentation = String::new();
        if vs_capability {
            vs_documentation = get_documentation_for_symbol(
                &self.documentation_location_mapper(Feature::HOVER),
                c,
                symbol,
                node,
                info.declaration,
                &lsproto::MarkupKind::PLAIN_TEXT,
                true, /*commentOnly*/
            );
        }

        (quick_info, documentation, vs_documentation, quick_info_runs)
    }
}

// Go: ls/hover.go:204 documentationLocationMapper
/// `l.documentationLocationMapper(feature)` or `noMappedLocation`.
// PORT: Go passes the func value; here a reference to the closure.
pub type DocumentationLocationMapper<'a> =
    &'a dyn Fn(Node, TextRange) -> (lsproto::Location, Fidelity);

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/hover.go:206 documentationLocationMapper
    pub fn documentation_location_mapper(
        &self,
        feature: Feature,
    ) -> impl Fn(Node, TextRange) -> (lsproto::Location, Fidelity) + '_ {
        move |file: Node, file_range: TextRange| {
            self.source_file_range_to_lsp_location_for_feature(file, file_range, feature)
        }
    }
}

// Go: ls/hover.go:212 getDocumentationForSymbol
// getDocumentationForSymbol tries each documentation source in turn (call-signature documentation,
// declaration JSDoc, root-symbol JSDoc, alias target JSDoc) and returns the first non-empty result,
// formatted for contentFormat. commentOnly restricts the result to the JSDoc summary, excluding the
// @tag section.
pub fn get_documentation_for_symbol(
    get_mapped_location: DocumentationLocationMapper<'_>,
    c: &mut Checker,
    symbol: SymbolId,
    node: Node,
    declaration: Node,
    content_format: &lsproto::MarkupKind,
    comment_only: bool,
) -> String {
    let call_node = get_call_or_new_expression(node);
    let mut documentation = documentation_from_signature(
        get_mapped_location,
        c,
        symbol,
        call_node,
        node,
        content_format,
        comment_only,
    );
    if !documentation.is_empty() {
        return documentation;
    }

    documentation = documentation_from_root_symbols(
        get_mapped_location,
        c,
        symbol,
        node,
        content_format,
        comment_only,
    );
    if !documentation.is_empty() {
        return documentation;
    }

    documentation = get_documentation_from_declaration(
        get_mapped_location,
        c,
        symbol,
        declaration,
        node,
        content_format,
        comment_only,
    );
    if !documentation.is_empty() {
        return documentation;
    }

    documentation_from_alias(
        get_mapped_location,
        c,
        symbol,
        node,
        content_format,
        comment_only,
    )
}

// Go: ls/hover.go:231 documentationFromSignature
pub fn documentation_from_signature(
    get_mapped_location: DocumentationLocationMapper<'_>,
    c: &mut Checker,
    symbol: SymbolId,
    node: Node,
    location: Node,
    content_format: &lsproto::MarkupKind,
    comment_only: bool,
) -> String {
    if node.is_nil() {
        return String::new();
    }
    let signature = c.get_resolved_signature_exported(node);
    if signature.is_nil() {
        return String::new();
    }
    let declaration = c.sig(signature).declaration;
    if declaration.is_nil() {
        return String::new();
    }
    if is_call_signature_declaration(declaration) || is_construct_signature_declaration(declaration)
    {
        return get_documentation_from_declaration(
            get_mapped_location,
            c,
            symbol,
            declaration,
            location,
            content_format,
            comment_only,
        );
    }
    String::new()
}

// Go: ls/hover.go:249 documentationFromAlias
pub fn documentation_from_alias(
    get_mapped_location: DocumentationLocationMapper<'_>,
    c: &mut Checker,
    symbol: SymbolId,
    node: Node,
    content_format: &lsproto::MarkupKind,
    comment_only: bool,
) -> String {
    if symbol.is_nil() || !c.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
        return String::new();
    }

    let aliased_symbol = c.get_aliased_symbol(symbol);
    if aliased_symbol.is_nil() || aliased_symbol == c.get_unknown_symbol() {
        return String::new();
    }

    let mut candidates = vec![aliased_symbol];
    let export_symbol = c.sym(aliased_symbol).export_symbol;
    if export_symbol.is_some() {
        candidates.push(export_symbol);
    }

    for candidate in candidates {
        let value_declaration = c.sym(candidate).value_declaration;
        let aliased_declaration = if value_declaration.is_some() {
            value_declaration
        } else {
            c.sym(candidate)
                .declarations
                .first()
                .copied()
                .unwrap_or(Node::NIL)
        };
        if aliased_declaration.is_nil() {
            continue;
        }

        let documentation = get_documentation_from_declaration(
            get_mapped_location,
            c,
            candidate,
            aliased_declaration,
            node,
            content_format,
            comment_only,
        );
        if !documentation.is_empty() {
            return documentation;
        }
    }

    String::new()
}

// Go: ls/hover.go:278 documentationFromRootSymbols
pub fn documentation_from_root_symbols(
    get_mapped_location: DocumentationLocationMapper<'_>,
    c: &mut Checker,
    symbol: SymbolId,
    node: Node,
    content_format: &lsproto::MarkupKind,
    comment_only: bool,
) -> String {
    if symbol.is_nil() {
        return String::new();
    }

    let root_symbols = c.get_root_symbols(symbol);
    if root_symbols.len() <= 1 {
        return String::new();
    }

    let mut docs: Vec<String> = Vec::new();
    for root_symbol in root_symbols {
        if root_symbol.is_nil() {
            continue;
        }
        let mut declarations = c.sym(root_symbol).declarations.clone();
        let value_declaration = c.sym(root_symbol).value_declaration;
        if declarations.is_empty() && value_declaration.is_some() {
            declarations = vec![value_declaration].into();
        }
        for declaration in declarations {
            let documentation = get_documentation_from_declaration(
                get_mapped_location,
                c,
                root_symbol,
                declaration,
                node,
                content_format,
                comment_only,
            );
            if !documentation.is_empty() && !docs.contains(&documentation) {
                docs.push(documentation);
            }
        }
    }
    docs.join("\n")
}

// Go: ls/hover.go:306 getDocumentationFromDeclaration
pub fn get_documentation_from_declaration(
    get_mapped_location: DocumentationLocationMapper<'_>,
    c: &mut Checker,
    symbol: SymbolId,
    declaration: Node,
    location: Node,
    content_format: &lsproto::MarkupKind,
    comment_only: bool,
) -> String {
    if declaration.is_nil() {
        return String::new();
    }
    let is_markdown = *content_format == lsproto::MarkupKind::MARKDOWN;
    let mut b = String::new();
    let jsdoc = get_js_doc_or_tag(c, declaration, &mut FxHashSet::default());
    if jsdoc.is_some()
        && !(!declaration.flags().intersects(NodeFlags::REPARSED) && contains_typedef_tag(jsdoc))
    {
        write_comments(
            get_mapped_location,
            &mut b,
            c,
            &jsdoc.comments().to_vec(),
            is_markdown,
        );
        if jsdoc.kind() == SyntaxKind::JsDoc && !comment_only {
            let tags = jsdoc.tags();
            if tags.is_some() {
                for tag in tags.nodes() {
                    if tag.kind() == SyntaxKind::JsDocTypeTag
                        || tag.kind() == SyntaxKind::JsDocTypedefTag
                        || tag.kind() == SyntaxKind::JsDocCallbackTag
                    {
                        continue;
                    }
                    b.push_str("\n\n");
                    if is_markdown {
                        b.push_str("*@");
                        b.push_str(tag.tag_name().text());
                        b.push('*');
                    } else {
                        b.push('@');
                        b.push_str(tag.tag_name().text());
                    }
                    match tag.kind() {
                        SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag => {
                            write_optional_entity_name(&mut b, tag.name());
                        }
                        SyntaxKind::JsDocAugmentsTag => {
                            write_optional_entity_name(&mut b, tag.class_name());
                        }
                        SyntaxKind::JsDocTemplateTag => {
                            for (i, tp) in tag.type_parameters().iter().enumerate() {
                                if i != 0 {
                                    b.push(',');
                                }
                                write_optional_entity_name(&mut b, tp.name());
                            }
                        }
                        _ => {}
                    }
                    let comments = tag.comments().to_vec();
                    if tag.kind() == SyntaxKind::JsDocUnknownTag
                        && tag.tag_name().text() == "example"
                    {
                        let mut comment_text = get_text_of_js_doc_comment(tag.comment_list());
                        if comment_text.starts_with("<caption>") {
                            if let Some(caption_end) = comment_text.find("</caption>") {
                                if caption_end > 0 {
                                    b.push_str(" — ");
                                    b.push_str(&comment_text["<caption>".len()..caption_end]);
                                    comment_text = comment_text[caption_end + "</caption>".len()..]
                                        .to_string();
                                    // Trim leading blank lines from commentText
                                    loop {
                                        let s1 = comment_text
                                            .trim_start_matches(|ch: char| ch == ' ' || ch == '\t');
                                        let s2 = s1.trim_start_matches(|ch: char| {
                                            ch == '\r' || ch == '\n'
                                        });
                                        if s1.len() == s2.len() {
                                            break;
                                        }
                                        comment_text = s2.to_string();
                                    }
                                }
                            }
                        }
                        b.push('\n');
                        if comment_text.len() > 6
                            && comment_text.starts_with("```")
                            && comment_text.ends_with("```")
                            && comment_text.contains('\n')
                        {
                            b.push_str(&comment_text);
                            b.push('\n');
                        } else {
                            write_code(&mut b, "tsx", &comment_text);
                        }
                    } else if tag.kind() == SyntaxKind::JsDocSeeTag
                        && tag.name_expression().is_some()
                    {
                        b.push_str(" — ");
                        write_name_link(
                            get_mapped_location,
                            &mut b,
                            c,
                            tag.name_expression().name(),
                            "",
                            false, /*quote*/
                            is_markdown,
                        );
                        if !comments.is_empty() {
                            b.push(' ');
                            write_comments(get_mapped_location, &mut b, c, &comments, is_markdown);
                        }
                    } else if tag.kind() == SyntaxKind::JsDocThrowsTag
                        && tag.type_expression().is_some()
                    {
                        b.push_str(" — ");
                        b.push_str(&get_text_of_node(tag.type_expression()));
                        if !comments.is_empty() {
                            b.push(' ');
                            write_comments(get_mapped_location, &mut b, c, &comments, is_markdown);
                        }
                    } else if !comments.is_empty() {
                        b.push(' ');
                        if comments[0].kind() != SyntaxKind::JsDocText
                            || !comments[0].text().starts_with('-')
                        {
                            b.push_str("— ");
                        }
                        write_comments(get_mapped_location, &mut b, c, &comments, is_markdown);
                    }
                }
            }
        }
    }
    b
}

// Go: ls/hover.go:396 formatQuickInfo
pub fn format_quick_info(quick_info: &str) -> String {
    let mut b = String::with_capacity(32);
    write_code(&mut b, "typescript", quick_info);
    b
}

// Go: ls/hover.go:403 shouldGetType
pub fn should_get_type(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::Identifier => {
            // If we're in a JSDoc node with no associated symbol, no binding has taken place for the node and
            // we can't answer questions about types of declaration nodes (such as property declarations).
            !(node.flags().intersects(NodeFlags::JS_DOC) && is_declaration_name(node))
                && !is_label_name(node)
                && !is_tag_name(node)
                && !is_const_type_reference(node.parent())
        }
        SyntaxKind::ThisKeyword
        | SyntaxKind::ThisType
        | SyntaxKind::SuperKeyword
        | SyntaxKind::NamedTupleMember => true,
        SyntaxKind::MetaProperty => is_import_meta(node),
        _ => false,
    }
}

// Go: ls/hover.go:419 symbolDisplayInfo
/// symbolDisplayInfo holds the result of getSymbolDisplayPartsDocumentationAndSymbolKind.
// PORT: Go `*displayPartsWriter` is the shared handle `Rc<RefCell<DisplayPartsWriter>>`.
pub struct SymbolDisplayInfo {
    pub display_parts: Rc<RefCell<DisplayPartsWriter>>,
    pub declaration: Node,
}

// Go: ls/hover.go:440 classifiedNodeBuilderFlags (local const of getQuickInfoAndDeclarationAtLocation)
// nodeBuilderFlags for classified output (same as signatureHelpNodeBuilderFlags)
const CLASSIFIED_NODE_BUILDER_FLAGS: NodeBuilderFlags = NodeBuilderFlags::IGNORE_ERRORS
    .union(NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE)
    .union(NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME);

// PORT: Go getQuickInfoAndDeclarationAtLocation builds its output with local
// closures (writeTypeClassified, writeSignatureClassified,
// writeSymbolClassified, setDeclaration, writeNewLine, writeSignatures,
// writeTypeParams, canExpandSymbol, tryExpandSymbol, writeSymbol) that share
// the checker, the writer, `vc` and the alias state. Rust closures cannot call
// each other while all of them borrow the checker mutably, so the shared
// state is this struct and each Go closure is a method with the Go name.
struct QuickInfoWriter<'c> {
    c: &'c mut Checker,
    node: Node,
    container: Node,
    vc: VerbosityContext,
    vs_capability: bool,
    meaning: SemanticMeaning,
    dpw: Rc<RefCell<DisplayPartsWriter>>,
    source_file: Node,
    visited_aliases: FxHashSet<SymbolId>,
    alias_level: i32,
    first_declaration: Node,
    symbol_was_expanded: bool,
    // Go: displayEmitContext (ts#64649, hover.go:441), made on first use.
    display_emit_context: Option<Rc<EmitContext>>,
}

impl QuickInfoWriter<'_> {
    // Go: ls/hover.go:442 getEmitContext (closure, ts#64649)
    // One emit context for the whole quick info.
    fn get_emit_context(&mut self) -> Rc<EmitContext> {
        self.display_emit_context
            .get_or_insert_with(new_emit_context)
            .clone()
    }

    // Go: ls/hover.go:444 writeTypeClassified (closure)
    // writeTypeClassified writes a type to dpw with proper classification (punctuation, symbols, keywords).
    // Falls back to flat text when vsCapability is false or when TypeToTypeNode fails.
    fn write_type_classified(&mut self, t: TypeId, enclosing: Node, flags: TypeFormatFlags) {
        let flags = flags | TypeFormatFlags::MULTILINE_OBJECT_LITERALS;
        if !self.vs_capability {
            let text = self
                .c
                .type_to_string_ex(t, enclosing, flags, Some(&self.vc));
            self.dpw.borrow_mut().write(&text);
            return;
        }
        let emit_context = self.get_emit_context();
        // PORT: Go shares one idToSymbol map between the node builder and the
        // printer. The Rust builder owns the map; it is moved into the printer
        // after the node is built.
        let nb = Rc::new(RefCell::new(new_node_builder_ex(
            self.c,
            emit_context.clone(),
            Some(FxHashMap::default()),
        )));
        let combined_flags = NodeBuilderFlags((flags & TypeFormatFlags::NODE_BUILDER_FLAGS_MASK).0)
            | CLASSIFIED_NODE_BUILDER_FLAGS;
        let type_node = self.c.node_builder_type_to_type_node(
            &nb,
            t,
            enclosing,
            combined_flags,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        if type_node.is_nil() {
            let text = self
                .c
                .type_to_string_ex(t, enclosing, flags, Some(&self.vc));
            self.dpw.borrow_mut().write(&text);
            return;
        }
        let mut p = new_printer(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..Default::default()
            },
            PrintHandlers::default(),
            Some(emit_context),
        );
        let id_to_symbol = std::mem::take(&mut nb.borrow().impl_.borrow_mut().id_to_symbol);
        p.id_to_symbol = Some(id_to_symbol);
        let temp_dpw = new_display_parts_writer(true);
        p.write_exported(type_node, self.source_file, temp_dpw.clone(), None);
        self.dpw.borrow_mut().write_from(&temp_dpw.borrow());
    }

    // Go: ls/hover.go:467 writeSignatureClassified (closure)
    // writeSignatureClassified writes a signature to dpw with proper classification.
    fn write_signature_classified(
        &mut self,
        sig: SignatureId,
        enclosing: Node,
        flags: TypeFormatFlags,
    ) {
        let flags = flags | TypeFormatFlags::MULTILINE_OBJECT_LITERALS;
        if !self.vs_capability {
            let text = self
                .c
                .signature_to_string_ex(sig, enclosing, flags, Some(&self.vc));
            self.dpw.borrow_mut().write(&text);
            return;
        }
        let is_constructor = self.c.sig(sig).flags.intersects(SignatureFlags::CONSTRUCT)
            && !flags.intersects(TypeFormatFlags::WRITE_CALL_STYLE_SIGNATURE);
        let sig_output = if flags.intersects(TypeFormatFlags::WRITE_ARROW_STYLE_SIGNATURE) {
            if is_constructor {
                SyntaxKind::ConstructorType
            } else {
                SyntaxKind::FunctionType
            }
        } else if is_constructor {
            SyntaxKind::ConstructSignature
        } else {
            SyntaxKind::CallSignature
        };
        let emit_context = self.get_emit_context();
        // PORT: shared idToSymbol map, see write_type_classified.
        let nb = Rc::new(RefCell::new(new_node_builder_ex(
            self.c,
            emit_context.clone(),
            Some(FxHashMap::default()),
        )));
        let combined_flags = NodeBuilderFlags((flags & TypeFormatFlags::NODE_BUILDER_FLAGS_MASK).0)
            | CLASSIFIED_NODE_BUILDER_FLAGS;
        let sig_node = self.c.node_builder_signature_to_signature_declaration(
            &nb,
            sig,
            sig_output,
            enclosing,
            combined_flags,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        if sig_node.is_nil() {
            let text = self
                .c
                .signature_to_string_ex(sig, enclosing, flags, Some(&self.vc));
            self.dpw.borrow_mut().write(&text);
            return;
        }
        let mut p = new_printer(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..Default::default()
            },
            PrintHandlers::default(),
            Some(emit_context),
        );
        let id_to_symbol = std::mem::take(&mut nb.borrow().impl_.borrow_mut().id_to_symbol);
        p.id_to_symbol = Some(id_to_symbol);
        let temp_dpw = new_display_parts_writer(true);
        p.write_exported(sig_node, self.source_file, temp_dpw.clone(), None);
        self.dpw.borrow_mut().write_from(&temp_dpw.borrow());
    }

    // Go: ls/hover.go:505 writeSymbolClassified (closure)
    // writeSymbolClassified writes a symbol name to dpw with proper classification based on symbol flags.
    fn write_symbol_classified(
        &mut self,
        symbol: SymbolId,
        enclosing: Node,
        meaning: SymbolFlags,
        flags: SymbolFormatFlags,
    ) {
        if !self.vs_capability {
            let text = self
                .c
                .symbol_to_string_ex(symbol, enclosing, meaning, flags);
            self.dpw.borrow_mut().write(&text);
            return;
        }
        // Use WriteSymbol which calls classificationForSymbol to determine the correct classification
        let text = self
            .c
            .symbol_to_string_ex(symbol, enclosing, meaning, flags);
        self.dpw.borrow_mut().write_symbol(&text, symbol);
    }

    // Go: ls/hover.go:515 writeModuleImportAttributes (closure)
    fn write_module_import_attributes(&mut self, symbol: SymbolId) {
        let declaration = self
            .c
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&declaration| {
                is_module_declaration(declaration) && declaration.attributes().is_some()
            })
            .unwrap_or(Node::NIL);
        if declaration.is_nil() {
            return;
        }
        let attributes = declaration.attributes();
        let emit_context = self.get_emit_context();
        emit_context.set_emit_flags(attributes, EmitFlags::SINGLE_LINE);
        let mut p = new_printer(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..Default::default()
            },
            PrintHandlers::default(),
            Some(emit_context),
        );
        let temp_dpw = new_display_parts_writer(self.vs_capability);
        p.write_exported(
            attributes,
            get_source_file_of_node(declaration),
            temp_dpw.clone(),
            None,
        );
        self.dpw.borrow_mut().write_keyword(" with ");
        self.dpw.borrow_mut().write_from(&temp_dpw.borrow());
    }

    // Go: ls/hover.go:545 setDeclaration (closure)
    fn set_declaration(&mut self, declaration: Node) {
        if self.first_declaration.is_nil() {
            self.first_declaration = declaration;
        }
    }

    // Go: ls/hover.go:550 writeNewLine (closure)
    fn write_new_line(&mut self) {
        if !self.dpw.borrow().string().is_empty() {
            self.dpw.borrow_mut().write("\n");
        }
        if self.alias_level != 0 {
            self.dpw.borrow_mut().write_punctuation("(");
            self.dpw.borrow_mut().write("alias");
            self.dpw.borrow_mut().write_punctuation(") ");
        }
    }

    // Go: ls/hover.go:560 writeSignatures (closure)
    fn write_signatures(
        &mut self,
        signatures: &[SignatureId],
        prefix: &str,
        parenthesized: bool,
        symbol: SymbolId,
    ) {
        for (i, &sig) in signatures.iter().enumerate() {
            self.write_new_line();
            if i == 3 && signatures.len() >= 5 {
                self.dpw
                    .borrow_mut()
                    .write_comment(&format!("// +{} more overloads", signatures.len() - 3));
                break;
            }
            if parenthesized {
                self.dpw.borrow_mut().write_punctuation("(");
                self.dpw.borrow_mut().write(prefix);
                self.dpw.borrow_mut().write_punctuation(") ");
            } else {
                self.dpw.borrow_mut().write_keyword(prefix);
            }
            self.write_symbol_classified(
                symbol,
                self.container,
                SymbolFlags::NONE,
                SYMBOL_FORMAT_FLAGS,
            );
            if self.c.sym(symbol).flags.intersects(SymbolFlags::OPTIONAL) {
                self.dpw.borrow_mut().write_punctuation("?");
            }
            self.write_signature_classified(
                sig,
                self.container,
                TYPE_FORMAT_FLAGS
                    | TypeFormatFlags::WRITE_CALL_STYLE_SIGNATURE
                    | TypeFormatFlags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE,
            );
        }
    }

    // Go: ls/hover.go:581 writeTypeParams (closure)
    fn write_type_params(&mut self, params: &[TypeId]) {
        if !params.is_empty() {
            self.dpw.borrow_mut().write_punctuation("<");
            for (i, &tp) in params.iter().enumerate() {
                if i != 0 {
                    self.dpw.borrow_mut().write_punctuation(", ");
                }
                let tp_symbol = self.c.ty(tp).symbol;
                self.write_symbol_classified(
                    tp_symbol,
                    Node::NIL,
                    SymbolFlags::NONE,
                    SYMBOL_FORMAT_FLAGS,
                );
                let cons = self.c.get_constraint_of_type_parameter_exported(tp);
                if cons.is_some() {
                    self.dpw.borrow_mut().write_keyword(" extends ");
                    self.write_type_classified(cons, Node::NIL, TYPE_FORMAT_FLAGS);
                }
                let def = self.c.get_default_from_type_parameter_exported(tp);
                if def.is_some() {
                    self.dpw.borrow_mut().write_operator(" = ");
                    self.write_type_classified(def, Node::NIL, TYPE_FORMAT_FLAGS);
                }
            }
            self.dpw.borrow_mut().write_punctuation(">");
        }
    }

    // Go: ls/hover.go:604 canExpandSymbol (closure)
    fn can_expand_symbol(&mut self, symbol: SymbolId) -> bool {
        // PORT: Go returns false for a nil vc here. vc is never nil at this
        // point (a nil vc is replaced at the start), so the check is dropped.

        // Only offer symbol-level expansion for types that tryExpandSymbol handles:
        // class, interface, enum, namespace/module. For functions/variables/properties,
        // the node builder's probeTypeExpandability detects expandable type components.
        let symbol_flags = self.c.sym(symbol).flags;
        if !symbol_flags
            .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE | SymbolFlags::NAMESPACE)
        {
            return false;
        }
        let t = if symbol_flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE) {
            self.c.get_declared_type_of_symbol_exported(symbol)
        } else {
            self.c.get_type_of_symbol_at_location(symbol, self.node)
        };
        if t.is_nil() || self.c.is_lib_type_for_hover_verbosity(t) {
            return false;
        }
        if self.vc.level > 0 {
            return true;
        }
        // At level 0, signal that expansion is possible but don't expand
        self.vc.can_increase_verbosity.set(true);
        false
    }

    // Go: ls/hover.go:631 tryExpandSymbol (closure)
    // tryExpandSymbol checks if a symbol can be expanded at the current verbosity level.
    fn try_expand_symbol(&mut self, symbol: SymbolId, meaning: SymbolFlags) -> bool {
        if self.symbol_was_expanded {
            return true;
        }
        if self.can_expand_symbol(symbol) {
            let expand_vc = VerbosityContext {
                level: self.vc.level - 1,
                max_truncation_length: self.vc.max_truncation_length,
                ..Default::default()
            };
            let expanded =
                self.c
                    .expand_symbol_for_hover_exported(symbol, meaning, Some(&expand_vc));
            if !expanded.is_empty() {
                self.vc.can_increase_verbosity.set(
                    self.vc.can_increase_verbosity.get() || expand_vc.can_increase_verbosity.get(),
                );
                self.vc
                    .truncated
                    .set(self.vc.truncated.get() || expand_vc.truncated.get());
                self.dpw.borrow_mut().write(&expanded);
                self.symbol_was_expanded = true;
                return true;
            }
        }
        false
    }

    // Go: ls/hover.go:652 writeSymbol (closure)
    fn write_symbol(&mut self, symbol: SymbolId) {
        // Recursively write all meanings of alias
        if self.c.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
            && self.visited_aliases.insert(symbol)
        {
            let aliased_symbol = self.c.get_aliased_symbol(symbol);
            if aliased_symbol != self.c.get_unknown_symbol() {
                self.alias_level += 1;
                self.write_symbol(aliased_symbol);
                self.alias_level -= 1;
            }
        }
        let symbol_flags = self.c.sym(symbol).flags;
        let mut flags = if self.meaning == SemanticMeaning::VALUE {
            symbol_flags & (SymbolFlags::VALUE | SymbolFlags::SIGNATURE)
        } else if self.meaning == SemanticMeaning::TYPE {
            symbol_flags & SymbolFlags::TYPE
        } else if self.meaning == SemanticMeaning::NAMESPACE {
            symbol_flags & SymbolFlags::NAMESPACE
        } else {
            symbol_flags
                & (SymbolFlags::VALUE
                    | SymbolFlags::SIGNATURE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE)
        };
        if flags.is_empty() {
            if self.alias_level != 0 || !self.dpw.borrow().string().is_empty() {
                return;
            }
            flags = symbol_flags
                & (SymbolFlags::VALUE
                    | SymbolFlags::SIGNATURE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE);
            if flags.is_empty() {
                return;
            }
        }
        let value_declaration = self.c.sym(symbol).value_declaration;
        if flags.intersects(SymbolFlags::PROPERTY)
            && value_declaration.is_some()
            && is_method_declaration(value_declaration)
        {
            flags = SymbolFlags::METHOD;
        }
        let container = self.container;
        let node = self.node;
        if flags.intersects(SymbolFlags::VARIABLE | SymbolFlags::PROPERTY | SymbolFlags::ACCESSOR) {
            self.write_new_line();
            if !self
                .c
                .sym(symbol)
                .check_flags
                .intersects(CheckFlags::INDEX_SYMBOL)
            {
                if flags.intersects(SymbolFlags::PROPERTY) {
                    self.dpw.borrow_mut().write_punctuation("(");
                    self.dpw.borrow_mut().write("property");
                    self.dpw.borrow_mut().write_punctuation(") ");
                } else if flags.intersects(SymbolFlags::ACCESSOR) {
                    self.dpw.borrow_mut().write_punctuation("(");
                    self.dpw.borrow_mut().write("accessor");
                    self.dpw.borrow_mut().write_punctuation(") ");
                } else {
                    let mut decl = value_declaration;
                    if decl.is_some() {
                        decl = get_root_declaration(decl);
                        if is_parameter_declaration(decl) {
                            self.dpw.borrow_mut().write_punctuation("(");
                            self.dpw.borrow_mut().write("parameter");
                            self.dpw.borrow_mut().write_punctuation(") ");
                        } else if is_var_let(decl) {
                            self.dpw.borrow_mut().write_keyword("let ");
                        } else if is_var_const(decl) {
                            self.dpw.borrow_mut().write_keyword("const ");
                        } else if is_var_using(decl) {
                            self.dpw.borrow_mut().write_keyword("using ");
                        } else if is_var_await_using(decl) {
                            self.dpw.borrow_mut().write_keyword("await ");
                            self.dpw.borrow_mut().write_keyword("using ");
                        } else {
                            self.dpw.borrow_mut().write_keyword("var ");
                        }
                    }
                }
                let symbol_name = self.c.sym(symbol).name.as_str();
                let symbol_parent = self.c.sym(symbol).parent;
                if symbol_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                    && symbol_parent.is_some()
                    && self
                        .c
                        .sym(symbol_parent)
                        .flags
                        .intersects(SymbolFlags::MODULE)
                {
                    self.dpw.borrow_mut().write("exports");
                } else {
                    self.write_symbol_classified(
                        symbol,
                        container,
                        SymbolFlags::NONE,
                        SYMBOL_FORMAT_FLAGS,
                    );
                }
                if self.c.sym(symbol).flags.intersects(SymbolFlags::OPTIONAL) {
                    self.dpw.borrow_mut().write_punctuation("?");
                }
                self.dpw.borrow_mut().write_punctuation(": ");
            }
            let call_node = get_call_or_new_expression(node);
            if call_node.is_some() {
                let mut flags = TYPE_FORMAT_FLAGS
                    | TypeFormatFlags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE
                    | TypeFormatFlags::WRITE_ARROW_STYLE_SIGNATURE;
                if is_call_expression(call_node) {
                    flags |= TypeFormatFlags::WRITE_CALL_STYLE_SIGNATURE;
                }
                let signature = self.c.get_resolved_signature_exported(call_node);
                self.write_signature_classified(signature, container, flags);
            } else {
                let t = self.c.get_type_of_symbol_at_location(symbol, node);
                // If the type is a constrained type parameter, support expansion:
                // Level 0: show just "T", signal canIncreaseVerbosity
                // Level 1+: show "T extends Constraint" with the constraint expanded at level-1
                // PORT: Go also tests `vc != nil`; vc is never nil here.
                let t_symbol = self.c.ty(t).symbol;
                if t_symbol.is_some()
                    && self
                        .c
                        .sym(t_symbol)
                        .flags
                        .intersects(SymbolFlags::TYPE_PARAMETER)
                    && self
                        .c
                        .get_constraint_of_type_parameter_exported(t)
                        .is_some()
                {
                    if self.vc.level > 0 {
                        let expand_vc = VerbosityContext {
                            level: self.vc.level - 1,
                            max_truncation_length: self.vc.max_truncation_length,
                            ..Default::default()
                        };
                        let text = type_parameter_to_string(self.c, t, container, Some(&expand_vc));
                        self.dpw.borrow_mut().write(&text);
                        self.vc.can_increase_verbosity.set(
                            self.vc.can_increase_verbosity.get()
                                || expand_vc.can_increase_verbosity.get(),
                        );
                        self.vc
                            .truncated
                            .set(self.vc.truncated.get() || expand_vc.truncated.get());
                    } else {
                        self.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
                        self.vc.can_increase_verbosity.set(true);
                    }
                } else {
                    self.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
                }
            }
            let declaration = if value_declaration.is_some() {
                value_declaration
            } else {
                self.c
                    .sym(symbol)
                    .declarations
                    .first()
                    .copied()
                    .unwrap_or(Node::NIL)
            };
            self.set_declaration(declaration);
        }
        if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.write_new_line();
            self.dpw.borrow_mut().write_punctuation("(");
            self.dpw.borrow_mut().write("enum member");
            self.dpw.borrow_mut().write_punctuation(") ");
            let t = self.c.get_type_of_symbol_exported(symbol);
            self.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
            if self.c.ty(t).flags.intersects(TypeFlags::LITERAL) {
                self.dpw.borrow_mut().write_operator(" = ");
                let literal = self.c.ty(t).as_literal_type().string();
                self.dpw.borrow_mut().write_literal(&literal);
            }
            self.set_declaration(value_declaration);
        }
        if flags.intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD) {
            let is_method = flags.intersects(SymbolFlags::METHOD);
            let prefix = if is_method { "method" } else { "function " };
            if is_identifier(node)
                && (is_function_like_declaration(node.parent())
                    || is_method_signature_declaration(node.parent()))
                && node.parent().name() == node
                && self.c.sym(symbol).declarations.contains(&node.parent())
            {
                self.set_declaration(node.parent());
                let signatures = vec![
                    self.c
                        .get_signature_from_declaration_exported(node.parent()),
                ];
                self.write_signatures(&signatures, prefix, is_method, symbol);
            } else {
                let signatures =
                    get_signatures_at_location(self.c, symbol, SignatureKind::CALL, node);
                if signatures.len() == 1 {
                    let d = self.c.sig(signatures[0]).declaration;
                    if d.is_some() && !d.flags().intersects(NodeFlags::JS_DOC) {
                        self.set_declaration(d);
                    }
                }
                self.write_signatures(&signatures, prefix, is_method, symbol);
            }
            self.set_declaration(value_declaration);
        }
        if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE) {
            if node.kind() == SyntaxKind::ThisKeyword || is_this_in_type_query(node) {
                self.write_new_line();
                self.dpw.borrow_mut().write_keyword("this");
            } else if node.kind() == SyntaxKind::ConstructorKeyword
                && (is_constructor_declaration(node.parent())
                    || is_construct_signature_declaration(node.parent()))
            {
                self.set_declaration(node.parent());
                let signatures = vec![
                    self.c
                        .get_signature_from_declaration_exported(node.parent()),
                ];
                self.write_signatures(&signatures, "constructor ", false, symbol);
            } else {
                let mut signatures: Vec<SignatureId> = Vec::new();
                if flags.intersects(SymbolFlags::CLASS)
                    && get_call_or_new_expression(node).is_some()
                {
                    signatures =
                        get_signatures_at_location(self.c, symbol, SignatureKind::CONSTRUCT, node);
                }
                if signatures.len() == 1 {
                    let d = self.c.sig(signatures[0]).declaration;
                    if d.is_some() && !d.flags().intersects(NodeFlags::JS_DOC) {
                        self.set_declaration(d);
                    }
                    self.write_signatures(&signatures, "constructor ", false, symbol);
                } else {
                    self.write_new_line();
                    if flags.intersects(SymbolFlags::CLASS) {
                        let class_expression = get_declaration_of_kind(
                            &self.c.symbols,
                            symbol,
                            SyntaxKind::ClassExpression,
                        );
                        if class_expression.is_some() {
                            // Local class expression: show "(local class)" prefix
                            self.dpw.borrow_mut().write_punctuation("(");
                            self.dpw.borrow_mut().write("local class");
                            self.dpw.borrow_mut().write_punctuation(") ");
                        }
                        if !self.try_expand_symbol(symbol, flags) {
                            if class_expression.is_nil() {
                                if self
                                    .c
                                    .sym(symbol)
                                    .declarations
                                    .iter()
                                    .any(|&d| is_class_declaration(d) && has_abstract_modifier(d))
                                {
                                    self.dpw.borrow_mut().write_keyword("abstract ");
                                }
                                self.dpw.borrow_mut().write_keyword("class ");
                            }
                            self.write_symbol_classified(
                                symbol,
                                container,
                                SymbolFlags::NONE,
                                SYMBOL_FORMAT_FLAGS,
                            );
                            let declared_type = self.c.get_declared_type_of_symbol_exported(symbol);
                            let params = self
                                .c
                                .ty(declared_type)
                                .as_interface_type()
                                .local_type_parameters()
                                .to_vec();
                            self.write_type_params(&params);
                        }
                    } else if !self.try_expand_symbol(symbol, flags) {
                        self.dpw.borrow_mut().write_keyword("interface ");
                        self.write_symbol_classified(
                            symbol,
                            container,
                            SymbolFlags::NONE,
                            SYMBOL_FORMAT_FLAGS,
                        );
                        let declared_type = self.c.get_declared_type_of_symbol_exported(symbol);
                        let params = self
                            .c
                            .ty(declared_type)
                            .as_interface_type()
                            .local_type_parameters()
                            .to_vec();
                        self.write_type_params(&params);
                    }
                }
            }
            if flags.intersects(SymbolFlags::CLASS) {
                self.set_declaration(value_declaration);
            } else {
                let declaration = self
                    .c
                    .sym(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| is_interface_declaration(d))
                    .unwrap_or(Node::NIL);
                self.set_declaration(declaration);
            }
        }
        if flags.intersects(SymbolFlags::ENUM) {
            self.write_new_line();
            if !self.try_expand_symbol(symbol, flags) {
                if self
                    .c
                    .sym(symbol)
                    .declarations
                    .iter()
                    .any(|&d| is_enum_declaration(d) && is_enum_const(d))
                {
                    self.dpw.borrow_mut().write_keyword("const ");
                }
                self.dpw.borrow_mut().write_keyword("enum ");
                self.write_symbol_classified(
                    symbol,
                    container,
                    SymbolFlags::NONE,
                    SYMBOL_FORMAT_FLAGS,
                );
            }
            let declaration = self
                .c
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_enum_declaration(d))
                .unwrap_or(Node::NIL);
            self.set_declaration(declaration);
        }
        if flags.intersects(SymbolFlags::MODULE) {
            self.write_new_line();
            if !self.try_expand_symbol(symbol, flags) {
                let is_module = value_declaration.is_some()
                    && (is_source_file(value_declaration) || is_ambient_module(value_declaration));
                self.dpw.borrow_mut().write_keyword(if is_module {
                    "module "
                } else {
                    "namespace "
                });
                self.write_symbol_classified(
                    symbol,
                    container,
                    SymbolFlags::NONE,
                    SYMBOL_FORMAT_FLAGS,
                );
                self.write_module_import_attributes(symbol);
            }
            let declaration = self
                .c
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_module_declaration(d))
                .unwrap_or(Node::NIL);
            self.set_declaration(declaration);
        }
        if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
            self.write_new_line();
            self.dpw.borrow_mut().write_punctuation("(");
            self.dpw.borrow_mut().write("type parameter");
            self.dpw.borrow_mut().write_punctuation(") ");
            if is_identifier(node) && is_type_reference_node(node.parent()) && {
                let t = self.c.get_type_at_location(node.parent());
                self.c.is_distributed_type_parameter(t)
            } {
                self.dpw.borrow_mut().write_punctuation("(");
                self.dpw.borrow_mut().write("distributed");
                self.dpw.borrow_mut().write_punctuation(") ");
            }
            let tp = self.c.get_declared_type_of_symbol_exported(symbol);
            self.write_symbol_classified(symbol, container, SymbolFlags::NONE, SYMBOL_FORMAT_FLAGS);
            let cons = self.c.get_constraint_of_type_parameter_exported(tp);
            if cons.is_some() {
                self.dpw.borrow_mut().write_keyword(" extends ");
                self.write_type_classified(cons, container, TYPE_FORMAT_FLAGS);
            }
            // Show context: "in ClassName<T>" or "in funcName<T>(...)"
            let symbol_parent = self.c.sym(symbol).parent;
            if symbol_parent.is_some() {
                // Class/Interface type parameter
                self.dpw.borrow_mut().write_keyword(" in ");
                self.write_symbol_classified(
                    symbol_parent,
                    container,
                    SymbolFlags::NONE,
                    SYMBOL_FORMAT_FLAGS,
                );
                let parent_type = self.c.get_declared_type_of_symbol_exported(symbol_parent);
                // PORT: Go `AsInterfaceType()` returns nil for other types; the
                // Option form of the accessor keeps that test.
                let parent_params = self
                    .c
                    .ty(parent_type)
                    .data
                    .as_interface_type()
                    .map(|parent_interface| parent_interface.local_type_parameters().to_vec());
                if let Some(parent_params) = parent_params {
                    self.write_type_params(&parent_params);
                }
            } else {
                // Method/function type parameter
                let decl =
                    get_declaration_of_kind(&self.c.symbols, symbol, SyntaxKind::TypeParameter);
                if decl.is_some() && decl.parent().is_some() {
                    let declaration = decl.parent();
                    if is_function_like(declaration) {
                        self.dpw.borrow_mut().write_keyword(" in ");
                        if declaration.kind() == SyntaxKind::ConstructSignature {
                            self.dpw.borrow_mut().write_keyword("new ");
                        } else if declaration.kind() != SyntaxKind::CallSignature
                            && declaration.name().is_some()
                        {
                            self.write_symbol_classified(
                                declaration.symbol(),
                                container,
                                SymbolFlags::NONE,
                                SYMBOL_FORMAT_FLAGS,
                            );
                        }
                        let sig = self.c.get_signature_from_declaration_exported(declaration);
                        if sig.is_some() {
                            self.write_signature_classified(
                                sig,
                                container,
                                TYPE_FORMAT_FLAGS
                                    | TypeFormatFlags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE,
                            );
                        }
                    } else if is_type_alias_declaration(declaration) {
                        self.dpw.borrow_mut().write_keyword(" in ");
                        self.dpw.borrow_mut().write_keyword("type ");
                        self.write_symbol_classified(
                            declaration.symbol(),
                            container,
                            SymbolFlags::NONE,
                            SYMBOL_FORMAT_FLAGS,
                        );
                        let decl_symbol = declaration.symbol();
                        if decl_symbol.is_some() {
                            let ta_params = self.c.get_type_alias_type_parameters(decl_symbol);
                            self.write_type_params(&ta_params);
                        }
                    }
                }
            }
            let declaration = self
                .c
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_type_parameter_declaration(d))
                .unwrap_or(Node::NIL);
            self.set_declaration(declaration);
        }
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            self.write_new_line();
            self.dpw.borrow_mut().write_keyword("type ");
            self.write_symbol_classified(symbol, container, SymbolFlags::NONE, SYMBOL_FORMAT_FLAGS);
            let type_alias_params = self.c.get_type_alias_type_parameters(symbol);
            self.write_type_params(&type_alias_params);
            self.dpw.borrow_mut().write_operator(" = ");
            let type_alias_type =
                if node.parent().is_some() && is_const_type_reference(node.parent()) {
                    self.c.get_type_at_location(node.parent())
                } else {
                    self.c.get_declared_type_of_symbol_exported(symbol)
                };
            self.write_type_classified(
                type_alias_type,
                container,
                TYPE_FORMAT_FLAGS | TypeFormatFlags::IN_TYPE_ALIAS,
            );
            let declaration = self
                .c
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_type_or_js_type_alias_declaration(d))
                .unwrap_or(Node::NIL);
            self.set_declaration(declaration);
        }
        if flags.intersects(SymbolFlags::SIGNATURE) {
            self.write_new_line();
            let t = self.c.get_type_of_symbol_exported(symbol);
            self.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
        }
    }
}

// Go: ls/hover.go:426 getQuickInfoAndDeclarationAtLocation
// getQuickInfoAndDeclarationAtLocation builds classified display parts using displayPartsWriter when vsCapability is true.
// When vsCapability is false, it still builds the plain text string but skips classification runs.
pub fn get_quick_info_and_declaration_at_location(
    c: &mut Checker,
    symbol: SymbolId,
    node: Node,
    vc: Option<&VerbosityContext>,
    vs_capability: bool,
    meaning: SemanticMeaning,
) -> SymbolDisplayInfo {
    let container = get_container_node(node);
    // PORT: a nil vc becomes a fresh context. A clone of the caller's
    // context shares its output cells, as Go shares the pointer.
    let vc = match vc {
        Some(vc) => vc.clone(),
        None => VerbosityContext::default(),
    };
    let dpw = new_display_parts_writer(vs_capability);

    // Source file for printer context
    let mut source_file = Node::NIL;
    if node.is_some() {
        source_file = get_source_file_of_node(node);
    }

    let mut w = QuickInfoWriter {
        c,
        node,
        container,
        vc,
        vs_capability,
        meaning,
        dpw: dpw.clone(),
        source_file,
        visited_aliases: FxHashSet::default(),
        alias_level: 0,
        first_declaration: Node::NIL,
        symbol_was_expanded: false,
        display_emit_context: None,
    };

    if node.kind() == SyntaxKind::ThisKeyword && is_in_expression_context(node)
        || is_this_in_type_query(node)
    {
        w.dpw.borrow_mut().write_keyword("this");
        w.dpw.borrow_mut().write_punctuation(": ");
        let t = w.c.get_type_at_location(node);
        w.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
        return SymbolDisplayInfo {
            display_parts: dpw,
            declaration: Node::NIL,
        };
    }
    if symbol.is_nil() {
        if should_get_type(node) {
            let t = w.c.get_type_at_location(node);
            w.write_type_classified(t, container, TYPE_FORMAT_FLAGS);
        }
        return SymbolDisplayInfo {
            display_parts: dpw,
            declaration: Node::NIL,
        };
    }
    w.write_symbol(symbol);

    SymbolDisplayInfo {
        display_parts: dpw,
        declaration: w.first_declaration,
    }
}

// Go: ls/hover.go:951 typeParameterToString
// typeParameterToString renders a type parameter declaration (e.g., "T extends FooType").
pub fn type_parameter_to_string(
    c: &mut Checker,
    t: TypeId,
    enclosing_declaration: Node,
    vc: Option<&VerbosityContext>,
) -> String {
    c.type_parameter_to_string_ex(t, enclosing_declaration, vc)
}

// Go: ls/hover.go:955 getNodeForQuickInfo
pub fn get_node_for_quick_info(node: Node) -> Node {
    if node.parent().is_nil() {
        return node;
    }
    if is_new_expression(node.parent()) && node.pos() == node.parent().pos() {
        return node.parent().expression();
    }
    if is_named_tuple_member(node.parent()) && node.pos() == node.parent().pos() {
        return node.parent();
    }
    if is_import_meta(node.parent()) && node.parent().name() == node {
        return node.parent();
    }
    if is_jsx_namespaced_name(node.parent()) {
        return node.parent();
    }
    node
}

// Go: ls/hover.go:974 getSymbolAtLocationForQuickInfo
pub fn get_symbol_at_location_for_quick_info(c: &mut Checker, node: Node) -> SymbolId {
    let object_element = get_containing_object_literal_element(node);
    if object_element.is_some() {
        let contextual_type =
            c.get_contextual_type_exported(object_element.parent(), ContextFlags::NONE);
        if contextual_type.is_some() {
            let properties = c.get_property_symbols_from_contextual_type(
                object_element,
                contextual_type,
                false, /*unionSymbolOk*/
            );
            if properties.len() == 1 {
                return properties[0];
            }
        }
    }
    c.get_symbol_at_location_exported(node)
}

// Go: ls/hover.go:985 getSignaturesAtLocation
pub fn get_signatures_at_location(
    c: &mut Checker,
    symbol: SymbolId,
    kind: SignatureKind,
    node: Node,
) -> Vec<SignatureId> {
    let type_of_symbol = c.get_type_of_symbol_exported(symbol);
    let t = c.remove_missing_or_undefined_type_exported(type_of_symbol);
    let signatures = c.get_signatures_of_type_exported(t, kind);
    if signatures.len() > 1
        || signatures.len() == 1 && !c.sig(signatures[0]).type_parameters.is_empty()
    {
        let call_node = get_call_or_new_expression(node);
        if call_node.is_some() {
            // We have a call or new expression, return the resolved signature
            return vec![c.get_resolved_signature_exported(call_node)];
        }
    }
    signatures
}

// Go: ls/hover.go:996 getCallOrNewExpression
pub fn get_call_or_new_expression(node: Node) -> Node {
    if is_source_file(node) {
        return Node::NIL;
    }
    let mut node = node;
    if is_property_access_expression(node.parent()) && node.parent().name() == node {
        node = node.parent();
    }
    if (is_call_expression(node.parent()) || is_new_expression(node.parent()))
        && node.parent().expression() == node
    {
        return node.parent();
    }
    Node::NIL
}

// Go: ls/hover.go:1009 containsTypedefTag
pub fn contains_typedef_tag(jsdoc: Node) -> bool {
    if jsdoc.kind() == SyntaxKind::JsDoc {
        let tags = jsdoc.tags();
        if tags.is_some() {
            for tag in tags.nodes() {
                if tag.kind() == SyntaxKind::JsDocTypedefTag
                    || tag.kind() == SyntaxKind::JsDocCallbackTag
                {
                    return true;
                }
            }
        }
    }
    false
}

// Go: ls/hover.go:1022 writeCode
pub fn write_code(b: &mut String, lang: &str, code: &str) {
    if code.is_empty() {
        return;
    }
    let mut ticks = 3;
    while code.contains(&"`".repeat(ticks)) {
        ticks += 1;
    }
    for _ in 0..ticks {
        b.push('`');
    }
    b.push_str(lang);
    b.push('\n');
    b.push_str(code);
    b.push('\n');
    for _ in 0..ticks {
        b.push('`');
    }
    b.push('\n');
}

// Go: ls/hover.go:1043 writeComments
pub fn write_comments(
    get_mapped_location: DocumentationLocationMapper<'_>,
    b: &mut String,
    c: &mut Checker,
    comments: &[Node],
    is_markdown: bool,
) {
    for &comment in comments {
        match comment.kind() {
            SyntaxKind::JsDocText => {
                b.push_str(comment.text());
            }
            SyntaxKind::JsDocLink | SyntaxKind::JsDocLinkPlain => {
                write_js_doc_link(
                    get_mapped_location,
                    b,
                    c,
                    comment,
                    false, /*quote*/
                    is_markdown,
                );
            }
            SyntaxKind::JsDocLinkCode => {
                write_js_doc_link(
                    get_mapped_location,
                    b,
                    c,
                    comment,
                    true, /*quote*/
                    is_markdown,
                );
            }
            _ => {}
        }
    }
}

// Go: ls/hover.go:1056 writeJSDocLink
pub fn write_js_doc_link(
    get_mapped_location: DocumentationLocationMapper<'_>,
    b: &mut String,
    c: &mut Checker,
    link: Node,
    quote: bool,
    is_markdown: bool,
) {
    let name = link.name();
    let text = link.text().trim_matches(' ');
    if name.is_nil() {
        write_quoted_string(b, text, quote && is_markdown);
        return;
    }
    if is_identifier(name)
        && (name.text() == "http" || name.text() == "https")
        && text.starts_with("://")
    {
        let mut link_text = name.text().to_string() + text;
        let mut link_uri = link_text.clone();
        if let Some(comment_pos) = link_text.find(|ch: char| ch == ' ' || ch == '|') {
            link_uri = link_text[..comment_pos].to_string();
            link_text = trim_comment_prefix(&link_text[comment_pos..]).to_string();
            if link_text.is_empty() {
                link_text = link_uri.clone();
            }
        }
        if is_markdown {
            write_markdown_link(b, &link_text, &link_uri, quote);
        } else {
            write_quoted_string(b, &link_text, false);
            if link_text != link_uri {
                b.push_str(" (");
                b.push_str(&link_uri);
                b.push(')');
            }
        }
        return;
    }
    write_name_link(get_mapped_location, b, c, name, text, quote, is_markdown);
}

// Go: ls/hover.go:1088 writeNameLink
pub fn write_name_link(
    get_mapped_location: DocumentationLocationMapper<'_>,
    b: &mut String,
    c: &mut Checker,
    name: Node,
    text: &str,
    quote: bool,
    is_markdown: bool,
) {
    let declarations = get_declarations_from_location(c, name);
    if !declarations.is_empty() {
        let declaration = declarations[0];
        let file = get_source_file_of_node(declaration);
        let name_of_declaration = get_name_of_declaration(declaration);
        let node = if name_of_declaration.is_some() {
            name_of_declaration
        } else {
            declaration
        };
        let (loc, fidelity) = get_mapped_location(file, create_range_from_node(node, file));
        let prefix_len = if text.starts_with("()") { 2 } else { 0 };
        let mut link_text = trim_comment_prefix(&text[prefix_len..]).to_string();
        if link_text.is_empty() {
            link_text = get_entity_name_string(name) + &text[..prefix_len];
        }
        if is_markdown && fidelity.is_single_segment() {
            let link_uri = format!(
                "{}#{},{}-{},{}",
                loc.uri,
                loc.range.start.line + 1,
                loc.range.start.character + 1,
                loc.range.end.line + 1,
                loc.range.end.character + 1
            );
            write_markdown_link(b, &link_text, &link_uri, quote);
        } else {
            write_quoted_string(b, &link_text, false);
        }
        return;
    }
    let separator = if !text.is_empty() { " " } else { "" };
    write_quoted_string(
        b,
        &(get_entity_name_string(name) + separator + text),
        quote && is_markdown,
    );
}

// Go: ls/hover.go:1111 trimCommentPrefix
pub fn trim_comment_prefix(text: &str) -> &str {
    let text = text.trim_start_matches(' ');
    let text = text.strip_prefix('|').unwrap_or(text);
    text.trim_start_matches(' ')
}

// Go: ls/hover.go:1115 writeMarkdownLink
pub fn write_markdown_link(b: &mut String, text: &str, uri: &str, quote: bool) {
    b.push('[');
    write_quoted_string(b, text, quote);
    b.push_str("](");
    b.push_str(uri);
    b.push(')');
}

// Go: ls/hover.go:1123 writeOptionalEntityName
pub fn write_optional_entity_name(b: &mut String, name: Node) {
    if name.is_some() {
        b.push(' ');
        write_quoted_string(b, &get_entity_name_string(name), true /*quote*/);
    }
}

// Go: ls/hover.go:1130 writeQuotedString
pub fn write_quoted_string(b: &mut String, str: &str, quote: bool) {
    if quote && !str.contains('`') {
        b.push('`');
        b.push_str(str);
        b.push('`');
    } else {
        b.push_str(str);
    }
}

// Go: ls/hover.go:1140 getEntityNameString
pub fn get_entity_name_string(name: Node) -> String {
    let mut b = String::new();
    write_entity_name_parts(&mut b, name);
    b
}

// Go: ls/hover.go:1146 writeEntityNameParts
pub fn write_entity_name_parts(b: &mut String, node: Node) {
    match node.kind() {
        SyntaxKind::Identifier => {
            b.push_str(node.text());
        }
        SyntaxKind::QualifiedName => {
            write_entity_name_parts(b, node.left());
            b.push('.');
            write_entity_name_parts(b, node.right());
        }
        SyntaxKind::PropertyAccessExpression => {
            write_entity_name_parts(b, node.expression());
            b.push('.');
            write_entity_name_parts(b, node.name());
        }
        SyntaxKind::ParenthesizedExpression | SyntaxKind::ExpressionWithTypeArguments => {
            write_entity_name_parts(b, node.expression());
        }
        SyntaxKind::JsDocNameReference => {
            write_entity_name_parts(b, node.name());
        }
        _ => {}
    }
}
