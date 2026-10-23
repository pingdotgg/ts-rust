//! Port of Go `ls/definition.go`.
//!
//! PORT: Go `*compiler.Program` is `&compiler::NewProgram`. A Go
//! checker lease `c, done := program.GetTypeCheckerForFile(ctx, file)` is
//! `ls_program::get_type_checker_for_file`, with `done` kept alive to the end
//! of the scope (Go `defer done()`).

use crate::ls::prelude::*;

use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/definition.go:19 ProvideDefinition
    pub fn provide_definition(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> Result<lsproto::DefinitionResponse, GoError> {
        if self.user_preferences().prefer_go_to_source_definition {
            return self.provide_source_definition(ctx, document_uri, position);
        }
        self.provide_definition_worker(ctx, document_uri, position)
    }

    // Go: ls/definition.go:30 provideDefinitionWorker
    pub fn provide_definition_worker(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> Result<lsproto::DefinitionResponse, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        let client_supports_link = caps.text_document.definition.link_support;

        let (program, file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            position,
            Feature::DEFINITION,
        );
        let mut results = Vec::with_capacity(positions.len());
        for mapped in &positions {
            if mapped.fidelity.is_single_segment() {
                results.push(self.provide_definition_at_position(
                    ctx,
                    program,
                    mapped.script,
                    mapped.position,
                    client_supports_link,
                ));
            }
        }
        Ok(combine_definition_responses(results, client_supports_link))
    }

    // Go: ls/definition.go:49 provideDefinitionAtPosition
    // PORT: Go `core.TextPos` is `i32`.
    pub fn provide_definition_at_position(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        file: Node,
        text_pos: i32,
        client_supports_link: bool,
    ) -> lsproto::DefinitionResponse {
        let pos = text_pos;
        let node = astnav::get_touching_property_name(file, pos);
        let reference = get_reference_at_position(file, pos, program);

        if node.kind() == SyntaxKind::SourceFile {
            return lsproto::LocationOrLocationsOrDefinitionLinksOrNull::default();
        }

        let (origin_selection_range, _) = self.create_lsp_range_from_node(node, file);
        if let Some(reference) = &reference {
            if reference.file.is_some() {
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &[],
                    Some(reference),
                    Feature::DEFINITION,
                );
            }
        }

        // Go: `defer done()`; `_done` releases the lease at the end of the scope.
        let (checker, _done) = ls_program::get_type_checker_for_file(program, ctx, file);
        let c = &mut *checker.borrow_mut();

        if node.kind() == SyntaxKind::OverrideKeyword {
            let sym = get_symbol_for_overridden_member(c, node);
            if sym.is_some() {
                let declarations = c.sym(sym).declarations.to_vec();
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &declarations,
                    None, /*reference*/
                    Feature::DEFINITION,
                );
            }
        }

        // PORT: Go calls `ast.IsJumpStatementTarget`; the ls prelude picks the
        // ls version of this name, so the ast one is called by path.
        if crate::ast::is_jump_statement_target(node) {
            let label = get_target_label(node.parent(), node.text());
            if label.is_some() {
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &[label],
                    None, /*reference*/
                    Feature::DEFINITION,
                );
            }
        }

        if node.kind() == SyntaxKind::CaseKeyword
            || node.kind() == SyntaxKind::DefaultKeyword && is_default_clause(node.parent())
        {
            let stmt = find_ancestor(node.parent(), is_switch_statement);
            if stmt.is_some() {
                let file = get_source_file_of_node(stmt);
                return self.create_location_from_file_and_range(
                    file,
                    get_range_of_token_at_position(file, stmt.pos()),
                    Feature::DEFINITION,
                );
            }
        }

        if node.kind() == SyntaxKind::ReturnKeyword
            || node.kind() == SyntaxKind::YieldKeyword
            || node.kind() == SyntaxKind::AwaitKeyword
        {
            let fn_ = find_ancestor(node, is_function_like_declaration);
            if fn_.is_some() {
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &[fn_],
                    None, /*reference*/
                    Feature::DEFINITION,
                );
            }
        }

        let mut declarations = get_declarations_from_location(c, node);
        let called_declaration = try_get_signature_declaration(c, node);
        if called_declaration.is_some()
            && !(is_jsx_opening_like_element(node.parent())
                && is_jsx_constructor_like(called_declaration))
        {
            let symbol = c.get_symbol_at_location_exported(get_declaration_name_for_keyword(node));
            let matches = symbol.is_some() && {
                let root_symbols = c.get_root_symbols(symbol);
                root_symbols.iter().any(|&root_symbol| {
                    symbol_matches_signature(&c.symbols, root_symbol, called_declaration)
                })
            };
            if matches {
                if !is_constructor_declaration(called_declaration) {
                    declarations = Vec::new();
                } else {
                    declarations = declarations
                        .into_iter()
                        .filter(|&node| {
                            node != called_declaration
                                && (is_class_declaration(node) || is_class_expression(node))
                        })
                        .collect();
                }
            } else {
                declarations = declarations
                    .into_iter()
                    .filter(|&node| node != called_declaration)
                    .collect();
            }
            declarations.push(called_declaration);
        }
        self.create_definition_locations(
            origin_selection_range,
            client_supports_link,
            &declarations,
            reference.as_ref(),
            Feature::DEFINITION,
        )
    }

    // Go: ls/definition.go:113 ProvideTypeDefinition
    pub fn provide_type_definition(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        position: lsproto::Position,
    ) -> Result<lsproto::TypeDefinitionResponse, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        let client_supports_link = caps.text_document.type_definition.link_support;

        let (program, file) = self.get_program_and_file(document_uri);
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            file,
            position,
            Feature::TYPE_DEFINITION,
        );
        let mut results = Vec::with_capacity(positions.len());
        for mapped in &positions {
            if mapped.fidelity.is_single_segment() {
                results.push(self.provide_type_definition_at_position(
                    ctx,
                    program,
                    mapped.script,
                    mapped.position,
                    client_supports_link,
                ));
            }
        }
        Ok(combine_definition_responses(results, client_supports_link))
    }

    // Go: ls/definition.go:132 provideTypeDefinitionAtPosition
    // PORT: Go `core.TextPos` is `i32`.
    pub fn provide_type_definition_at_position(
        &self,
        ctx: &Context,
        program: &compiler::NewProgram,
        file: Node,
        text_pos: i32,
        client_supports_link: bool,
    ) -> lsproto::TypeDefinitionResponse {
        let pos = text_pos;
        let mut node = astnav::get_touching_property_name(file, pos);
        if node.kind() == SyntaxKind::SourceFile {
            return lsproto::LocationOrLocationsOrDefinitionLinksOrNull::default();
        }
        let (origin_selection_range, _) = self.create_lsp_range_from_node(node, file);

        let (checker, _done) = ls_program::get_type_checker_for_file(program, ctx, file);
        let c = &mut *checker.borrow_mut();

        node = get_declaration_name_for_keyword(node);

        let symbol = c.get_symbol_at_location_exported(node);
        if symbol.is_some() {
            let symbol_type = get_type_of_symbol_at_location(c, symbol, node);
            let mut declarations = get_declarations_from_type(c, symbol_type);
            let type_argument = c.get_first_type_argument_from_known_type(symbol_type);
            if type_argument.is_some() {
                let mut concatenated = get_declarations_from_type(c, type_argument);
                concatenated.extend(declarations);
                declarations = concatenated;
            }
            if !declarations.is_empty() {
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &declarations,
                    None, /*reference*/
                    Feature::TYPE_DEFINITION,
                );
            }
            let flags = c.sym_flags(symbol);
            if !flags.intersects(SymbolFlags::VALUE) && flags.intersects(SymbolFlags::TYPE) {
                let declarations = c.sym(symbol).declarations.to_vec();
                return self.create_definition_locations(
                    origin_selection_range,
                    client_supports_link,
                    &declarations,
                    None, /*reference*/
                    Feature::TYPE_DEFINITION,
                );
            }
        }

        lsproto::LocationOrLocationsOrDefinitionLinksOrNull::default()
    }
}

