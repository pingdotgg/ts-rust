use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:32252 getIndexSignaturesAtLocation
    pub fn get_index_signatures_at_location(&mut self, node: Node) -> Vec<Node> {
        let mut signatures: Vec<Node> = Vec::new();
        if is_identifier(node)
            && is_property_access_expression(node.parent())
            && node.parent().name() == node
        {
            let key_type = self.get_literal_type_from_property_name(node);
            let object_type = self.get_type_of_expression(node.parent().expression());
            let distributed = self.ty(object_type).distributed();
            for t in distributed {
                for info in self.get_applicable_index_infos(t, key_type) {
                    let declaration = self.index_info(info).declaration;
                    if declaration.is_some() {
                        // Go: core.AppendIfUnique
                        if !signatures.contains(&declaration) {
                            signatures.push(declaration);
                        }
                    }
                }
            }
        }
        signatures
    }

    // Go: checker/checker.go:32268 getSymbolOfNameOrPropertyAccessExpression
    pub fn get_symbol_of_name_or_property_access_expression(&mut self, mut name: Node) -> SymbolId {
        if is_declaration_name(name) {
            return self.get_symbol_of_node(name.parent());
        }
        if name.parent().kind() == SyntaxKind::ExportAssignment && is_entity_name_expression(name) {
            // Even an entity name expression that doesn't resolve as an entityname may still typecheck as a property access expression
            let success = self.resolve_entity_name(
                name,
                /*all meanings*/
                SymbolFlags::VALUE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE
                    | SymbolFlags::ALIAS,
                true,      /*ignoreErrors*/
                false,     /*dontResolveAlias*/
                Node::NIL, /*location*/
            );
            if success.is_some() && success != self.unknown_symbol {
                return success;
            }
        } else if is_entity_name(name) && is_in_right_side_of_import_or_export_assignment(name) {
            // Since we already checked for ExportAssignment, this really could only be an Import
            let import_equals_declaration =
                find_ancestor_kind(name, SyntaxKind::ImportEqualsDeclaration);
            if import_equals_declaration.is_nil() {
                panic!("ImportEqualsDeclaration should be defined");
            }
            return self.get_symbol_of_part_of_right_hand_side_of_import_equals(name);
        }

        if is_entity_name(name) {
            let possible_import_node = is_import_type_qualifier_part(name);
            if possible_import_node.is_some() {
                self.get_type_from_type_node(possible_import_node);
                let sym = self.get_resolved_symbol_or_nil(name);
                return if sym == self.unknown_symbol {
                    SymbolId::NIL
                } else {
                    sym
                };
            }
        }

        while is_right_side_of_qualified_name_or_property_access(name) {
            name = name.parent();
        }

        if is_in_name_of_expression_with_type_arguments_or_heritage_type_reference(name) {
            let mut meaning: SymbolFlags;
            if name.parent().kind() == SyntaxKind::ExpressionWithTypeArguments
                || name.parent().kind() == SyntaxKind::TypeReference
            {
                // A heritage element name may appear in type space, value space, or both;
                // ensure the meaning matches its context.
                meaning = if is_part_of_type_node(name) {
                    SymbolFlags::TYPE
                } else {
                    SymbolFlags::VALUE
                };

                // In a class 'extends' clause we are also looking for a value.
                if is_expression_with_type_arguments_in_class_extends_clause(name.parent()) {
                    meaning = meaning | SymbolFlags::VALUE;
                }
            } else {
                meaning = SymbolFlags::NAMESPACE;
            }

            meaning = meaning | SymbolFlags::ALIAS;
            let mut entity_name_symbol = SymbolId::NIL;
            if is_entity_name_expression(name) {
                entity_name_symbol = self.resolve_entity_name(
                    name,
                    meaning,
                    true,      /*ignoreErrors*/
                    false,     /*dontResolveAlias*/
                    Node::NIL, /*location*/
                );
            }
            if entity_name_symbol.is_some() {
                return entity_name_symbol;
            }
        }

        if is_expression_node(name) {
            if node_is_missing(name) {
                // Missing entity name.
                return SymbolId::NIL;
            }
            let is_js_doc = is_js_doc_name_reference_context(name);
            if is_identifier(name) {
                if is_jsx_tag_name(name) && is_jsx_intrinsic_tag_name(name) {
                    let symbol = self.get_intrinsic_tag_symbol(name.parent());
                    return if symbol == self.unknown_symbol {
                        SymbolId::NIL
                    } else {
                        symbol
                    };
                }
                let meaning = if is_js_doc {
                    SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE
                } else {
                    SymbolFlags::VALUE
                };
                let mut location = Node::NIL;
                if is_js_doc {
                    location = get_host_signature_from_js_doc(name);
                }
                let mut result = self.resolve_entity_name(
                    name, meaning, true, /*ignoreErrors*/
                    true, /*dontResolveAlias*/
                    location,
                );
                if result.is_nil() && is_js_doc {
                    let container = find_ancestor(name, is_class_or_interface_like);
                    if container.is_some() {
                        let symbol = self.get_symbol_of_declaration(container);
                        // Handle unqualified references to class static members and class or interface instance members
                        let exports = self.get_exports_of_symbol(symbol);
                        let s = self.get_symbol(exports, name.text(), meaning);
                        result = self.get_merged_symbol(s);
                        if result.is_nil() {
                            let declared_type = self.get_declared_type_of_symbol(symbol);
                            result = self.get_property_of_type(declared_type, name.text());
                        }
                    }
                }
                return result;
            } else if is_private_identifier(name) {
                return self.get_symbol_for_private_identifier_expression(name);
            } else if is_property_access_expression(name) || is_qualified_name(name) {
                let resolved_symbol = self.symbol_node_links.get(name).resolved_symbol;
                if resolved_symbol.is_some() {
                    return resolved_symbol;
                }
                if is_property_access_expression(name) {
                    self.check_property_access_expression(
                        name,
                        CheckMode::NORMAL,
                        false, /*writeOnly*/
                    );
                    if self.symbol_node_links.get(name).resolved_symbol.is_nil()
                        && !is_private_identifier(name.name())
                    {
                        let expr_type = self.check_expression_cached(name.expression());
                        let key_type = self.get_literal_type_from_property_name(name.name());
                        let index_symbol = self.get_applicable_index_symbol(expr_type, key_type);
                        self.symbol_node_links.get(name).resolved_symbol = index_symbol;
                    }
                } else {
                    self.check_qualified_name(name, CheckMode::NORMAL);
                }
                if self.symbol_node_links.get(name).resolved_symbol.is_nil()
                    && is_js_doc
                    && is_qualified_name(name)
                {
                    return self.resolve_js_doc_member_name(name);
                }
                return self.symbol_node_links.get(name).resolved_symbol;
            }
        } else if is_entity_name(name) && is_type_reference_identifier(name) {
            let meaning = if name.parent().kind() == SyntaxKind::TypeReference {
                SymbolFlags::TYPE
            } else {
                SymbolFlags::NAMESPACE
            };
            let symbol = self.resolve_entity_name(
                name,
                meaning,
                true,      /*ignoreErrors*/
                true,      /*dontResolveAlias*/
                Node::NIL, /*location*/
            );
            if symbol.is_some() && symbol != self.unknown_symbol {
                return symbol;
            }
            if is_name_of_heritage_clause_type_reference(name) {
                return SymbolId::NIL;
            }
            return self.get_unresolved_symbol_for_entity_name(name);
        }

        if name.parent().kind() == SyntaxKind::TypePredicate {
            return self.resolve_entity_name(
                name,
                SymbolFlags::FUNCTION_SCOPED_VARIABLE, /*meaning*/
                true,                                  /*ignoreErrors*/
                false,                                 /*dontResolveAlias*/
                Node::NIL,                             /*location*/
            );
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:32403 isThisPropertyAndThisTyped
    pub fn is_this_property_and_this_typed(&mut self, node: Node) -> bool {
        if node.expression().kind() == SyntaxKind::ThisKeyword {
            let container = self.get_this_container(
                node, false, /*includeArrowFunctions*/
                false, /*includeClassComputedPropertyName*/
            );
            if is_function_like(container) {
                let containing_literal = get_containing_object_literal(container);
                if containing_literal.is_some() {
                    let contextual_type = self.get_apparent_type_of_contextual_type(
                        containing_literal,
                        ContextFlags::NONE,
                    );
                    let t = self.get_this_type_of_object_literal_from_contextual_type(
                        containing_literal,
                        contextual_type,
                    );
                    return t.is_some() && !self.is_type_any(t);
                }
            }
        }
        false
    }

    // Go: checker/checker.go:32418 getTypeOfNode
    pub fn get_type_of_node(&mut self, node: Node) -> TypeId {
        if is_source_file(node) && !is_external_or_common_js_module(node) {
            return self.error_type;
        }

        if node.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return self.error_type;
        }

        let (class_decl, is_implements) =
            try_get_class_implementing_or_extending_heritage_clause_element(node);
        let mut class_type = TypeId::NIL;
        if class_decl.is_some() {
            let class_symbol = self.get_symbol_of_declaration(class_decl);
            class_type = self.get_declared_type_of_class_or_interface(class_symbol);
        }

        if is_part_of_type_node(node) {
            let type_from_type_node = self.get_type_from_type_node(node);
            if class_type.is_some() {
                let this_type = self.ty(class_type).as_interface_type().this_type;
                return self.get_type_with_this_argument(
                    type_from_type_node,
                    this_type,
                    false, /*needApparentType*/
                );
            }
            return type_from_type_node;
        }

        if is_expression_node(node) {
            return self.get_regular_type_of_expression(node);
        }

        if class_type.is_some() && !is_implements {
            // A SyntaxKind.ExpressionWithTypeArguments is considered a type node, except when it occurs in the
            // extends clause of a class. We handle that case here.
            let base_type = self
                .get_base_types(class_type)
                .first()
                .copied()
                .unwrap_or(TypeId::NIL);
            if base_type.is_some() {
                let this_type = self.ty(class_type).as_interface_type().this_type;
                return self.get_type_with_this_argument(
                    base_type, this_type, false, /*needApparentType*/
                );
            }
            return self.error_type;
        }

        if is_type_declaration(node) {
            // In this case, we call getSymbolOfDeclaration instead of getSymbolAtLocation because it is a declaration
            let symbol = self.get_symbol_of_declaration(node);
            return self.get_declared_type_of_symbol(symbol);
        }

        if is_type_declaration_name(node) {
            let symbol = self.get_symbol_at_location(node, false /*ignoreErrors*/);
            if symbol.is_some() {
                return self.get_declared_type_of_symbol(symbol);
            }
            return self.error_type;
        }

        if is_binding_element(node) {
            let t = self.get_type_for_variable_like_declaration(
                node,
                true, /*includeOptionality*/
                CheckMode::NORMAL,
            );
            if t.is_some() {
                return t;
            }
            return self.error_type;
        }

        if is_declaration(node) {
            // In this case, we call getSymbolOfDeclaration instead of getSymbolLAtocation because it is a declaration
            let symbol = self.get_symbol_of_declaration(node);
            if symbol.is_some() {
                return self.get_type_of_symbol(symbol);
            }
            return self.error_type;
        }

        if is_declaration_name_or_import_property_name(node) {
            let symbol = self.get_symbol_at_location(node, false /*ignoreErrors*/);
            if symbol.is_some() {
                return self.get_type_of_symbol(symbol);
            }
            return self.error_type;
        }

        if is_binding_pattern(node) {
            let t = self.get_type_for_variable_like_declaration(
                node.parent(),
                true, /*includeOptionality*/
                CheckMode::NORMAL,
            );
            if t.is_some() {
                return t;
            }
            return self.error_type;
        }

        if is_in_right_side_of_import_or_export_assignment(node) {
            let symbol = self.get_symbol_at_location(node, false /*ignoreErrors*/);
            if symbol.is_some() {
                let declared_type = self.get_declared_type_of_symbol(symbol);
                if !self.is_error_type(declared_type) {
                    return declared_type;
                }
                return self.get_type_of_symbol(symbol);
            }
        }

        if is_meta_property(node.parent()) && node.parent().keyword_token() == node.kind() {
            return self.check_meta_property_keyword(node.parent());
        }

        if is_import_attributes(node) {
            return self.check_import_attributes_expression(node);
        }

        self.error_type
    }

    // Go: checker/checker.go:32530 getThisTypeOfObjectLiteralFromContextualType
    pub fn get_this_type_of_object_literal_from_contextual_type(
        &mut self,
        containing_literal: Node,
        contextual_type: TypeId,
    ) -> TypeId {
        let mut literal = containing_literal;
        let mut t = contextual_type;
        while t.is_some() {
            let this_type = self.get_this_type_from_contextual_type(t);
            if this_type.is_some() {
                return this_type;
            }
            if literal.parent().kind() != SyntaxKind::PropertyAssignment {
                break;
            }
            literal = literal.parent().parent();
            t = self.get_apparent_type_of_contextual_type(literal, ContextFlags::NONE);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:32547 getThisTypeFromContextualType
    pub fn get_this_type_from_contextual_type(&mut self, t: TypeId) -> TypeId {
        self.map_type(t, &mut |c: &mut Checker, t: TypeId| -> TypeId {
            if c.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
                for i in 0..c.ty(t).types().len() {
                    let t = c.type_at(t, i);
                    let type_arg = c.get_this_type_argument(t);
                    if type_arg.is_some() {
                        return type_arg;
                    }
                }
                TypeId::NIL
            } else {
                c.get_this_type_argument(t)
            }
        })
    }

    // Go: checker/checker.go:32563 getThisTypeArgument
    pub fn get_this_type_argument(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE)
            && self.ty(t).target() == self.global_this_type
        {
            return self.type_arguments_of(t)[0];
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:32570 getApplicableIndexInfos
    pub fn get_applicable_index_infos(&mut self, t: TypeId, key_type: TypeId) -> Vec<IndexInfoId> {
        // Go: core.Filter
        let infos = self.get_index_infos_of_type(t);
        let mut result = Vec::new();
        for info in infos {
            let info_key_type = self.index_info(info).key_type;
            if self.is_applicable_index_type(key_type, info_key_type) {
                result.push(info);
            }
        }
        result
    }

    // Go: checker/checker.go:32574 getApplicableIndexSymbol
    pub fn get_applicable_index_symbol(&mut self, t: TypeId, key_type: TypeId) -> SymbolId {
        let info = self.get_applicable_index_info(t, key_type);
        if info.is_some() && info != self.any_base_type_index_info {
            if self.index_info(info).index_symbol.is_nil() {
                let mut declarations: Vec<Node> = Vec::new();
                let info_declaration = self.index_info(info).declaration;
                if info_declaration.is_some() {
                    declarations = vec![info_declaration];
                } else {
                    for info in self.get_index_infos_of_type(t) {
                        let declaration = self.index_info(info).declaration;
                        let info_key_type = self.index_info(info).key_type;
                        if declaration.is_some()
                            && self.is_applicable_index_type(key_type, info_key_type)
                        {
                            declarations.push(declaration);
                        }
                    }
                }
                if !declarations.is_empty() {
                    let symbol = self.new_symbol(SymbolFlags::PROPERTY, INTERNAL_SYMBOL_NAME_INDEX);
                    let type_symbol = self.ty(t).symbol;
                    {
                        let s = self.sym_mut(symbol);
                        s.check_flags = s.check_flags | CheckFlags::INDEX_SYMBOL;
                        s.value_declaration = declarations[0];
                        s.declarations = declarations.into();
                        s.parent = type_symbol;
                    }
                    let value_type = self.index_info(info).value_type;
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .resolved_type = value_type;
                    self.index_info_mut(info).index_symbol = symbol;
                }
            }
            return self.index_info(info).index_symbol;
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:32603 getRegularTypeOfExpression
    pub fn get_regular_type_of_expression(&mut self, mut expr: Node) -> TypeId {
        if is_right_side_of_qualified_name_or_property_access(expr) {
            expr = expr.parent();
        }
        let t = self.get_type_of_expression(expr);
        self.get_regular_type_of_literal_type(t)
    }

    // Go: checker/checker.go:32610 containsArgumentsReference
    pub fn contains_arguments_reference(&mut self, node: Node) -> bool {
        if node.body().is_nil() {
            return false;
        }

        if let Some(&contains_arguments) = self.cached_arguments_referenced.get(&node) {
            return contains_arguments;
        }

        let contains_arguments = self.contains_arguments_reference_visit(node.body());
        self.cached_arguments_referenced
            .insert(node, contains_arguments);
        contains_arguments
    }

    // Go: checker/checker.go:31884 containsArgumentsReference.visit
    // PORT: the Go recursive closure `visit` becomes this helper method.
    fn contains_arguments_reference_visit(&mut self, node: Node) -> bool {
        if node.is_nil() {
            return false;
        }
        match node.kind() {
            SyntaxKind::Identifier => {
                let arguments_name = self.sym(self.arguments_symbol).name.clone();
                return node.text() == arguments_name && {
                    let resolved = self.get_resolved_symbol(node);
                    // PORT: Go c.IsArgumentsSymbol (services.go) is `symbol == c.argumentsSymbol`; inlined.
                    resolved == self.arguments_symbol
                };
            }
            SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => {
                if is_computed_property_name(node.name()) {
                    return self.contains_arguments_reference_visit(node.name());
                }
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                return self.contains_arguments_reference_visit(node.expression());
            }
            SyntaxKind::PropertyAssignment => {
                return self.contains_arguments_reference_visit(node.initializer());
            }
            _ => {}
        }
        if node_starts_new_lexical_environment(node) || is_part_of_type_node(node) {
            return false;
        }
        node.for_each_child(&mut |child: Node| self.contains_arguments_reference_visit(child))
    }

    // Go: checker/checker.go:32647 GetTypeAtLocation
    pub fn get_type_at_location(&mut self, node: Node) -> TypeId {
        self.get_type_of_node(get_reparsed_node_for_node(node))
    }

    // Go: checker/checker.go:32659 GetAliasedSymbol
    pub fn get_aliased_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        self.resolve_alias(symbol)
    }
}
