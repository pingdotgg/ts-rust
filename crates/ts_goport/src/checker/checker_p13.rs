//! Port of Go `checker/checker.go` lines 11172-12100.
//!
//! PORT: Go `c.error(...)` returns the `*ast.Diagnostic` it already added, and
//! callers then mutate it (`AddRelatedInfo`). Diagnostics are owned values
//! here, so those call sites build the diagnostic with
//! `new_diagnostic_for_node`, add the related information, then call
//! `self.add_diagnostic` (the same two steps Go `error` does). The collection
//! is append-only, so the stored result is the same.

use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:11402 isSameScopedBindingElement
    pub fn is_same_scoped_binding_element(&mut self, node: Node, declaration: Node) -> bool {
        if is_binding_element(declaration) {
            let binding_element = find_ancestor(node, is_binding_element);
            return binding_element.is_some()
                && get_root_declaration(binding_element) == get_root_declaration(declaration);
        }
        false
    }

    // Go: checker/checker.go:11411 removeOptionalityFromDeclaredType
    // Remove undefined from the annotated type of a parameter when there is an initializer (that doesn't include undefined)
    pub fn remove_optionality_from_declared_type(
        &mut self,
        declared_type: TypeId,
        declaration: Node,
    ) -> TypeId {
        let remove_undefined = self.strict_null_checks
            && is_parameter_declaration(declaration)
            && declaration.initializer().is_some()
            && self.has_type_facts(declared_type, TypeFacts::IS_UNDEFINED)
            && !self.parameter_initializer_contains_undefined(declaration);
        if remove_undefined {
            return self.get_type_with_facts(declared_type, TypeFacts::NE_UNDEFINED);
        }
        declared_type
    }

    // Go: checker/checker.go:11419 parameterInitializerContainsUndefined
    pub fn parameter_initializer_contains_undefined(&mut self, declaration: Node) -> bool {
        if !self
            .node_links
            .get(declaration)
            .flags
            .intersects(NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED)
        {
            if !self.push_type_resolution(
                TypeSystemEntity::Node(declaration),
                TypeSystemPropertyName::INITIALIZER_IS_UNDEFINED,
            ) {
                self.report_circularity_error(declaration.symbol());
                return true;
            }
            let initializer_type =
                self.check_declaration_initializer(declaration, CheckMode::NORMAL, TypeId::NIL);
            let contains_undefined = self.has_type_facts(initializer_type, TypeFacts::IS_UNDEFINED);
            if !self.pop_type_resolution() {
                self.report_circularity_error(declaration.symbol());
                return true;
            }
            let links = self.node_links.get(declaration);
            if !links
                .flags
                .intersects(NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED)
            {
                links.flags |= NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED
                    | if contains_undefined {
                        NodeCheckFlags::INITIALIZER_IS_UNDEFINED
                    } else {
                        NodeCheckFlags::NONE
                    };
            }
        }
        self.node_links
            .get(declaration)
            .flags
            .intersects(NodeCheckFlags::INITIALIZER_IS_UNDEFINED)
    }

    // Go: checker/checker.go:11438 isInAmbientOrTypeNode
    // PERF: U4 (CH7). `AMBIENT` is a parser bit (`Node::parser_flags`), and
    // the walk tests kinds from the store tables (`find_ancestor_with_kind`).
    pub fn is_in_ambient_or_type_node(&mut self, node: Node) -> bool {
        !node.parser_flags(NodeFlags::AMBIENT).is_empty()
            || find_ancestor_with_kind(node, |_, kind| {
                matches!(
                    kind,
                    SyntaxKind::InterfaceDeclaration
                        | SyntaxKind::TypeAliasDeclaration
                        | SyntaxKind::JsTypeAliasDeclaration
                        | SyntaxKind::TypeLiteral
                )
            })
            .is_some()
    }

    // Go: checker/checker.go:11444 checkPropertyAccessExpression
    pub fn check_property_access_expression(
        &mut self,
        node: Node,
        check_mode: CheckMode,
        write_only: bool,
    ) -> TypeId {
        if !node.parser_flags(NodeFlags::OPTIONAL_CHAIN).is_empty() {
            return self.check_property_access_chain(node, check_mode);
        }
        let expr = node.expression();
        let left_type = self.check_non_null_expression(expr);
        self.check_property_access_expression_or_qualified_name(
            node,
            expr,
            left_type,
            node.name(),
            check_mode,
            write_only,
        )
    }

    // Go: checker/checker.go:11452 checkPropertyAccessChain
    pub fn check_property_access_chain(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let left_type = self.check_expression(node.expression());
        let non_optional_type = self.get_optional_expression_type(left_type, node.expression());
        let non_null_type = self.check_non_null_type(non_optional_type, node.expression());
        let t = self.check_property_access_expression_or_qualified_name(
            node,
            node.expression(),
            non_null_type,
            node.name(),
            check_mode,
            false,
        );
        self.propagate_optional_type_marker(t, node, non_optional_type != left_type)
    }

    // Go: checker/checker.go:11458 checkPropertyAccessExpressionOrQualifiedName
    pub fn check_property_access_expression_or_qualified_name(
        &mut self,
        node: Node,
        left: Node,
        left_type: TypeId,
        right: Node,
        check_mode: CheckMode,
        write_only: bool,
    ) -> TypeId {
        let parent_symbol = self.get_resolved_symbol_or_nil(left);
        let assignment_kind = get_assignment_target_kind(node);
        let mut widened_type = left_type;
        if assignment_kind != AssignmentKind::NONE || self.is_method_access_for_call(node) {
            widened_type = self.get_widened_type(left_type);
        }
        let apparent_type = self.get_apparent_type(widened_type);
        let is_any_like =
            self.is_type_any(apparent_type) || apparent_type == self.silent_never_type;
        let mut prop = SymbolId::NIL;
        if is_private_identifier(right) {
            if self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.private_names_and_class_static_blocks
                || self.language_version
                    < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators
                || !self.compiler_options.get_use_define_for_class_fields()
            {
                if assignment_kind != AssignmentKind::NONE {
                    self.check_external_emit_helpers(
                        node,
                        ExternalEmitHelpers::CLASS_PRIVATE_FIELD_SET,
                    );
                }
                if assignment_kind != AssignmentKind::DEFINITE {
                    self.check_external_emit_helpers(
                        node,
                        ExternalEmitHelpers::CLASS_PRIVATE_FIELD_GET,
                    );
                }
            }
            let lexically_scoped_symbol =
                self.lookup_symbol_for_private_identifier_declaration(right.text(), right);
            if assignment_kind != AssignmentKind::NONE
                && lexically_scoped_symbol.is_some()
                && self
                    .sym(lexically_scoped_symbol)
                    .value_declaration
                    .is_some()
                && is_method_declaration(self.sym(lexically_scoped_symbol).value_declaration)
            {
                self.grammar_error_on_node(
                    right,
                    diag::Cannot_assign_to_private_method_0_Private_methods_are_not_writable,
                    args![right.text()],
                );
            }
            if is_any_like {
                if lexically_scoped_symbol.is_some() {
                    if self.is_error_type(apparent_type) {
                        return self.error_type;
                    }
                    return apparent_type;
                }
                if get_containing_class_excluding_class_decorators(right).is_nil() {
                    self.grammar_error_on_node(
                        right,
                        diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                        args![],
                    );
                    return self.any_type;
                }
            }
            if lexically_scoped_symbol.is_some() {
                prop = self
                    .get_private_identifier_property_of_type(left_type, lexically_scoped_symbol);
            }
            if prop.is_nil() {
                // Check for private-identifier-specific shadowing and lexical-scoping errors.
                if self.check_private_identifier_property_access(
                    left_type,
                    right,
                    lexically_scoped_symbol,
                ) {
                    return self.error_type;
                }
                let containing_class = get_containing_class_excluding_class_decorators(right);
                if containing_class.is_some()
                    && is_plain_js_file(
                        get_source_file_of_node(containing_class),
                        self.compiler_options.check_js,
                    )
                {
                    self.grammar_error_on_node(
                        right,
                        diag::Private_field_0_must_be_declared_in_an_enclosing_class,
                        args![right.text()],
                    );
                }
            } else {
                let prop_flags = self.sym(prop).flags;
                let is_setonly_accessor = prop_flags.intersects(SymbolFlags::SET_ACCESSOR)
                    && !prop_flags.intersects(SymbolFlags::GET_ACCESSOR);
                if is_setonly_accessor && assignment_kind != AssignmentKind::DEFINITE {
                    self.error(
                        node,
                        diag::Private_accessor_was_defined_without_a_getter,
                        args![],
                    );
                }
            }
        } else {
            if is_any_like {
                if is_identifier(left) && parent_symbol.is_some() {
                    self.mark_linked_references(
                        node,
                        ReferenceHint::PROPERTY,
                        SymbolId::NIL, /*propSymbol*/
                        left_type,
                    );
                }
                if self.is_error_type(apparent_type) {
                    return self.error_type;
                }
                return apparent_type;
            }
            let skip_object_function_property_augment =
                self.is_const_enum_object_type(apparent_type);
            prop = self.get_property_of_type_ex(
                apparent_type,
                right.text(),
                skip_object_function_property_augment, /*skipObjectFunctionPropertyAugment*/
                node.kind() == SyntaxKind::QualifiedName, /*includeTypeOnlyMembers*/
            );
        }
        self.mark_linked_references(node, ReferenceHint::PROPERTY, prop, left_type);
        let prop_type;
        if prop.is_nil() {
            let mut index_info = IndexInfoId::NIL;
            if !is_private_identifier(right)
                && (assignment_kind == AssignmentKind::NONE
                    || !self.is_generic_object_type(left_type)
                    || self.is_this_type_parameter(left_type))
            {
                index_info = self.get_applicable_index_info_for_name(apparent_type, right.text());
            }
            if index_info.is_nil() {
                let left_symbol = self.ty(left_type).symbol;
                let is_unchecked_js = self.is_unchecked_js_suggestion(
                    node,
                    left_symbol,
                    true, /*excludeClasses*/
                );
                if !is_unchecked_js && self.is_js_literal_type(left_type) {
                    return self.any_type;
                }
                if left_symbol == self.global_this_symbol {
                    let global_exports = self.sym(self.global_this_symbol).exports;
                    let global_symbol = self.symbols.get(global_exports, right.text());
                    if global_symbol.is_some()
                        && self
                            .sym(global_symbol)
                            .flags
                            .intersects(SymbolFlags::BLOCK_SCOPED)
                    {
                        let type_string = self.type_to_string(left_type);
                        self.error(
                            right,
                            diag::Property_0_does_not_exist_on_type_1,
                            args![right.text(), type_string],
                        );
                    } else if self.no_implicit_any {
                        let type_string = self.type_to_string(left_type);
                        self.error(
                            right,
                            diag::Element_implicitly_has_an_any_type_because_type_0_has_no_index_signature,
                            args![type_string],
                        );
                    }
                    return self.any_type;
                }
                if !right.text().is_empty()
                    && !self.check_and_report_error_for_extending_interface(node)
                {
                    self.add_deferred_diagnostic(Rc::new(move |c: &mut Checker| {
                        // must be deferred because reporting this error can cause us to materialize the containing type completely (to print it), leading to erroneous circularity errors
                        let containing_type = if c.is_this_type_parameter(left_type) {
                            apparent_type
                        } else {
                            left_type
                        };
                        c.report_nonexistent_property(right, containing_type, is_unchecked_js);
                    }));
                }
                return self.error_type;
            }
            let info_is_readonly = self.index_info(index_info).is_readonly;
            if info_is_readonly && (is_assignment_target(node) || is_delete_target(node)) {
                let type_string = self.type_to_string(apparent_type);
                self.error(
                    node,
                    diag::Index_signature_in_type_0_only_permits_reading,
                    args![type_string],
                );
            }
            let mut t = self.index_info(index_info).value_type;
            if self.compiler_options.no_unchecked_indexed_access == Tristate::True
                && get_assignment_target_kind(node) != AssignmentKind::DEFINITE
            {
                let missing_type = self.missing_type;
                t = self.get_union_type(&[t, missing_type]);
            }
            if self
                .compiler_options
                .no_property_access_from_index_signature
                == Tristate::True
                && is_property_access_expression(node)
            {
                self.error(
                    right,
                    diag::Property_0_comes_from_an_index_signature_so_it_must_be_accessed_with_0,
                    args![right.text()],
                );
            }
            let info_declaration = self.index_info(index_info).declaration;
            if info_declaration.is_some() && self.is_deprecated_declaration(info_declaration) {
                self.add_deprecated_suggestion(right, &[info_declaration], right.text());
            }
            prop_type = t;
        } else {
            let target_prop_symbol = self.resolve_alias_with_deprecation_check(prop, right);
            if self.is_deprecated_symbol(target_prop_symbol)
                && self.is_uncalled_function_reference(node, target_prop_symbol)
                && !self.sym(target_prop_symbol).declarations.is_empty()
            {
                // PORT: Go checks `Declarations != nil`; the binder never stores an
                // empty non-nil slice, so `!is_empty()` matches.
                let declarations = self.sym(target_prop_symbol).declarations.clone();
                self.add_deprecated_suggestion(right, &declarations, right.text());
            }
            self.check_property_not_used_before_declaration(prop, node, right);
            let is_self_type_access = self.is_self_type_access(left, parent_symbol);
            self.mark_property_as_referenced(prop, node, is_self_type_access);
            self.symbol_node_links.get(node).resolved_symbol = prop;
            self.check_property_accessibility(
                node,
                left.kind() == SyntaxKind::SuperKeyword,
                is_write_access(node),
                apparent_type,
                prop,
            );
            if self.is_assignment_to_readonly_entity(node, prop, assignment_kind) {
                self.error(
                    right,
                    diag::Cannot_assign_to_0_because_it_is_a_read_only_property,
                    args![right.text()],
                );
                return self.error_type;
            }
            prop_type = if self.is_this_property_access_in_constructor(node, prop) {
                self.auto_type
            } else if write_only || is_write_only_access(node) {
                self.get_write_type_of_symbol(prop)
            } else {
                self.get_type_of_symbol(prop)
            };
        }
        self.get_flow_type_of_access_expression(node, prop, prop_type, right, check_mode)
    }

    // Go: checker/checker.go:11592 getFlowTypeOfAccessExpression
    pub fn get_flow_type_of_access_expression(
        &mut self,
        node: Node,
        prop: SymbolId,
        prop_type: TypeId,
        error_node: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        // Only compute control flow type if this is a property access expression that isn't an
        // assignment target, and the referenced property was declared as a variable, property,
        // accessor, or optional method.
        let assignment_kind = get_assignment_target_kind(node);
        if assignment_kind == AssignmentKind::DEFINITE {
            let is_optional =
                prop.is_some() && self.sym(prop).flags.intersects(SymbolFlags::OPTIONAL);
            return self.remove_missing_type(prop_type, is_optional);
        }
        if prop.is_some() {
            let prop_flags = self.sym(prop).flags;
            if !prop_flags
                .intersects(SymbolFlags::VARIABLE | SymbolFlags::PROPERTY | SymbolFlags::ACCESSOR)
                && !(prop_flags.intersects(SymbolFlags::METHOD)
                    && self.ty(prop_type).flags.intersects(TypeFlags::UNION))
            {
                return prop_type;
            }
        }
        if prop_type == self.auto_type {
            return self.get_flow_type_of_property(node, prop);
        }
        let prop_type = self.get_narrowable_type_for_reference(prop_type, node, check_mode);
        // If strict null checks and strict property initialization checks are enabled, if we have
        // a this.xxx property access, if the property is an instance property without an initializer,
        // and if we are in a constructor of the same class as the property declaration, assume that
        // the property is uninitialized at the top of the control flow.
        let mut assume_uninitialized = false;
        if self.strict_null_checks && prop.is_some() {
            let declaration = self.sym(prop).value_declaration;
            if declaration.is_some() {
                if self.strict_property_initialization
                    && is_access_expression(node)
                    && node.expression().kind() == SyntaxKind::ThisKeyword
                    && self.is_property_without_initializer(declaration)
                    && !is_static(declaration)
                {
                    let flow_container = self.get_control_flow_container(node);
                    if is_constructor_declaration(flow_container)
                        && flow_container.parent() == declaration.parent()
                        && declaration.parser_flags(NodeFlags::AMBIENT).is_empty()
                    {
                        assume_uninitialized = true;
                    }
                } else if is_binary_expression(declaration)
                    && is_property_access_expression(declaration.left())
                    && self.get_control_flow_container(node)
                        == self.get_control_flow_container(declaration)
                {
                    assume_uninitialized = true;
                }
            }
        }
        let initial_type =
            self.add_optionality_ex(prop_type, false /*isProperty*/, assume_uninitialized);
        let flow_type = self.get_flow_type_of_reference_ex(
            node,
            prop_type,
            initial_type,
            Node::NIL,
            FlowNodeId::NIL,
        );
        if assume_uninitialized
            && !self.contains_undefined_type(prop_type)
            && self.contains_undefined_type(flow_type)
        {
            let prop_string = self.symbol_to_string(prop);
            self.error(
                error_node,
                diag::Property_0_is_used_before_being_assigned,
                args![prop_string],
            );
            // Return the declared type to reduce follow-on errors
            return prop_type;
        }
        if assignment_kind != AssignmentKind::NONE {
            return self.get_base_type_of_literal_type(flow_type);
        }
        flow_type
    }

    // Go: checker/checker.go:11638 getControlFlowContainer
    // PERF: U4 (CH7). The walk tests kinds from the store tables
    // (`find_ancestor_with_kind`); `is_function_like(n)` is
    // `is_function_like_kind(n.kind())` for a node that is not nil.
    // PERF (cfcache1, not in Go): the answer depends only on the tree, so a
    // small table keeps recent answers (`control_flow_containers`). It
    // keeps only a walk that stayed in one published store whose parents
    // are all in that store (`StoreFacts::parents_local`): the walk and its
    // immediately invoked function test then read only parents of that
    // store, which never change. In the cfcache1 counts (11 projects), 91.6%
    // of the calls come from `check_identifier` (the declaration, then the
    // reference; 99.8% with its loop), and 63% of all calls (44% to 77% per
    // project) ask again for a node that was asked before.
    pub fn get_control_flow_container(&mut self, node: Node) -> Node {
        let slot = control_flow_container_slot(node);
        let (key, container) = self.control_flow_containers[slot];
        if key == node && node.is_some() {
            return container;
        }
        let is_container = |node: Node, kind: SyntaxKind| {
            is_function_like_kind(kind)
                && get_immediately_invoked_function_expression(node).is_nil()
                || matches!(
                    kind,
                    SyntaxKind::ModuleBlock
                        | SyntaxKind::SourceFile
                        | SyntaxKind::PropertyDeclaration
                )
        };
        // `frozen_store_parent` is `Some` only for a published store node
        // whose parent is in the same store.
        match frozen_store_parent(node)
            .and_then(|parent| frozen_find_ancestor(parent, is_container))
        {
            Some(AncestorWalk::Found(container)) => {
                // The test of a function-like node reads its parent, which
                // can be outside the store when some parent of the store is.
                if frozen_node_store_facts(node).is_some_and(|facts| facts.parents_local) {
                    self.control_flow_containers[slot] = (node, container);
                }
                container
            }
            Some(AncestorWalk::Next(next)) => find_ancestor_with_kind(next, is_container),
            None => find_ancestor_with_kind(node.parent(), is_container),
        }
    }

    // Go: checker/checker.go:11644 getFlowTypeOfProperty
    pub fn get_flow_type_of_property(&mut self, reference: Node, prop: SymbolId) -> TypeId {
        let mut initial_type = self.undefined_type;
        if prop.is_some() && self.sym(prop).value_declaration.is_some() {
            let value_declaration = self.sym(prop).value_declaration;
            if !self.is_auto_typed_property(prop)
                || value_declaration
                    .modifier_flags()
                    .intersects(ModifierFlags::AMBIENT)
            {
                let base_type = self.get_type_of_property_in_base_class(prop);
                if base_type.is_some() {
                    initial_type = base_type;
                }
            }
        }
        let auto_type = self.auto_type;
        self.get_flow_type_of_reference_ex(
            reference,
            auto_type,
            initial_type,
            Node::NIL,
            FlowNodeId::NIL,
        )
    }

    // Go: checker/checker.go:11655 getTypeOfPropertyInBaseClass
    // Return the inherited type of the given property or undefined if property doesn't exist in a base class.
    pub fn get_type_of_property_in_base_class(&mut self, property: SymbolId) -> TypeId {
        let class_type = self.get_declaring_class(property);
        if class_type.is_some() {
            let base_class_types = self.get_base_types(class_type);
            if !base_class_types.is_empty() {
                let name = self.sym(property).name.clone();
                return self.get_type_of_property_of_type(base_class_types[0], &name);
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:11666 isMethodAccessForCall
    pub fn is_method_access_for_call(&mut self, node: Node) -> bool {
        let mut node = node;
        while is_parenthesized_expression(node.parent()) {
            node = node.parent();
        }
        is_call_or_new_expression(node.parent()) && node.parent().expression() == node
    }

    /// Go `binder.GetSymbolNameForPrivateIdentifier` in the checker: the
    /// symbol table key of private name `description` in class
    /// `containing_class_symbol`. Go puts the id of the class in the key
    /// (`ast.GetSymbolId`), so it gives the class its id here.
    // PORT: the key holds the arena index of the class
    // (`get_symbol_name_for_private_identifier`); the id is only given, as
    // Go gives it. The binder gives no id (PORTING.md, Threads).
    pub fn private_identifier_symbol_name(
        &self,
        containing_class_symbol: SymbolId,
        description: &str,
    ) -> String {
        get_symbol_id(&self.symbols, containing_class_symbol);
        get_symbol_name_for_private_identifier(&self.symbols, containing_class_symbol, description)
    }

    // Go: checker/checker.go:11674 lookupSymbolForPrivateIdentifierDeclaration
    // Lookup the private identifier lexically.
    pub fn lookup_symbol_for_private_identifier_declaration(
        &mut self,
        prop_name: &str,
        location: Node,
    ) -> SymbolId {
        let mut containing_class = get_containing_class_excluding_class_decorators(location);
        while containing_class.is_some() {
            let symbol = containing_class.symbol();
            let name = self.private_identifier_symbol_name(symbol, prop_name);
            let members = self.sym(symbol).members;
            let prop = self.symbols.get(members, &name);
            if prop.is_some() {
                return prop;
            }
            let exports = self.sym(symbol).exports;
            let prop = self.symbols.get(exports, &name);
            if prop.is_some() {
                return prop;
            }
            containing_class = get_containing_class(containing_class);
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:11690 getPrivateIdentifierPropertyOfType
    pub fn get_private_identifier_property_of_type(
        &mut self,
        left_type: TypeId,
        lexically_scoped_identifier: SymbolId,
    ) -> SymbolId {
        let name = self.sym(lexically_scoped_identifier).name.clone();
        self.get_property_of_type(left_type, &name)
    }

    // Go: checker/checker.go:11694 checkPrivateIdentifierPropertyAccess
    pub fn check_private_identifier_property_access(
        &mut self,
        left_type: TypeId,
        right: Node,
        lexically_scoped_identifier: SymbolId,
    ) -> bool {
        // Either the identifier could not be looked up in the lexical scope OR the lexically scoped identifier did not exist on the type.
        // Find a private identifier with the same description on the type.
        let properties = self.get_properties_of_type(left_type);
        let mut property_on_type = SymbolId::NIL;
        for &symbol in properties.iter() {
            let decl = self.sym(symbol).value_declaration;
            if decl.is_some()
                && decl.name().is_some()
                && is_private_identifier(decl.name())
                && decl.name().text() == right.text()
            {
                property_on_type = symbol;
                break;
            }
        }
        let diag_name = declaration_name_to_string(right);
        if property_on_type.is_some() {
            let type_value_decl = self.sym(property_on_type).value_declaration;
            let type_class = get_containing_class(type_value_decl);
            // We found a private identifier property with the same description.
            // Either:
            // - There is a lexically scoped private identifier AND it shadows the one we found on the type.
            // - It is an attempt to access the private identifier outside of the class.
            if lexically_scoped_identifier.is_some()
                && self
                    .sym(lexically_scoped_identifier)
                    .value_declaration
                    .is_some()
            {
                let lexical_value_decl = self.sym(lexically_scoped_identifier).value_declaration;
                let lexical_class = get_containing_class(lexical_value_decl);
                if find_ancestor(lexical_class, |n| type_class == n).is_some() {
                    let type_string = self.type_to_string(left_type);
                    let mut diagnostic = new_diagnostic_for_node(
                        right,
                        diag::The_property_0_cannot_be_accessed_on_type_1_within_this_class_because_it_is_shadowed_by_another_private_identifier_with_the_same_spelling,
                        args![diag_name, type_string],
                    );
                    diagnostic.add_related_info(Some(create_diagnostic_for_node(
                        lexical_value_decl,
                        diag::The_shadowing_declaration_of_0_is_defined_here,
                        args![diag_name],
                    )));
                    diagnostic.add_related_info(Some(create_diagnostic_for_node(
                        type_value_decl,
                        diag::The_declaration_of_0_that_you_probably_intended_to_use_is_defined_here,
                        args![diag_name],
                    )));
                    self.add_diagnostic(diagnostic);
                    return true;
                }
            }
            let class_string = self.symbol_to_string(type_class.symbol());
            self.error(
                right,
                diag::Property_0_is_not_accessible_outside_class_1_because_it_has_a_private_identifier,
                args![diag_name, class_string],
            );
            return true;
        }
        false
    }

    // Go: checker/checker.go:11730 reportNonexistentProperty
    pub fn report_nonexistent_property(
        &mut self,
        prop_node: Node,
        containing_type: TypeId,
        is_unchecked_js: bool,
    ) {
        let key = NonExistentPropertyKey {
            prop_node,
            containing_type,
            is_unchecked_js,
        };
        if self.non_existent_properties.contains(&key) {
            return;
        }
        self.non_existent_properties.insert(key);
        let links = self.node_links.get(prop_node);
        if links.flags.intersects(NodeCheckFlags::TYPE_CHECKED) {
            return; // error already made/in progress
        }
        links.flags |= NodeCheckFlags::TYPE_CHECKED;
        if is_js_doc_name_reference_context(prop_node) {
            return;
        }
        // PORT: Go `*ast.Diagnostic` nil is `None`.
        let mut diagnostic: Option<Diagnostic> = None;
        let containing_flags = self.ty(containing_type).flags;
        if !is_private_identifier(prop_node)
            && containing_flags.intersects(TypeFlags::UNION)
            && !containing_flags.intersects(TypeFlags::PRIMITIVE)
        {
            let types = self.ty(containing_type).types_list();
            for subtype in types {
                if self
                    .get_property_of_type(subtype, prop_node.text())
                    .is_nil()
                    && self
                        .get_applicable_index_info_for_name(subtype, prop_node.text())
                        .is_nil()
                {
                    let prop_name = declaration_name_to_string(prop_node);
                    let type_string = self.type_to_string(subtype);
                    diagnostic = Some(new_diagnostic_chain_for_node(
                        diagnostic.take(),
                        prop_node,
                        diag::Property_0_does_not_exist_on_type_1,
                        args![prop_name, type_string],
                    ));
                    break;
                }
            }
        }
        if self.type_has_static_property(prop_node.text(), containing_type) {
            let prop_name = declaration_name_to_string(prop_node);
            let type_name = self.type_to_string(containing_type);
            let static_name = format!("{type_name}.{prop_name}");
            diagnostic = Some(new_diagnostic_chain_for_node(
                diagnostic.take(),
                prop_node,
                diag::Property_0_does_not_exist_on_type_1_Did_you_mean_to_access_the_static_member_2_instead,
                args![prop_name, type_name, static_name],
            ));
        } else {
            let promised_type = self.get_promised_type_of_promise(containing_type);
            if promised_type.is_some()
                && self
                    .get_property_of_type(promised_type, prop_node.text())
                    .is_some()
            {
                let prop_name = declaration_name_to_string(prop_node);
                let type_string = self.type_to_string(containing_type);
                let mut d = new_diagnostic_chain_for_node(
                    diagnostic.take(),
                    prop_node,
                    diag::Property_0_does_not_exist_on_type_1,
                    args![prop_name, type_string],
                );
                d.add_related_info(Some(new_diagnostic_for_node(
                    prop_node,
                    diag::Did_you_forget_to_use_await,
                    args![],
                )));
                diagnostic = Some(d);
            } else {
                let missing_property = declaration_name_to_string(prop_node);
                let container = self.type_to_string(containing_type);
                let lib_suggestion = self.get_suggested_lib_for_non_existent_property(
                    &missing_property,
                    containing_type,
                );
                if !lib_suggestion.is_empty() {
                    diagnostic = Some(new_diagnostic_chain_for_node(
                        diagnostic.take(),
                        prop_node,
                        diag::Property_0_does_not_exist_on_type_1_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_2_or_later,
                        args![missing_property, container, lib_suggestion],
                    ));
                } else {
                    let suggestion = self
                        .get_suggested_symbol_for_nonexistent_property(prop_node, containing_type);
                    if suggestion.is_some() {
                        let suggested_name = symbol_name(&self.symbols, suggestion);
                        let message = if is_unchecked_js {
                            diag::Property_0_may_not_exist_on_type_1_Did_you_mean_2
                        } else {
                            diag::Property_0_does_not_exist_on_type_1_Did_you_mean_2
                        };
                        let mut d = new_diagnostic_chain_for_node(
                            diagnostic.take(),
                            prop_node,
                            message,
                            args![missing_property, container, suggested_name],
                        );
                        let suggestion_value_declaration = self.sym(suggestion).value_declaration;
                        if suggestion_value_declaration.is_some() {
                            d.add_related_info(Some(new_diagnostic_for_node(
                                suggestion_value_declaration,
                                diag::X_0_is_declared_here,
                                args![suggested_name],
                            )));
                        }
                        diagnostic = Some(d);
                    } else {
                        diagnostic = self.elaborate_never_intersection(
                            diagnostic.take(),
                            prop_node,
                            containing_type,
                        );
                        let message: &'static crate::diagnostics::Message = if self
                            .container_seems_to_be_empty_dom_element(containing_type)
                        {
                            diag::Property_0_does_not_exist_on_type_1_Try_changing_the_lib_compiler_option_to_include_dom
                        } else {
                            diag::Property_0_does_not_exist_on_type_1
                        };
                        diagnostic = Some(new_diagnostic_chain_for_node(
                            diagnostic.take(),
                            prop_node,
                            message,
                            args![missing_property, container],
                        ));
                    }
                }
            }
        }
        let diagnostic = diagnostic.expect("reportNonexistentProperty: diagnostic is always set");
        let is_error = !is_unchecked_js
            || diagnostic.code()
                != diag::Property_0_may_not_exist_on_type_1_Did_you_mean_2.code() as i32;
        self.add_error_or_suggestion(is_error, diagnostic);
    }

    // Go: checker/checker.go:11793 getSuggestedLibForNonExistentProperty
    pub fn get_suggested_lib_for_non_existent_property(
        &mut self,
        missing_property: &str,
        containing_type: TypeId,
    ) -> String {
        let apparent_type = self.get_apparent_type(containing_type);
        let container = self.ty(apparent_type).symbol;
        if container.is_some() {
            let feature_map = get_feature_map();
            if let Some(type_features) = feature_map.get(self.sym(container).name.as_str()) {
                for entry in type_features.iter() {
                    if entry.props.iter().any(|p| *p == missing_property) {
                        return entry.lib.to_string();
                    }
                }
            }
        }
        String::new()
    }

    // Go: checker/checker.go:11808 getSuggestedSymbolForNonexistentProperty
    pub fn get_suggested_symbol_for_nonexistent_property(
        &mut self,
        name: Node,
        containing_type: TypeId,
    ) -> SymbolId {
        let mut props = self.get_properties_of_type(containing_type).to_vec();
        let parent = name.parent();
        if is_property_access_expression(parent) {
            let mut filtered = Vec::with_capacity(props.len());
            for prop in props {
                if self.is_valid_property_access_for_completions(parent, containing_type, prop) {
                    filtered.push(prop);
                }
            }
            props = filtered;
        }
        self.get_spelling_suggestion_for_name(name.text(), &props, SymbolFlags::VALUE)
    }

    // Go: checker/checker.go:11827 isValidPropertyAccessForCompletions
    // Checks if an existing property access is valid for completions purposes.
    // @param node a property access-like node where we want to check if we can access a property.
    // This node does not need to be an access of the property we are checking.
    // e.g. in completions, this node will often be an incomplete property access node, as in `foo.`.
    // Besides providing a location (i.e. scope) used to check property accessibility, we use this node for
    // computing whether this is a `super` property access.
    // @param type the type whose property we are checking.
    // @param property the accessed property's symbol.
    pub fn is_valid_property_access_for_completions(
        &mut self,
        node: Node,
        t: TypeId,
        property: SymbolId,
    ) -> bool {
        let is_super = is_property_access_expression(node)
            && node.expression().kind() == SyntaxKind::SuperKeyword;
        self.is_property_accessible(node, is_super, false /*isWrite*/, t, property)
        // Previously we validated the 'this' type of methods but this adversely affected performance. See #31377 for more context.
    }

    // Go: checker/checker.go:11840 isPropertyAccessible
    // Checks if a property can be accessed in a location.
    // The location is given by the `node` parameter.
    // The node does not need to be a property access.
    // @param node location where to check property accessibility
    // @param isSuper whether to consider this a `super` property access, e.g. `super.foo`.
    // @param isWrite whether this is a write access, e.g. `++foo.x`.
    // @param containingType type where the property comes from.
    // @param property property symbol.
    pub fn is_property_accessible(
        &mut self,
        node: Node,
        is_super: bool,
        is_write: bool,
        containing_type: TypeId,
        property: SymbolId,
    ) -> bool {
        // Short-circuiting for improved performance.
        if self.is_type_any(containing_type) {
            return true;
        }
        // A #private property access in an optional chain is an error dealt with by the parser.
        // The checker does not check for it, so we need to do our own check here.
        let value_declaration = self.sym(property).value_declaration;
        if value_declaration.is_some()
            && is_private_identifier_class_element_declaration(value_declaration)
        {
            let decl_class = get_containing_class(value_declaration);
            return !is_optional_chain(node) && is_node_descendant_of(node, decl_class);
        }
        self.check_property_accessibility_at_location(
            node,
            is_super,
            is_write,
            containing_type,
            property,
            Node::NIL,
        )
    }

    // Go: checker/checker.go:11854 containerSeemsToBeEmptyDomElement
    pub fn container_seems_to_be_empty_dom_element(&mut self, containing_type: TypeId) -> bool {
        !self
            .compiler_options
            .lib
            .iter()
            .flatten()
            .any(|lib| lib == "lib.dom.d.ts")
            && self.every_contained_type(containing_type, &mut |c: &mut Checker, t: TypeId| {
                c.has_common_dom_type_name(t)
            })
            && self.is_empty_object_type(containing_type)
    }

    // Go: checker/checker.go:11858 hasCommonDomTypeName
    pub fn has_common_dom_type_name(&self, t: TypeId) -> bool {
        let symbol = self.ty(t).symbol;
        if symbol.is_nil() {
            return false;
        }
        let name = self.sym(symbol).name.as_str();
        name == "EventTarget"
            || name == "Node"
            || name == "Element"
            || name.starts_with("HTML") && name.ends_with("Element")
    }

    // Go: checker/checker.go:11866 checkAndReportErrorForExtendingInterface
    pub fn check_and_report_error_for_extending_interface(&mut self, error_location: Node) -> bool {
        let expression = self.get_entity_name_for_extending_interface(error_location);
        if expression.is_some()
            && self
                .resolve_entity_name(
                    expression,
                    SymbolFlags::INTERFACE,
                    true, /*ignoreErrors*/
                    false,
                    Node::NIL,
                )
                .is_some()
        {
            self.error(
                error_location,
                diag::Cannot_extend_an_interface_0_Did_you_mean_implements,
                args![get_text_of_node(expression)],
            );
            return true;
        }
        false
    }

    // Go: checker/checker.go:11878 getEntityNameForExtendingInterface
    // Climbs up parents to a heritage clause element and returns its entity name.
    pub fn get_entity_name_for_extending_interface(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::Identifier
            | SyntaxKind::QualifiedName
            | SyntaxKind::PropertyAccessExpression => {
                if node.parent().is_some() {
                    return self.get_entity_name_for_extending_interface(node.parent());
                }
            }
            SyntaxKind::TypeReference => {
                return node.type_name();
            }
            SyntaxKind::ExpressionWithTypeArguments => {
                if is_entity_name_expression(node.expression()) {
                    return node.expression();
                }
            }
            _ => {}
        }
        Node::NIL
    }

    // Go: checker/checker.go:11894 isUncalledFunctionReference
    pub fn is_uncalled_function_reference(&mut self, node: Node, symbol: SymbolId) -> bool {
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD)
        {
            let mut parent = find_ancestor(node.parent(), |n| !is_access_expression(n));
            if parent.is_nil() {
                parent = node.parent();
            }
            if is_call_like_expression(parent) {
                return is_call_or_new_expression(parent)
                    && is_identifier(node)
                    && self.has_matching_argument(parent, node);
            }
            let declarations = self.sym(symbol).declarations.clone();
            for &d in declarations.iter() {
                if !(!is_function_like(d) || self.is_deprecated_declaration(d)) {
                    return false;
                }
            }
            return true;
        }
        true
    }

    // Go: checker/checker.go:11910 checkPropertyNotUsedBeforeDeclaration
    pub fn check_property_not_used_before_declaration(
        &mut self,
        prop: SymbolId,
        node: Node,
        right: Node,
    ) {
        let value_declaration = self.sym(prop).value_declaration;
        if value_declaration.is_nil()
            || with_source_file_info(get_source_file_of_node(node), |info| {
                info.is_declaration_file
            })
        {
            return;
        }
        let mut diagnostic: Option<Diagnostic> = None;
        // PERF: chkA. Go reads `right.Text()` here; it only reads the tree,
        // so it is read when a diagnostic needs it.
        if self.is_in_property_initializer_or_class_static_block(
            node, false, /*ignoreArrowFunctions*/
        ) && !self.is_optional_property_declaration(value_declaration)
            && !(is_access_expression(node) && is_access_expression(node.expression()))
            && !self.is_block_scoped_name_declared_before_use(value_declaration, right)
            && !(is_method_declaration(value_declaration)
                && self
                    .get_combined_modifier_flags_cached(value_declaration)
                    .intersects(ModifierFlags::STATIC))
            && (self.compiler_options.get_use_define_for_class_fields()
                || !self.is_property_declared_in_ancestor_class(prop))
        {
            diagnostic = Some(new_diagnostic_for_node(
                right,
                diag::Property_0_is_used_before_its_initialization,
                args![right.text()],
            ));
        } else if is_class_declaration(value_declaration)
            && !is_type_reference_node(node.parent())
            && value_declaration
                .parser_flags(NodeFlags::AMBIENT)
                .is_empty()
            && !self.is_block_scoped_name_declared_before_use(value_declaration, right)
        {
            diagnostic = Some(new_diagnostic_for_node(
                right,
                diag::Class_0_used_before_its_declaration,
                args![right.text()],
            ));
        }
        if let Some(mut diagnostic) = diagnostic {
            diagnostic.add_related_info(Some(new_diagnostic_for_node(
                value_declaration,
                diag::X_0_is_declared_here,
                args![right.text()],
            )));
            self.add_diagnostic(diagnostic);
        }
    }

    // Go: checker/checker.go:11932 isOptionalPropertyDeclaration
    pub fn is_optional_property_declaration(&mut self, node: Node) -> bool {
        is_property_declaration(node)
            && !has_accessor_modifier(node)
            && is_question_token(node.postfix_token())
    }

    // Go: checker/checker.go:11936 isPropertyDeclaredInAncestorClass
    pub fn is_property_declared_in_ancestor_class(&mut self, prop: SymbolId) -> bool {
        let parent = self.sym(prop).parent;
        if self.sym(parent).flags.intersects(SymbolFlags::CLASS) {
            let declared_type = self.get_declared_type_of_symbol(parent);
            let base_types = self.get_base_types(declared_type);
            if !base_types.is_empty() {
                let name = self.sym(prop).name.clone();
                let super_property = self.get_property_of_type(base_types[0], &name);
                return super_property.is_some()
                    && self.sym(super_property).value_declaration.is_some();
            }
        }
        false
    }

    // Go: checker/checker.go:11954 checkPropertyAccessibility
    // Check whether the requested property access is valid.
    // Returns true if node is a valid property access, and false otherwise.
    // @param node The node to be checked.
    // @param isSuper True if the access is from `super.`.
    // @param type The type of the object whose property is being accessed. (Not the type of the property.)
    // @param prop The symbol for the property being accessed.
    pub fn check_property_accessibility(
        &mut self,
        node: Node,
        is_super: bool,
        writing: bool,
        t: TypeId,
        prop: SymbolId,
    ) -> bool {
        self.check_property_accessibility_ex(
            node, is_super, writing, t, prop, true, /*reportError*/
        )
    }

    // Go: checker/checker.go:11958 checkPropertyAccessibilityEx
    pub fn check_property_accessibility_ex(
        &mut self,
        node: Node,
        is_super: bool,
        writing: bool,
        t: TypeId,
        prop: SymbolId,
        report_error: bool, /*  = true */
    ) -> bool {
        let mut error_node = Node::NIL;
        if report_error {
            error_node = match node.kind() {
                SyntaxKind::PropertyAccessExpression => node.name(),
                SyntaxKind::QualifiedName => node.right(),
                SyntaxKind::ImportType => node,
                SyntaxKind::BindingElement => get_binding_element_property_name(node),
                _ => node.name(),
            };
        }
        self.check_property_accessibility_at_location(node, is_super, writing, t, prop, error_node)
    }

    // Go: checker/checker.go:11987 checkPropertyAccessibilityAtLocation
    // Check whether the requested property can be accessed at the requested location.
    // Returns true if node is a valid property access, and false otherwise.
    // @param location The location node where we want to check if the property is accessible.
    // @param isSuper True if the access is from `super.`.
    // @param writing True if this is a write property access, false if it is a read property access.
    // @param containingType The type of the object whose property is being accessed. (Not the type of the property.)
    // @param prop The symbol for the property being accessed.
    // @param errorNode The node where we should report an invalid property access error, or undefined if we should not report errors.
    pub fn check_property_accessibility_at_location(
        &mut self,
        location: Node,
        is_super: bool,
        writing: bool,
        containing_type: TypeId,
        prop: SymbolId,
        error_node: Node,
    ) -> bool {
        let mut containing_type = containing_type;
        let flags = self.get_declaration_modifier_flags_from_symbol_ex(prop, writing);
        if is_super {
            // TS 1.0 spec (April 2014): 4.8.2
            // - In a constructor, instance member function, instance member accessor, or
            //   instance member variable initializer where this references a derived class instance,
            //   a super property access is permitted and must specify a public instance member function of the base class.
            // - In a static member function or static member accessor
            //   where this references the constructor function object of a derived class,
            //   a super property access is permitted and must specify a public static member function of the base class.
            if flags.intersects(ModifierFlags::ABSTRACT) {
                // A method cannot be accessed in a super property access if the method is abstract.
                // This error could mask a private property access error. But, a member
                // cannot simultaneously be private and abstract, so this will trigger an
                // additional error elsewhere.
                if error_node.is_some() {
                    let prop_string = self.symbol_to_string(prop);
                    let declaring_class = self.get_declaring_class(prop);
                    let class_string = self.type_to_string(declaring_class);
                    self.error(
                        error_node,
                        diag::Abstract_method_0_in_class_1_cannot_be_accessed_via_super_expression,
                        args![prop_string, class_string],
                    );
                }
                return false;
            }
            // A class field cannot be accessed via super.* from a derived class.
            // This is true for both [[Set]] (old) and [[Define]] (ES spec) semantics.
            if !flags.intersects(ModifierFlags::STATIC)
                && self
                    .sym(prop)
                    .declarations
                    .iter()
                    .any(|&d| is_class_instance_property(d))
            {
                if error_node.is_some() {
                    let prop_string = self.symbol_to_string(prop);
                    self.error(
                        error_node,
                        diag::Class_field_0_defined_by_the_parent_class_is_not_accessible_in_the_child_class_via_super,
                        args![prop_string],
                    );
                }
                return false;
            }
        }
        // Referencing abstract properties within their own constructors is not allowed
        if flags.intersects(ModifierFlags::ABSTRACT)
            && self.symbol_has_non_method_declaration(prop)
            && (is_this_property(location)
                || is_this_initialized_object_binding_expression(location)
                || is_object_binding_pattern(location.parent())
                    && is_this_initialized_declaration(location.parent().parent()))
        {
            let parent_symbol = self.get_parent_of_symbol(prop);
            if parent_symbol.is_some()
                && self.sym(parent_symbol).flags.intersects(SymbolFlags::CLASS)
                && self.is_node_used_during_class_initialization(location)
            {
                if error_node.is_some() {
                    let prop_string = self.symbol_to_string(prop);
                    let parent_string = self.symbol_to_string(parent_symbol);
                    self.error(
                        error_node,
                        diag::Abstract_property_0_in_class_1_cannot_be_accessed_in_the_constructor,
                        args![prop_string, parent_string],
                    );
                }
                return false;
            }
        }
        // Public properties are otherwise accessible.
        if !flags.intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER) {
            return true;
        }
        // Property is known to be private or protected at this point
        // Private property is accessible if the property is within the declaring class
        if flags.intersects(ModifierFlags::PRIVATE) {
            let mut declaring_class_declaration = Node::NIL;
            let parent = self.get_parent_of_symbol(prop);
            if parent.is_some() {
                declaring_class_declaration =
                    get_class_like_declaration_of_symbol(&self.symbols, parent);
            }
            if declaring_class_declaration.is_nil()
                || !self.is_node_within_class(location, declaring_class_declaration)
            {
                if error_node.is_some() {
                    let mut class = self.get_declaring_class(prop);
                    if class.is_nil() {
                        class = containing_type;
                    }
                    let prop_string = self.symbol_to_string(prop);
                    let class_string = self.type_to_string(class);
                    self.error(
                        error_node,
                        diag::Property_0_is_private_and_only_accessible_within_class_1,
                        args![prop_string, class_string],
                    );
                }
                return false;
            }
            return true;
        }
        // Property is known to be protected at this point
        // All protected properties of a supertype are accessible in a super access
        if is_super {
            return true;
        }
        // Find the first enclosing class that has the declaring classes of the protected constituents
        // of the property as base classes
        let mut enclosing_class = TypeId::NIL;
        let mut container = get_containing_class(location);
        while container.is_some() {
            let container_symbol = self.get_symbol_of_declaration(container);
            let class = self.get_declared_type_of_symbol(container_symbol);
            if self.is_class_derived_from_declaring_classes(class, prop, writing) {
                enclosing_class = class;
                break;
            }
            container = get_containing_class(container);
        }
        // A protected property is accessible if the property is within the declaring class or classes derived from it
        if enclosing_class.is_nil() {
            // allow PropertyAccessibility if context is in function with this parameter
            // static member access is disallowed
            let class = self.get_enclosing_class_from_this_parameter(location);
            if class.is_some() && self.is_class_derived_from_declaring_classes(class, prop, writing)
            {
                enclosing_class = class;
            }
            if flags.intersects(ModifierFlags::STATIC) || enclosing_class.is_nil() {
                if error_node.is_some() {
                    let mut class = self.get_declaring_class(prop);
                    if class.is_nil() {
                        class = containing_type;
                    }
                    let prop_string = self.symbol_to_string(prop);
                    let class_string = self.type_to_string(class);
                    self.error(
                        error_node,
                        diag::Property_0_is_protected_and_only_accessible_within_class_1_and_its_subclasses,
                        args![prop_string, class_string],
                    );
                }
                return false;
            }
        }
        // No further restrictions for static properties
        if flags.intersects(ModifierFlags::STATIC) {
            return true;
        }
        if self
            .ty(containing_type)
            .flags
            .intersects(TypeFlags::TYPE_PARAMETER)
        {
            // get the original type -- represented as the type constraint of the 'this' type
            if self.ty(containing_type).as_type_parameter().is_this_type {
                containing_type = self.get_constraint_of_type_parameter(containing_type);
            } else {
                containing_type = self.get_base_constraint_of_type(containing_type);
            }
        }
        if containing_type.is_nil() || !self.has_base_type(containing_type, enclosing_class) {
            if error_node.is_some() && containing_type.is_some() {
                let prop_string = self.symbol_to_string(prop);
                let enclosing_string = self.type_to_string(enclosing_class);
                let containing_string = self.type_to_string(containing_type);
                self.error(
                    error_node,
                    diag::Property_0_is_protected_and_only_accessible_through_an_instance_of_class_1_This_is_an_instance_of_class_2,
                    args![prop_string, enclosing_string, containing_string],
                );
            }
            return false;
        }
        true
    }

    // Go: checker/checker.go:12102 symbolHasNonMethodDeclaration
    pub fn symbol_has_non_method_declaration(&mut self, symbol: SymbolId) -> bool {
        self.for_each_property(symbol, &mut |c: &mut Checker, prop: SymbolId| {
            !c.sym(prop).flags.intersects(SymbolFlags::METHOD)
        })
    }

    // Go: checker/checker.go:12108 forEachProperty
    // Invoke the callback for each underlying property symbol of the given symbol and return the first
    // value that isn't undefined.
    pub fn for_each_property(
        &mut self,
        prop: SymbolId,
        callback: &mut dyn FnMut(&mut Checker, SymbolId) -> bool,
    ) -> bool {
        if !self.sym(prop).check_flags.intersects(CheckFlags::SYNTHETIC) {
            return callback(self, prop);
        }
        let containing_type = self
            .value_symbol_links
            .get_by_id(&self.symbols, prop)
            .containing_type;
        let types = self.ty(containing_type).types_list();
        let name = self.sym(prop).name.clone();
        for t in types {
            let p = self.get_property_of_type(t, &name);
            if p.is_some() && self.for_each_property(p, callback) {
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:12122 getDeclaringClass
    // Return the declaring class type of a property or undefined if property not declared in class
    pub fn get_declaring_class(&mut self, prop: SymbolId) -> TypeId {
        let parent = self.sym(prop).parent;
        if parent.is_some() && self.sym(parent).flags.intersects(SymbolFlags::CLASS) {
            let parent_of_symbol = self.get_parent_of_symbol(prop);
            return self.get_declared_type_of_symbol(parent_of_symbol);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:12130 isValidOverrideOf
    // Return true if source property is a valid override of protected parts of target property.
    pub fn is_valid_override_of(&mut self, source_prop: SymbolId, target_prop: SymbolId) -> bool {
        !self.for_each_property(target_prop, &mut |c: &mut Checker, tp: SymbolId| {
            if c.get_declaration_modifier_flags_from_symbol(tp)
                .intersects(ModifierFlags::PROTECTED)
            {
                let declaring_class = c.get_declaring_class(tp);
                return !c.is_property_in_class_derived_from(source_prop, declaring_class);
            }
            false
        })
    }

    // Go: checker/checker.go:12141 isPropertyInClassDerivedFrom
    // Return true if some underlying source property is declared in a class that derives
    // from the given base class.
    pub fn is_property_in_class_derived_from(
        &mut self,
        prop: SymbolId,
        base_class: TypeId,
    ) -> bool {
        self.for_each_property(prop, &mut |c: &mut Checker, sp: SymbolId| {
            let source_class = c.get_declaring_class(sp);
            if source_class.is_some() {
                return c.has_base_type(source_class, base_class);
            }
            false
        })
    }

    // Go: checker/checker.go:12151 isNodeUsedDuringClassInitialization
    pub fn is_node_used_during_class_initialization(&mut self, node: Node) -> bool {
        find_ancestor_or_quit(node, |element| {
            if is_constructor_declaration(element) && node_is_present(element.body())
                || is_property_declaration(element)
            {
                FindAncestorResult::FIND_ANCESTOR_TRUE
            } else if is_class_like(element) || is_function_like_declaration(element) {
                FindAncestorResult::FIND_ANCESTOR_QUIT
            } else {
                FindAncestorResult::FIND_ANCESTOR_FALSE
            }
        })
        .is_some()
    }

    // Go: checker/checker.go:12162 isNodeWithinClass
    pub fn is_node_within_class(&mut self, node: Node, class_declaration: Node) -> bool {
        self.for_each_enclosing_class(node, &mut |_c: &mut Checker, n: Node| {
            n == class_declaration
        })
    }

    // Go: checker/checker.go:12166 forEachEnclosingClass
    pub fn for_each_enclosing_class(
        &mut self,
        node: Node,
        callback: &mut dyn FnMut(&mut Checker, Node) -> bool,
    ) -> bool {
        let mut containing_class = get_containing_class(node);
        while containing_class.is_some() {
            let result = callback(self, containing_class);
            if result {
                return true;
            }
            containing_class = get_containing_class(containing_class);
        }
        false
    }

    // Go: checker/checker.go:12180 isClassDerivedFromDeclaringClasses
    // Return true if the given class derives from each of the declaring classes of the protected
    // constituents of the given property.
    pub fn is_class_derived_from_declaring_classes(
        &mut self,
        check_class: TypeId,
        prop: SymbolId,
        writing: bool,
    ) -> bool {
        !self.for_each_property(prop, &mut |c: &mut Checker, p: SymbolId| {
            if c.get_declaration_modifier_flags_from_symbol_ex(p, writing)
                .intersects(ModifierFlags::PROTECTED)
            {
                let declaring_class = c.get_declaring_class(p);
                return !c.has_base_type(check_class, declaring_class);
            }
            false
        })
    }

    // Go: checker/checker.go:12189 getEnclosingClassFromThisParameter
    pub fn get_enclosing_class_from_this_parameter(&mut self, node: Node) -> TypeId {
        // 'this' type for a node comes from, in priority order...
        // 1. The type of a syntactic 'this' parameter in the enclosing function scope
        let this_parameter = get_this_parameter_from_node_context(node);
        let mut this_type = TypeId::NIL;
        if this_parameter.is_some() && this_parameter.type_().is_some() {
            this_type = self.get_type_from_type_node(this_parameter.type_());
        }
        if this_type.is_some() {
            // 2. The constraint of a type parameter used for an explicit 'this' parameter
            if self
                .ty(this_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
            {
                this_type = self.get_constraint_of_type_parameter(this_type);
            }
        } else {
            // 3. The 'this' parameter of a contextual type
            let this_container = get_this_container(
                node, false, /*includeArrowFunctions*/
                false, /*includeClassComputedPropertyName*/
            );
            if this_container.is_some() && is_function_like(this_container) {
                this_type = self.get_contextual_this_parameter_type(this_container);
            }
        }
        if this_type.is_some()
            && self
                .ty(this_type)
                .object_flags
                .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
        {
            return self.get_target_type(this_type);
        }
        TypeId::NIL
    }
}

// Go: checker/checker.go:12215 getThisParameterFromNodeContext
pub fn get_this_parameter_from_node_context(node: Node) -> Node {
    let this_container = get_this_container(
        node, false, /*includeArrowFunctions*/
        false, /*includeClassComputedPropertyName*/
    );
    if this_container.is_some() && is_function_like(this_container) {
        return get_this_parameter(this_container);
    }
    Node::NIL
}

impl Checker {
    // Go: checker/checker.go:12223 getContextualThisParameterType
    pub fn get_contextual_this_parameter_type(&mut self, func: Node) -> TypeId {
        if is_arrow_function(func) {
            return TypeId::NIL;
        }
        if self.is_context_sensitive_function_or_object_literal_method(func) {
            let contextual_signature = self.get_contextual_signature(func);
            if contextual_signature.is_some() {
                let this_parameter = self.sig(contextual_signature).this_parameter;
                if this_parameter.is_some() {
                    return self.get_type_of_symbol(this_parameter);
                }
            }
        }
        let in_js = is_in_js_file(func);
        if self.no_implicit_this || in_js {
            let containing_literal = get_containing_object_literal(func);
            if containing_literal.is_some() {
                // We have an object literal method. Check if the containing object literal has a contextual type
                // that includes a ThisType<T>. If so, T is the contextual type for 'this'. We continue looking in
                // any directly enclosing object literals.
                let contextual_type = self
                    .get_apparent_type_of_contextual_type(containing_literal, ContextFlags::NONE);
                let mut this_type = self.get_this_type_of_object_literal_from_contextual_type(
                    containing_literal,
                    contextual_type,
                );
                if this_type.is_some() {
                    let inference_context = self.get_inference_context(containing_literal);
                    let mapper = self.get_mapper_from_context(inference_context);
                    return self.instantiate_type(this_type, mapper);
                }
                // There was no contextual ThisType<T> for the containing object literal, so the contextual type
                // for 'this' is the non-null form of the contextual type for the containing object literal or
                // the type of the object literal itself.
                if contextual_type.is_some() {
                    this_type = self.get_non_nullable_type(contextual_type);
                } else {
                    this_type = self.check_expression_cached(containing_literal);
                }
                return self.get_widened_type(this_type);
            }
            // In an assignment of the form 'obj.xxx = function(...)' or 'obj[xxx] = function(...)', the
            // contextual type for 'this' is 'obj'.
            let parent = walk_up_parenthesized_expressions(func.parent());
            if is_assignment_expression(parent, false) {
                let target = parent.left();
                if is_access_expression(target) {
                    let expression = target.expression();
                    // Don't contextually type `this` as `exports` in `exports.Point = function(x, y) { this.x = x; this.y = y; }`
                    if in_js && is_identifier(expression) {
                        let source_file = get_source_file_of_node(parent);
                        if with_source_file_info(source_file, |info| {
                            info.common_js_module_indicator
                        })
                        .is_some()
                        {
                            let resolved = self.get_resolved_symbol(expression);
                            if self
                                .sym(resolved)
                                .flags
                                .intersects(SymbolFlags::MODULE_EXPORTS)
                            {
                                return TypeId::NIL;
                            }
                        }
                    }
                    let expression_type = self.check_expression_cached(expression);
                    return self.get_widened_type(expression_type);
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:12279 checkThisExpression
    pub fn check_this_expression(&mut self, node: Node) -> TypeId {
        // Stop at the first arrow function so that we can
        // tell whether 'this' needs to be captured.
        let mut container = get_this_container(
            node, true, /*includeArrowFunctions*/
            true, /*includeClassComputedPropertyName*/
        );
        let mut captured_by_arrow_function = false;
        let mut this_in_computed_property_name = false;
        if is_constructor_declaration(container) {
            self.check_this_before_super(
                node,
                container,
                diag::X_super_must_be_called_before_accessing_this_in_the_constructor_of_a_derived_class,
            );
        }
        loop {
            // Now skip arrow functions to get the "real" owner of 'this'.
            if is_arrow_function(container) {
                container = get_this_container(
                    container,
                    false, /*includeArrowFunctions*/
                    !this_in_computed_property_name,
                );
                captured_by_arrow_function = true;
            }
            if is_computed_property_name(container) {
                container = get_this_container(
                    container,
                    !captured_by_arrow_function,
                    false, /*includeClassComputedPropertyName*/
                );
                this_in_computed_property_name = true;
                continue;
            }
            break;
        }
        self.check_this_in_static_class_field_initializer_in_decorated_class(node, container);
        if this_in_computed_property_name {
            self.error(
                node,
                diag::X_this_cannot_be_referenced_in_a_computed_property_name,
                args![],
            );
        } else {
            match container.kind() {
                SyntaxKind::ModuleDeclaration => {
                    self.error(
                        node,
                        diag::X_this_cannot_be_referenced_in_a_module_or_namespace_body,
                        args![],
                    );
                    // do not return here so in case if lexical this is captured - it will be reflected in flags on NodeLinks
                }
                SyntaxKind::EnumDeclaration => {
                    self.error(
                        node,
                        diag::X_this_cannot_be_referenced_in_current_location,
                        args![],
                    );
                    // do not return here so in case if lexical this is captured - it will be reflected in flags on NodeLinks
                }
                _ => {}
            }
        }
        let t = self.try_get_this_type_at_ex(node, true /*includeGlobalThis*/, container);
        if self.no_implicit_this {
            let global_this_type = self.get_type_of_symbol(self.global_this_symbol);
            if t == global_this_type && captured_by_arrow_function {
                self.error(
                    node,
                    diag::The_containing_arrow_function_captures_the_global_value_of_this,
                    args![],
                );
            } else if t.is_nil() {
                // With noImplicitThis, functions may not reference 'this' if it has type 'any'
                // PORT: Go mutates the diagnostic returned by c.error. Here the diagnostic is built
                // first, tryGetThisTypeAt runs, and then the diagnostic is added. Diagnostics that
                // tryGetThisTypeAt reports therefore come before this one in the collection.
                let mut diagnostic = new_diagnostic_for_node(
                    node,
                    diag::X_this_implicitly_has_type_any_because_it_does_not_have_a_type_annotation,
                    args![],
                );
                if !is_source_file(container) {
                    let outside_this = self.try_get_this_type_at(container);
                    if outside_this.is_some() && outside_this != global_this_type {
                        diagnostic.add_related_info(Some(create_diagnostic_for_node(
                            container,
                            diag::An_outer_value_of_this_is_shadowed_by_this_container,
                            args![],
                        )));
                    }
                }
                self.add_diagnostic(diagnostic);
            }
        }
        if t.is_nil() {
            return self.any_type;
        }
        t
    }
}

/// Slots of `Checker::control_flow_containers` (16 KiB). On T3 Code, the
/// gate projects and 5 realworld4 repos, the misses of 1024 slots are at most
/// 2 points more than those of a memo with no limit (effect: 38% against
/// 23%). 256 slots cost effect 0.07% more instructions; a `LinkStore` memo
/// costs more instructions than this table everywhere except effect.
pub const CONTROL_FLOW_CONTAINER_SLOTS: usize = 1024;

/// The slot of `node` in `Checker::control_flow_containers`: the high bits
/// of a Fibonacci hash of the handle (file and slot index).
#[inline]
fn control_flow_container_slot(node: Node) -> usize {
    const BITS: u32 = CONTROL_FLOW_CONTAINER_SLOTS.trailing_zeros();
    (node.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - BITS)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go `getSymbolNameForPrivateIdentifier` (binder/binder.go) writes the
    // class symbol's id into the name of each private identifier symbol.
    // Each file binds into its own arena here, and
    // `SymbolArena::prepare_file_arena` moves the ids, and so these names,
    // when the arena joins the program after the lib files. The members and
    // exports tables must then find the moved names: their hash filter and
    // index are built again (corefix1). The checker finds each private name
    // as Go `lookupSymbolForPrivateIdentifierDeclaration` (checker.go:11674)
    // does, in tables of 2, 8, 9 and 41 entries (with no index and with
    // one).
    #[test]
    fn private_names_are_found_after_the_file_arena_moves() {
        let mut source = String::new();
        for (class, size) in [1, 7, 8, 40].into_iter().enumerate() {
            source.push_str(&format!("export class C{class} {{\n"));
            for k in 0..size {
                source.push_str(&format!("    #p{k} = {k};\n    static #s{k} = {k};\n"));
            }
            source.push_str("    m() {}\n}\n");
        }
        let dir =
            std::env::temp_dir().join(format!("ts_goport_private_names_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), source).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "target": "es2022", "types": [] }, "files": ["a.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let file = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/a.ts"))
            .expect("a.ts is not in the program")
            .root;
        let (found, missing) = crate::program::with_type_checker_for_file(file, move |checker| {
            let (mut found, mut missing) = (0, Vec::new());
            for class in file.statements().iter() {
                let method = class.members().iter().last().expect("m");
                for member in class.members().iter() {
                    let name = member.name();
                    if !is_private_identifier(name) {
                        continue;
                    }
                    let symbol = checker
                        .lookup_symbol_for_private_identifier_declaration(name.text(), method);
                    if symbol.is_some() && symbol == checker.get_symbol_of_declaration(member) {
                        found += 1;
                    } else {
                        missing.push(name.text().to_string());
                    }
                }
            }
            (found, missing)
        });
        drop(scope);
        crate::program::release_program(program);
        assert!(missing.is_empty(), "not found: {missing:?}");
        assert_eq!(found, 2 * (1 + 7 + 8 + 40));
    }

    /// Go `getControlFlowContainer` (checker.go:11638) as written: a walk of
    /// `node.Parent` and its parents with no memo.
    fn go_control_flow_container(node: Node) -> Node {
        let mut n = node.parent();
        while n.is_some() {
            if is_function_like(n) && get_immediately_invoked_function_expression(n).is_nil()
                || is_module_block(n)
                || is_source_file(n)
                || is_property_declaration(n)
            {
                return n;
            }
            n = n.parent();
        }
        Node::NIL
    }

    // cfcache1: `get_control_flow_container` keeps answers in a small table.
    // On every node of a TS file, a JS file and lib.es5.d.ts, in tree order
    // and then in reverse (so that slots are taken again by other nodes),
    // each answer must be the walk's answer, from the table or not.
    #[test]
    fn control_flow_container_memo_gives_the_walk_answer() {
        let a = r#"
namespace N { export const x = 1; function f() { return x; } namespace M { export let y = x; } }
declare module "m" { export const z: number; }
class C {
    p = 1;
    q = () => this.p;
    static s = (function () { return 1; })();
    static { const b = C.s; }
    constructor(public w: number) { const v = ((() => w))(); }
    m(this: C) { const v = [1].map(a => a + this.p); return v; }
    get g() { return 1; }
    set g(v: number) { this.p = v; }
}
const iife = (() => { const inner = 1; return inner; })();
const iife2 = (function named() { return 2; }());
const iife3 = ((async () => 3))();
const obj = { method() { return 1; }, arrow: () => 2, get acc() { return 3; }, nested: { deep() { return () => 4; } } };
function outer(a: number) {
    function inner() { return a; }
    label: for (const k of [1]) { if (k) { break label; } }
    return [1, 2].map(function (b) { return b + a + inner(); });
}
function* gen() { yield 1; }
enum E { A = 1, B = A + 1 }
type T<U> = U extends string ? { [K in keyof U]: U[K] } : never;
interface I { (x: number): string; new (y: string): I; method?(): void; prop: (z: number) => void; }
const cls = class { field = (() => 1)(); m() { return this.field; } };
export default class { field = 1; }
"#;
        let b = r#"
/** @param {number} a @returns {number} */
function j(a) { return a + 1; }
/** @type {(x: number) => number} */
const k = (x) => x;
module.exports.k = k;
(function () { var hidden = 1; return hidden; })();
"#;
        let dir = std::env::temp_dir().join(format!("ts_goport_cfcache_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), a).unwrap();
        std::fs::write(dir.join("b.js"), b).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "target": "es2022", "types": [], "allowJs": true, "checkJs": true, "noEmit": true }, "files": ["a.ts", "b.js"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let roots: Vec<Node> = program
            .source_files()
            .filter(|file| {
                let name = &file.info.file_name;
                name.ends_with("/a.ts")
                    || name.ends_with("/b.js")
                    || name.ends_with("/lib.es5.d.ts")
            })
            .map(|file| file.root)
            .collect();
        assert_eq!(roots.len(), 3, "a.ts, b.js and lib.es5.d.ts");
        let mut nodes = Vec::new();
        fn collect(node: Node, nodes: &mut Vec<Node>) {
            nodes.push(node);
            node.for_each_child(|child| {
                collect(child, nodes);
                false
            });
        }
        for &root in &roots {
            collect(root, &mut nodes);
        }
        let expected: Vec<Node> = nodes
            .iter()
            .map(|&n| go_control_flow_container(n))
            .collect();
        let total = nodes.len();
        let (wrong, kept) = crate::program::with_type_checker_for_file(roots[0], move |checker| {
            let (mut wrong, mut kept) = (Vec::new(), 0usize);
            let mut check = |checker: &mut Checker, i: usize| {
                let got = checker.get_control_flow_container(nodes[i]);
                if got != expected[i] {
                    wrong.push((nodes[i], got, expected[i]));
                }
                if checker.control_flow_containers[control_flow_container_slot(nodes[i])]
                    == (nodes[i], expected[i])
                {
                    kept += 1;
                }
            };
            for i in 0..nodes.len() {
                check(checker, i);
                check(checker, i);
            }
            for i in (0..nodes.len()).rev() {
                check(checker, i);
            }
            (wrong, kept)
        });
        drop(scope);
        crate::program::release_program(program);
        assert!(
            wrong.is_empty(),
            "{} wrong answers, first: {:?}",
            wrong.len(),
            wrong.first()
        );
        // More nodes than slots, so slots are taken again. Only the 3 roots
        // (no parent) are not kept: 10,673 nodes, 32,010 of 32,019 checks.
        assert!(total > 4 * CONTROL_FLOW_CONTAINER_SLOTS, "{total} nodes");
        assert!(
            kept * 10 >= total * 3 * 9,
            "the table kept {kept} of {} answers",
            total * 3
        );
    }

    // cfcache1 (R183 reviewer item 4): the table keeps an answer only from
    // a store whose parents are all in that store (`parents_local`). The
    // test of a function-like node reads its parent, which can be outside
    // the store when some parent of the store is. Stores `a` and `c` each
    // hold `x;` in a module block; in `a` the parent of that block is a
    // node of store `b`. Both walks end at the block, but only the answer
    // from `c` is kept. It publishes, so no other test may build or publish
    // stores while it runs (the runner uses one thread).
    #[test]
    fn control_flow_container_memo_keeps_only_stores_with_local_parents() {
        let a = new_file_store("/cfcache_local/a.ts", "x;");
        let b = new_file_store("/cfcache_local/b.ts", "y;");
        let c = new_file_store("/cfcache_local/c.ts", "x;");
        let module_block_of_x = |file: usize| {
            let f = NodeFactory::for_file(file);
            let x = f.new_identifier("x");
            let statement = f.new_expression_statement(x);
            let block = f.new_module_block(f.new_node_list(&[statement]));
            set_node_parent(x, statement);
            set_node_parent(statement, block);
            (x, block)
        };
        let (x_a, block_a) = module_block_of_x(a);
        let (x_c, block_c) = module_block_of_x(c);
        let y = NodeFactory::for_file(b).new_identifier("y");
        set_node_parent(block_a, y);
        for file in [a, b, c] {
            freeze_file_store(file);
        }
        crate::program::publish_parsed_files("/");
        let parents_local = |file| frozen_store_facts(file).map(|facts| facts.parents_local);
        assert_eq!(parents_local(a), Some(false));
        assert_eq!(parents_local(c), Some(true));
        assert_eq!(go_control_flow_container(x_a), block_a);
        assert_eq!(go_control_flow_container(x_c), block_c);

        let dir =
            std::env::temp_dir().join(format!("ts_goport_cfcache_local_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.ts"), "export {};\n").unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "types": [], "noEmit": true }, "files": ["main.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let root = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/main.ts"))
            .expect("main.ts is not in the program")
            .root;
        let answers = crate::program::with_type_checker_for_file(root, move |checker| {
            [x_a, x_c].map(|x| {
                let got = checker.get_control_flow_container(x);
                let kept =
                    checker.control_flow_containers[control_flow_container_slot(x)] == (x, got);
                (got, kept)
            })
        });
        drop(scope);
        crate::program::release_program(program);
        assert_eq!(answers, [(block_a, false), (block_c, true)]);
    }
}