// Go: ls/definition.go:162 combineDefinitionResponses
// PORT: Go takes the slice; here the results are moved in. Go appends the
// same `*LocationLink` it read; here the link is moved. Go `seen` is a
// `collections.Set[lsproto.Location]`. Go returns a pointer to a possibly
// nil slice, which marshals as `[]`; here `Some` of a possibly empty `Vec`.
#[must_use]
pub fn combine_definition_responses(
    results: Vec<lsproto::DefinitionResponse>,
    links: bool,
) -> lsproto::DefinitionResponse {
    let mut locations: Vec<lsproto::Location> = Vec::new();
    let mut definition_links: Vec<lsproto::LocationLink> = Vec::new();
    let mut seen: FxHashSet<lsproto::Location> = FxHashSet::default();
    for result in results {
        if let Some(result_links) = result.definition_links {
            for link in result_links {
                let location = lsproto::Location {
                    uri: link.target_uri.clone(),
                    range: link.target_selection_range,
                };
                if seen.insert(location.clone()) {
                    definition_links.push(link);
                    locations.push(location);
                }
            }
        }
        if let Some(location) = result.location {
            if seen.insert(location.clone()) {
                definition_links.push(lsproto::LocationLink {
                    origin_selection_range: None,
                    target_uri: location.uri.clone(),
                    target_range: location.range,
                    target_selection_range: location.range,
                });
                locations.push(location);
            }
        }
        if let Some(result_locations) = result.locations {
            for location in result_locations {
                if seen.insert(location.clone()) {
                    definition_links.push(lsproto::LocationLink {
                        origin_selection_range: None,
                        target_uri: location.uri.clone(),
                        target_range: location.range,
                        target_selection_range: location.range,
                    });
                    locations.push(location);
                }
            }
        }
    }
    if links {
        return lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
            definition_links: Some(definition_links),
            ..Default::default()
        };
    }
    lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
        locations: Some(locations),
        ..Default::default()
    }
}

// Go: ls/definition.go:195 getDeclarationNameForKeyword
pub fn get_declaration_name_for_keyword(node: Node) -> Node {
    if (node.kind() as u16) >= (SyntaxKind::FIRST_KEYWORD as u16)
        && (node.kind() as u16) <= (SyntaxKind::LAST_KEYWORD as u16)
    {
        if is_variable_declaration_list(node.parent()) {
            if let Some(decl) = node.parent().declarations().nodes().first() {
                if decl.name().is_some() {
                    return decl.name();
                }
            }
        } else if is_declaration_node(node.parent())
            && node.parent().name().is_some()
            && node.pos() < node.parent().name().pos()
        {
            return node.parent().name();
        }
    }
    node
}

// Go: ls/definition.go:208 fileRange
// PORT: Go `file *ast.SourceFile` compares by pointer; here the file root
// `Node`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FileRange {
    pub file: Node,
    pub file_range: TextRange,
}

impl LanguageService {
    // Go: ls/definition.go:213 createDefinitionLocations
    // PORT: Go `reference *refInfo` is `Option<&RefInfo>`.
    pub fn create_definition_locations(
        &self,
        origin_selection_range: lsproto::Range,
        client_supports_link: bool,
        declarations: &[Node],
        reference: Option<&RefInfo>,
        feature: Feature,
    ) -> lsproto::DefinitionResponse {
        let mut locations: Vec<lsproto::LocationLink> = Vec::new();
        let mut location_ranges: FxHashSet<FileRange> = FxHashSet::default();

        if let Some(reference) = reference {
            let target_range = lsproto::Range {
                start: lsproto::Position {
                    line: 0,
                    character: 0,
                },
                end: lsproto::Position {
                    line: 0,
                    character: 0,
                },
            };
            locations.push(lsproto::LocationLink {
                origin_selection_range: Some(origin_selection_range),
                target_uri: lsconv::file_name_to_document_uri(&reference.file_name),
                target_range,
                target_selection_range: target_range,
            });
        }

        for &decl in declarations {
            let file = get_source_file_of_node(decl);
            let name = {
                let name = get_name_of_declaration(decl);
                if name.is_some() { name } else { decl }
            };
            let name_range = if name.kind() == SyntaxKind::EmptyStatement {
                TextRange::new(name.pos(), name.pos())
            } else {
                create_range_from_node(name, file)
            };
            if location_ranges.insert(FileRange {
                file,
                file_range: name_range,
            }) {
                let context_node = {
                    let context_node = get_context_node(decl);
                    if context_node.is_some() {
                        context_node
                    } else {
                        decl
                    }
                };
                let mut context_range =
                    to_context_range(Some(name_range), file, context_node).unwrap_or(name_range);
                if !name_range.contained_by(context_range) {
                    context_range = TextRange::new(
                        name_range.pos().min(context_range.pos()),
                        name_range.end().max(context_range.end()),
                    );
                }
                let (target_selection_loc, selection_fidelity) =
                    self.source_file_range_to_lsp_location_for_feature(file, name_range, feature);
                if !selection_fidelity.is_single_segment() {
                    continue;
                }
                let (mut target_loc, context_fidelity) =
                    self.source_file_range_to_lsp_location(file, context_range);
                if context_fidelity.is_none()
                    || target_loc.uri != target_selection_loc.uri
                    || !lsp_range_contains(target_loc.range, target_selection_loc.range)
                {
                    target_loc = target_selection_loc.clone();
                }
                locations.push(lsproto::LocationLink {
                    origin_selection_range: Some(origin_selection_range),
                    target_selection_range: target_selection_loc.range,
                    target_uri: target_loc.uri,
                    target_range: target_loc.range,
                });
            }
        }

        if client_supports_link {
            return lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
                definition_links: Some(locations),
                ..Default::default()
            };
        }
        create_locations_from_links(&locations)
    }
}

// Go: ls/definition.go:281 lspRangeContains
#[must_use]
pub fn lsp_range_contains(outer: lsproto::Range, inner: lsproto::Range) -> bool {
    lsproto::compare_positions(outer.start, inner.start) <= 0
        && lsproto::compare_positions(inner.end, outer.end) <= 0
}

// Go: ls/definition.go:286 createLocationsFromLinks
pub fn create_locations_from_links(links: &[lsproto::LocationLink]) -> lsproto::DefinitionResponse {
    let locations: Vec<lsproto::Location> = links
        .iter()
        .map(|link| lsproto::Location {
            uri: link.target_uri.clone(),
            range: link.target_selection_range,
        })
        .collect();
    lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
        locations: Some(locations),
        ..Default::default()
    }
}

impl LanguageService {
    // Go: ls/definition.go:296 createLocationFromFileAndRange
    pub fn create_location_from_file_and_range(
        &self,
        file: Node,
        text_range: TextRange,
        feature: Feature,
    ) -> lsproto::DefinitionResponse {
        let (mut mapped_location, fidelity) =
            self.source_file_range_to_lsp_location_for_feature(file, text_range, feature);
        if fidelity.is_none() {
            mapped_location.range = lsproto::Range::default();
        }
        lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
            location: Some(mapped_location),
            ..Default::default()
        }
    }
}

// Go: ls/definition.go:306 getDeclarationsFromLocation
pub fn get_declarations_from_location(c: &mut Checker, node: Node) -> Vec<Node> {
    if is_identifier(node) && is_shorthand_property_assignment(node.parent()) {
        // Because name in short-hand property assignment has two different meanings: property name and property value,
        // using go-to-definition at such position should go to the variable declaration of the property value rather than
        // go to the declaration of the property name (in this case stay at the same position). However, if go-to-definition
        // is performed at the location of property access, we would like to go to definition of the property in the short-hand
        // assignment. This case and others are handled by the following code.
        // and the contextual type's property declarations
        let shorthand_symbol = c.get_resolved_symbol_exported(node);
        let mut declarations: Vec<Node> = Vec::new();
        if shorthand_symbol.is_some() {
            declarations = c.sym(shorthand_symbol).declarations.to_vec();
        }
        let contextual_declarations = get_declarations_from_object_literal_element(c, node);
        declarations.extend(contextual_declarations);
        return declarations;
    }

    if is_property_name(node)
        && is_binding_element(node.parent())
        && is_object_binding_pattern(node.parent().parent())
    {
        // If the node is the name of a BindingElement within an ObjectBindingPattern instead of just returning the
        // declaration of the symbol (which is itself), we should try to get to the original type of the
        // ObjectBindingPattern and return the property declaration for the referenced property.
        // For example:
        //      import('./foo').then(({ bar }) => undefined); => should navigate to the declaration in file "./foo"
        //
        //      function bar<T>(onfulfilled: (value: T) => void) { }
        //      interface Test { prop1: number }
        //      bar<Test>(({ prop1 }) => {});  => should navigate to prop1 in Test
        let binding_el = node.parent();
        let property_name_or_name = {
            let property_name = binding_el.property_name();
            if property_name.is_some() {
                property_name
            } else {
                node.parent().name()
            }
        };
        if binding_el.dot_dot_dot_token().is_nil() && node == property_name_or_name {
            let (name, ok) = try_get_text_of_property_name(node);
            if ok {
                let t = c.get_type_at_location(node.parent().parent());
                let mut types: Vec<TypeId> = vec![t];
                if c.ty(t).is_union() {
                    types = c.ty(t).types().to_vec();
                }
                let mut result: Vec<Node> = Vec::new();
                for union_type in types {
                    let prop = c.get_property_of_type_exported(union_type, &name);
                    if prop.is_some() {
                        result.extend(c.sym(prop).declarations.iter().copied());
                    }
                }
                return result;
            }
        }
    }

    let node = get_declaration_name_for_keyword(node);
    let mut symbol = c.get_symbol_at_location_exported(node);
    if symbol.is_some() {
        let flags = c.sym_flags(symbol);
        if flags.intersects(SymbolFlags::CLASS)
            && !flags.intersects(SymbolFlags::FUNCTION | SymbolFlags::VARIABLE)
            && node.kind() == SyntaxKind::ConstructorKeyword
        {
            let constructor = c
                .symbols
                .get(c.sym(symbol).members, INTERNAL_SYMBOL_NAME_CONSTRUCTOR);
            if constructor.is_some() {
                symbol = constructor;
            }
        }
        if c.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
            let (resolved, ok) = c.resolve_alias_exported(symbol);
            if ok {
                symbol = resolved;
            }
        }
        let object_literal_element_declarations =
            get_declarations_from_object_literal_element(c, node);
        if !object_literal_element_declarations.is_empty() {
            return object_literal_element_declarations;
        }
        if !c.sym(symbol).declarations.is_empty() {
            return c.sym(symbol).declarations.to_vec();
        }
    }
    let index_infos = c.get_index_signatures_at_location_exported(node);
    if !index_infos.is_empty() {
        return index_infos;
    }
    Vec::new()
}

// Go: ls/definition.go:380 getDeclarationsFromObjectLiteralElement
// getDeclarationsFromObjectLiteralElement returns declarations from the contextual type
// of an object literal element, if available.
pub fn get_declarations_from_object_literal_element(c: &mut Checker, node: Node) -> Vec<Node> {
    let element = get_containing_object_literal_element(node);
    if element.is_nil() {
        return Vec::new();
    }

    let contextual_type = c.get_contextual_type_exported(element.parent(), ContextFlags::NONE);
    if contextual_type.is_nil() {
        return Vec::new();
    }

    let mut properties = c.get_property_symbols_from_contextual_type(
        element,
        contextual_type,
        false, /*unionSymbolOk*/
    );
    if properties.iter().any(|&p| {
        let value_declaration = c.sym(p).value_declaration;
        value_declaration.is_some()
            && is_object_literal_expression(value_declaration.parent())
            && is_object_literal_element(value_declaration)
            && value_declaration.name() == node
    }) {
        let without_node_inferences_type =
            c.get_contextual_type_exported(element.parent(), ContextFlags::IGNORE_NODE_INFERENCES);
        if without_node_inferences_type.is_some() {
            let without_node_inferences_properties = c.get_property_symbols_from_contextual_type(
                element,
                without_node_inferences_type,
                false, /*unionSymbolOk*/
            );
            if !without_node_inferences_properties.is_empty() {
                properties = without_node_inferences_properties;
            }
        }
    }

    let mut result: Vec<Node> = Vec::new();
    for prop in properties {
        result.extend(c.sym(prop).declarations.iter().copied());
    }
    result
}

// Go: ls/definition.go:410 getAncestorCallLikeExpression
// Returns a CallLikeExpression where `node` is the target being invoked.
pub fn get_ancestor_call_like_expression(node: Node) -> Node {
    // PORT: Go calls `ast.IsRightSideOfPropertyAccess`; the ls prelude picks
    // the ls version of this name, so the ast one is called by path.
    let target = find_ancestor(node, |n| !crate::ast::is_right_side_of_property_access(n));
    let call_like = target.parent();
    if call_like.is_some()
        && is_call_like_expression(call_like)
        && get_invoked_expression(call_like) == target
    {
        return call_like;
    }
    Node::NIL
}

// Go: ls/definition.go:421 tryGetSignatureDeclaration
pub fn try_get_signature_declaration(type_checker: &mut Checker, node: Node) -> Node {
    let mut signature = SignatureId::NIL;
    let call_like = get_ancestor_call_like_expression(node);
    if call_like.is_some() {
        signature = type_checker.get_resolved_signature_exported(call_like);
    }
    // Don't go to a function type, go to the value having that type.
    if signature.is_some() && type_checker.sig(signature).declaration().is_some() {
        let declaration = type_checker.sig(signature).declaration();
        if is_function_like(declaration) && !is_function_type_node(declaration) {
            return declaration;
        }
    }
    Node::NIL
}

// Go: ls/definition.go:438 isJsxConstructorLike
pub fn is_jsx_constructor_like(node: Node) -> bool {
    is_constructor_declaration(node)
        || is_constructor_type_node(node)
        || is_call_signature_declaration(node)
        || is_construct_signature_declaration(node)
}

// Go: ls/definition.go:450 symbolMatchesSignature
// PORT: Go reads symbol fields without a checker; the symbol arena is the
// first parameter, as for ast helpers that take a symbol.
pub fn symbol_matches_signature(
    symbols: &SymbolArena,
    symbol: SymbolId,
    called_declaration: Node,
) -> bool {
    if symbol.is_nil() || called_declaration.is_nil() {
        return false;
    }
    let called_symbol = called_declaration.symbol();
    if symbol == called_symbol
        || called_symbol.is_some() && symbol == symbols.sym(called_symbol).parent
    {
        return true;
    }
    let parent = called_declaration.parent();
    parent.is_some()
        && (is_assignment_expression(parent, false /*excludeCompoundAssignment*/)
            || !is_call_like_expression(parent)
                && can_have_symbol(parent)
                && symbol == parent.symbol())
}

// Go: ls/definition.go:463 getSymbolForOverriddenMember
pub fn get_symbol_for_overridden_member(type_checker: &mut Checker, node: Node) -> SymbolId {
    let class_element = find_ancestor(node, is_class_element);
    if class_element.is_nil() || class_element.name().is_nil() {
        return SymbolId::NIL;
    }
    let base_declaration = find_ancestor(class_element, is_class_like);
    if base_declaration.is_nil() {
        return SymbolId::NIL;
    }
    let base_type_node = get_class_extends_heritage_element(base_declaration);
    if base_type_node.is_nil() {
        return SymbolId::NIL;
    }
    let expression = skip_parentheses(base_type_node.expression());
    let base = if is_class_expression(expression) {
        expression.symbol()
    } else {
        type_checker.get_symbol_at_location_exported(expression)
    };
    if base.is_nil() {
        return SymbolId::NIL;
    }
    let name = get_text_of_property_name(class_element.name());
    if has_static_modifier(class_element) {
        let t = type_checker.get_type_of_symbol_exported(base);
        return type_checker.get_property_of_type_exported(t, &name);
    }
    let t = type_checker.get_declared_type_of_symbol_exported(base);
    type_checker.get_property_of_type_exported(t, &name)
}

// Go: ls/definition.go:493 getTypeOfSymbolAtLocation
// PORT: a free function; `Checker::get_type_of_symbol_at_location` is the
// checker method it calls (a method and a free fn do not clash).
pub fn get_type_of_symbol_at_location(c: &mut Checker, symbol: SymbolId, node: Node) -> TypeId {
    let t = c.get_type_of_symbol_at_location(symbol, node);
    // If the type is just a function's inferred type, go-to-type should go to the return type instead since
    // go-to-definition takes you to the function anyway.
    let t_symbol = c.ty(t).symbol;
    let symbol_value_declaration = c.sym(symbol).value_declaration;
    if t_symbol == symbol
        || t_symbol.is_some()
            && symbol_value_declaration.is_some()
            && is_variable_declaration(symbol_value_declaration)
            && symbol_value_declaration.initializer() == c.sym(t_symbol).value_declaration
    {
        let sigs = c.get_call_signatures(t);
        if sigs.len() == 1 {
            return c.get_return_type_of_signature_exported(sigs[0]);
        }
    }
    t
}

// Go: ls/definition.go:506 getDeclarationsFromType
// PORT: Go reads type and symbol fields without a checker; the checker is
// the first parameter.
pub fn get_declarations_from_type(c: &Checker, t: TypeId) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();
    for t in c.ty(t).distributed() {
        let symbol = c.ty(t).symbol;
        if symbol.is_some() {
            for &decl in c.sym(symbol).declarations.iter() {
                // Go core.AppendIfUnique
                if !result.contains(&decl) {
                    result.push(decl);
                }
            }
        }
    }
    result
}
