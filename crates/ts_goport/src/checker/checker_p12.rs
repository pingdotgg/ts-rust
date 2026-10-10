//! Port of `checker/checker.go` lines 10269-11171 (createUnionSignature
//! through checkIdentifier).

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: Go `getInstantiationExpressionType` keeps its state in closure
// variables shared by nested recursive closures. This struct holds that
// shared state; the closures are the `instantiation_expression_*` methods.
struct InstantiationExpressionTypeState {
    node: Node,
    type_arguments: Vec<Node>,
    has_some_applicable_signature: bool,
    non_applicable_type: TypeId,
}

impl Checker {
    // Go: checker/checker.go:10492 createUnionSignature
    pub fn create_union_signature(
        &mut self,
        sig: SignatureId,
        union_signatures: &[SignatureId],
    ) -> SignatureId {
        let result = self.clone_signature(sig);
        let s = self.sig_mut(result);
        s.composite = Some(Rc::new(CompositeSignature {
            is_union: true,
            signatures: union_signatures.to_vec(),
        }));
        s.target = SignatureId::NIL;
        s.mapper = MapperId::NIL;
        result
    }

    // If the given type is an object or union type with a single signature, and if that signature has at
    // least as many parameters as the given function, return the signature. Otherwise return undefined.
    // Go: checker/checker.go:10502 getContextualCallSignature
    pub fn get_contextual_call_signature(&mut self, t: TypeId, node: Node) -> SignatureId {
        let signatures = self.get_signatures_of_type(t, SignatureKind::CALL);
        let mut applicable_by_arity: Vec<SignatureId> = Vec::new();
        for s in signatures.iter().copied() {
            if !self.is_arity_smaller(s, node) {
                applicable_by_arity.push(s);
            }
        }
        if applicable_by_arity.len() == 1 {
            return applicable_by_arity[0];
        }
        self.get_intersected_signatures(&applicable_by_arity)
    }

    // Go: checker/checker.go:10511 getIntersectedSignatures
    pub fn get_intersected_signatures(&mut self, signatures: &[SignatureId]) -> SignatureId {
        if !self.no_implicit_any {
            return SignatureId::NIL;
        }
        let mut combined = SignatureId::NIL;
        for sig in signatures.iter().copied() {
            if combined == sig || combined.is_nil() {
                combined = sig;
            } else {
                let combined_type_parameters = self.sig(combined).type_parameters.clone();
                let sig_type_parameters = self.sig(sig).type_parameters.clone();
                if self.compare_type_parameters_identical(
                    &combined_type_parameters,
                    &sig_type_parameters,
                ) {
                    combined = self.combine_union_or_intersection_member_signatures(
                        combined, sig, false, /*isUnion*/
                    );
                } else {
                    return SignatureId::NIL;
                }
            }
        }
        combined
    }

    /** If the contextual signature has fewer parameters than the function expression, do not use it */
    // Go: checker/checker.go:10530 isAritySmaller
    pub fn is_arity_smaller(&mut self, signature: SignatureId, target: Node) -> bool {
        let parameters = target.parameters();
        let mut target_parameter_count: i32 = 0;
        while (target_parameter_count as usize) < parameters.len() {
            let param = parameters.get(target_parameter_count as usize);
            if param.initializer().is_some()
                || param.question_token().is_some()
                || has_dot_dot_dot_token(param)
            {
                break;
            }
            target_parameter_count += 1;
        }
        if parameters.len() != 0 && is_this_parameter(parameters.get(0)) {
            target_parameter_count -= 1;
        }
        !self.has_effective_rest_parameter(signature)
            && self.get_parameter_count(signature) < target_parameter_count
    }

    // Go: checker/checker.go:10546 assignContextualParameterTypes
    pub fn assign_contextual_parameter_types(&mut self, sig: SignatureId, context: SignatureId) {
        if !self.sig(context).type_parameters.is_empty() {
            if !self.sig(sig).type_parameters.is_empty() {
                // This signature has already has a contextual inference performed and cached on it
                return;
            }
            let context_type_parameters = self.sig(context).type_parameters.clone();
            let origin = self.share_type_parameters_origin(context);
            let s = self.sig_mut(sig);
            s.type_parameters = context_type_parameters;
            s.type_parameters_origin = origin;
        }
        let context_this_parameter = self.sig(context).this_parameter;
        if context_this_parameter.is_some() {
            let parameter = self.sig(sig).this_parameter;
            let parameter_value_declaration = if parameter.is_some() {
                self.sym(parameter).value_declaration
            } else {
                Node::NIL
            };
            if parameter.is_nil()
                || parameter_value_declaration.is_some()
                    && parameter_value_declaration.type_().is_nil()
            {
                if parameter.is_nil() {
                    let this_parameter = self
                        .create_symbol_with_type(context_this_parameter, TypeId::NIL /*type*/);
                    self.sig_mut(sig).this_parameter = this_parameter;
                }
                let this_parameter = self.sig(sig).this_parameter;
                let t = self.get_type_of_symbol(context_this_parameter);
                self.assign_parameter_type(this_parameter, t);
            }
        }
        let has_rest = self.signature_has_rest_parameter(sig);
        let length = self.sig(sig).parameters.len() - if has_rest { 1 } else { 0 };
        for i in 0..length {
            let parameter = self.sig(sig).parameters[i];
            let declaration = self.sym(parameter).value_declaration;
            if declaration.type_().is_nil() {
                let mut t = self.try_get_type_at_position(context, i as i32);
                if t.is_some() && declaration.initializer().is_some() {
                    let mut initializer_type = self.check_declaration_initializer(
                        declaration,
                        CheckMode::NORMAL,
                        TypeId::NIL,
                    );
                    if !self.is_type_assignable_to(initializer_type, t) {
                        initializer_type = self
                            .widen_type_inferred_from_initializer(declaration, initializer_type);
                        if self.is_type_assignable_to(t, initializer_type) {
                            t = initializer_type;
                        }
                    }
                }
                self.assign_parameter_type(parameter, t);
            }
        }
        if self.signature_has_rest_parameter(sig) {
            // parameter might be a transient symbol generated by use of `arguments` in the function body.
            let parameter = self.sig(sig).parameters.last().copied().unwrap_or_default();
            let value_declaration = self.sym(parameter).value_declaration;
            let check_flags = self.sym(parameter).check_flags;
            if value_declaration.is_some() && value_declaration.type_().is_nil()
                || value_declaration.is_nil() && check_flags.intersects(CheckFlags::DEFERRED_TYPE)
            {
                let contextual_parameter_type =
                    self.get_rest_type_at_position(context, length as i32, false);
                self.assign_parameter_type(parameter, contextual_parameter_type);
            }
        }
    }

    // Go: checker/checker.go:10592 assignNonContextualParameterTypes
    pub fn assign_non_contextual_parameter_types(&mut self, signature: SignatureId) {
        let this_parameter = self.sig(signature).this_parameter;
        if this_parameter.is_some() {
            self.assign_parameter_type(this_parameter, TypeId::NIL);
        }
        let parameters = self.sig(signature).parameters.clone();
        for parameter in parameters {
            self.assign_parameter_type(parameter, TypeId::NIL);
        }
    }

    // Go: checker/checker.go:10601 assignParameterType
    pub fn assign_parameter_type(&mut self, parameter: SymbolId, contextual_type: TypeId) {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, parameter)
            .resolved_type
            .is_some()
        {
            return;
        }
        let declaration = self.sym(parameter).value_declaration;
        let mut t = contextual_type;
        if t.is_nil() {
            if declaration.is_some() {
                t = self.get_widened_type_for_variable_like_declaration(
                    declaration,
                    true, /*reportErrors*/
                );
            } else {
                t = self.get_type_of_symbol(parameter);
            }
        }
        let is_optional = declaration.is_some()
            && declaration.initializer().is_nil()
            && is_optional_declaration(declaration);
        let resolved_type = self.add_optionality_ex(t, false, is_optional);
        self.value_symbol_links
            .get_by_id(&self.symbols, parameter)
            .resolved_type = resolved_type;
        if declaration.is_some() && !is_identifier(declaration.name()) {
            // if inference didn't come up with anything but unknown, fall back to the binding pattern if present.
            if self
                .value_symbol_links
                .get_by_id(&self.symbols, parameter)
                .resolved_type
                == self.unknown_type
            {
                let binding_pattern_type =
                    self.get_type_from_binding_pattern(declaration.name(), false, false);
                self.value_symbol_links
                    .get_by_id(&self.symbols, parameter)
                    .resolved_type = binding_pattern_type;
            }
            let resolved_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, parameter)
                .resolved_type;
            self.assign_binding_element_types(declaration.name(), resolved_type);
        }
    }

    // When contextual typing assigns a type to a parameter that contains a binding pattern, we also need to push
    // the destructured type into the contained binding elements.
    // Go: checker/checker.go:10627 assignBindingElementTypes
    pub fn assign_binding_element_types(&mut self, pattern: Node, parent_type: TypeId) {
        for element in pattern.elements().iter() {
            let name = element.name();
            if name.is_some() {
                let t = self.get_binding_element_type_from_parent_type(
                    element,
                    parent_type,
                    false, /*noTupleBoundsCheck*/
                );
                if is_identifier(name) {
                    let symbol = self.get_symbol_of_declaration(element);
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .resolved_type = t;
                } else {
                    self.assign_binding_element_types(name, t);
                }
            }
        }
    }

    // Go: checker/checker.go:10641 checkCollisionsForDeclarationName
    pub fn check_collisions_for_declaration_name(&mut self, node: Node, name: Node) {
        if name.is_nil() {
            return;
        }
        self.check_collision_with_require_exports_in_generated_code(node, name);
        self.check_collision_with_global_object_in_generated_code(node, name);
        self.check_collision_with_global_promise_in_generated_code(node, name);
        self.record_potential_collision_with_weak_map_set_in_generated_code(node, name);
        self.record_potential_collision_with_reflect_in_generated_code(node, name);
        if is_class_like(node) {
            self.check_type_name_is_reserved(name, diag::Class_name_cannot_be_0);
            if !node.flags().intersects(NodeFlags::AMBIENT) {
                self.check_class_name_collision_with_object(name);
            }
        } else if is_enum_declaration(node) {
            self.check_type_name_is_reserved(name, diag::Enum_name_cannot_be_0);
        }
    }

    // Go: checker/checker.go:10660 checkCollisionWithRequireExportsInGeneratedCode
    pub fn check_collision_with_require_exports_in_generated_code(
        &mut self,
        node: Node,
        name: Node,
    ) {
        // No need to check for require or exports for ES6 modules and later
        if get_emit_module_format_of_file(get_source_file_of_node(node)) >= ModuleKind::ES2015 {
            return;
        }
        if name.is_nil()
            || !self.need_collision_check_for_identifier(node, name, "require")
                && !self.need_collision_check_for_identifier(node, name, "exports")
        {
            return;
        }
        // Uninstantiated modules shouldnt do this check
        if is_module_declaration(node)
            && get_module_instance_state(node) != ModuleInstanceState::INSTANTIATED
        {
            return;
        }
        // In case of variable declaration, node.parent is variable statement so look at the variable statement's parent
        let parent = get_declaration_container(node);
        if is_source_file(parent) && is_external_or_common_js_module(parent) {
            // If the declaration happens to be in external module, report error that require and exports are reserved keywords
            self.error_skipped_on_no_emit(
                name,
                diag::Duplicate_identifier_0_Compiler_reserves_name_1_in_top_level_scope_of_a_module,
                args![declaration_name_to_string(name), declaration_name_to_string(name)],
            );
        }
    }

    // Go: checker/checker.go:10680 checkCollisionWithGlobalObjectInGeneratedCode
    pub fn check_collision_with_global_object_in_generated_code(&mut self, node: Node, name: Node) {
        if name.is_nil()
            || is_class_like(node)
            || !self.need_collision_check_for_identifier(node, name, "Object")
        {
            return;
        }
        // Uninstantiated modules shouldn't do this check
        if is_module_declaration(node)
            && get_module_instance_state(node) != ModuleInstanceState::INSTANTIATED
        {
            return;
        }
        // In case of variable declaration, node.parent is variable statement so look at the variable statement's parent
        let parent = get_declaration_container(node);
        if is_source_file(parent)
            && is_external_or_common_js_module(parent)
            && get_emit_module_format_of_file(parent) == ModuleKind::COMMON_JS
        {
            // If the declaration happens to be in external module, report error that Object is a reserved identifier.
            self.error_skipped_on_no_emit(
                name,
                diag::Duplicate_identifier_0_Compiler_reserves_name_1_in_top_level_scope_of_a_module,
                args![declaration_name_to_string(name), declaration_name_to_string(name)],
            );
        }
    }

    // Go: checker/checker.go:10696 needCollisionCheckForIdentifier
    pub fn need_collision_check_for_identifier(
        &mut self,
        node: Node,
        identifier: Node,
        name: &str,
    ) -> bool {
        if identifier.is_some() && identifier.text() != name {
            return false;
        }
        match node.kind() {
            SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::PropertyAssignment => {
                // it is ok to have member named '_super', '_this', `Promise`, etc. - member access is always qualified
                return false;
            }
            _ => {}
        }
        if node.flags().intersects(NodeFlags::AMBIENT) {
            // ambient context - no codegen impact
            return false;
        }
        if is_import_clause(node) || is_import_equals_declaration(node) || is_import_specifier(node)
        {
            // type-only imports do not require collision checks against runtime values.
            if is_type_only_import_or_export_declaration(node) {
                return false;
            }
        }
        let root = get_root_declaration(node);
        if is_parameter_declaration(root) && node_is_missing(root.parent().body()) {
            // just an overload - no codegen impact
            return false;
        }
        true
    }

    // Go: checker/checker.go:10724 setNodeLinksForPrivateIdentifierScope
    pub fn set_node_links_for_private_identifier_scope(&mut self, node: Node) {
        let name = node.name();
        if is_private_identifier(name) {
            if self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.private_names_and_class_static_blocks
                || self.language_version
                    < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators
                || !self.compiler_options.get_use_define_for_class_fields()
            {
                let mut lexical_scope = get_enclosing_block_scope_container(node);
                while lexical_scope.is_some() {
                    self.node_links.get(lexical_scope).flags |=
                        NodeCheckFlags::CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS;
                    lexical_scope = get_enclosing_block_scope_container(lexical_scope);
                }
            }
        }
    }

    // Go: checker/checker.go:10736 recordPotentialCollisionWithWeakMapSetInGeneratedCode
    pub fn record_potential_collision_with_weak_map_set_in_generated_code(
        &mut self,
        node: Node,
        name: Node,
    ) {
        if self.language_version <= ScriptTarget::ES2021
            && (self.need_collision_check_for_identifier(node, name, "WeakMap")
                || self.need_collision_check_for_identifier(node, name, "WeakSet"))
        {
            self.add_deferred_diagnostic(Rc::new(move |c: &mut Checker| {
                c.check_weak_map_set_collision(node);
            }));
        }
    }

    // Go: checker/checker.go:10745 checkWeakMapSetCollision
    pub fn check_weak_map_set_collision(&mut self, node: Node) {
        let enclosing_block_scope = get_enclosing_block_scope_container(node);
        if self
            .node_links
            .get(enclosing_block_scope)
            .flags
            .intersects(NodeCheckFlags::CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS)
        {
            let name = node.name();
            if name.is_some() && is_identifier(name) {
                self.error_skipped_on_no_emit(
                    node,
                    diag::Compiler_reserves_name_0_when_emitting_private_identifier_downlevel,
                    args![name.text()],
                );
            }
        }
    }

    // Go: checker/checker.go:10755 checkCollisionWithGlobalPromiseInGeneratedCode
    pub fn check_collision_with_global_promise_in_generated_code(
        &mut self,
        node: Node,
        name: Node,
    ) {
        if name.is_nil()
            || self.language_version >= ScriptTarget::ES2017
            || !self.need_collision_check_for_identifier(node, name, "Promise")
        {
            return;
        }
        // Uninstantiated modules shouldn't do this check
        if is_module_declaration(node)
            && get_module_instance_state(node) != ModuleInstanceState::INSTANTIATED
        {
            return;
        }
        // In case of variable declaration, node.parent is variable statement so look at the variable statement's parent
        let parent = get_declaration_container(node);
        if is_source_file(parent)
            && is_external_or_common_js_module(parent)
            && parent.flags().intersects(NodeFlags::HAS_ASYNC_FUNCTIONS)
        {
            // If the declaration happens to be in external module, report error that Promise is a reserved identifier.
            self.error_skipped_on_no_emit(
                name,
                diag::Duplicate_identifier_0_Compiler_reserves_name_1_in_top_level_scope_of_a_module_containing_async_functions,
                args![declaration_name_to_string(name), declaration_name_to_string(name)],
            );
        }
    }

    // Go: checker/checker.go:10771 recordPotentialCollisionWithReflectInGeneratedCode
    pub fn record_potential_collision_with_reflect_in_generated_code(
        &mut self,
        node: Node,
        name: Node,
    ) {
        if name.is_some()
            && self.language_version <= ScriptTarget::ES2021
            && self.need_collision_check_for_identifier(node, name, "Reflect")
        {
            self.add_deferred_diagnostic(Rc::new(move |c: &mut Checker| {
                c.check_reflect_collision(node);
            }));
        }
    }

    // Go: checker/checker.go:10779 checkReflectCollision
    pub fn check_reflect_collision(&mut self, node: Node) {
        let mut has_collision = false;
        if is_class_expression(node) {
            // ClassExpression names don't contribute to their containers, but do matter for any of their block-scoped members.
            for member in node.members().iter() {
                if self
                    .node_links
                    .get(member)
                    .flags
                    .intersects(NodeCheckFlags::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER)
                {
                    has_collision = true;
                    break;
                }
            }
        } else if is_function_expression(node) {
            // FunctionExpression names don't contribute to their containers, but do matter for their contents
            if self
                .node_links
                .get(node)
                .flags
                .intersects(NodeCheckFlags::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER)
            {
                has_collision = true;
            }
        } else {
            let container = get_enclosing_block_scope_container(node);
            if container.is_some()
                && self
                    .node_links
                    .get(container)
                    .flags
                    .intersects(NodeCheckFlags::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER)
            {
                has_collision = true;
            }
        }
        if has_collision {
            let name = node.name();
            if name.is_some() && is_identifier(name) {
                self.error_skipped_on_no_emit(
                    node,
                    diag::Duplicate_identifier_0_Compiler_reserves_name_1_when_emitting_super_references_in_static_initializers,
                    args![declaration_name_to_string(name), "Reflect"],
                );
            }
        }
    }

    // Go: checker/checker.go:10808 checkClassNameCollisionWithObject
    pub fn check_class_name_collision_with_object(&mut self, name: Node) {
        if name.text() == "Object"
            && get_emit_module_format_of_file(get_source_file_of_node(name)) < ModuleKind::ES2015
        {
            let module_kind = self.module_kind.string();
            self.error(
                name,
                diag::Class_name_cannot_be_Object_when_targeting_ES5_and_above_with_module_0,
                args![module_kind],
            );
        }
    }

    // Go: checker/checker.go:10814 checkTypeOfExpression
    pub fn check_type_of_expression(&mut self, node: Node) -> TypeId {
        self.check_expression(node.expression());
        self.typeof_type
    }

    // Go: checker/checker.go:10819 checkNonNullAssertion
    pub fn check_non_null_assertion(&mut self, node: Node) -> TypeId {
        if node.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
            // checkNonNullChain checks the same operand expression (node.Expression()),
            // so the child is still visited on this branch.
            return self.check_non_null_chain(node);
        }
        let t = self.check_expression(node.expression());
        self.get_non_nullable_type(t)
    }

    // Go: checker/checker.go:10828 checkNonNullChain
    pub fn check_non_null_chain(&mut self, node: Node) -> TypeId {
        let left_type = self.check_expression(node.expression());
        let non_optional_type = self.get_optional_expression_type(left_type, node.expression());
        let non_nullable = self.get_non_nullable_type(non_optional_type);
        self.propagate_optional_type_marker(non_nullable, node, non_optional_type != left_type)
    }

    // Go: checker/checker.go:10834 checkExpressionWithTypeArguments
    pub fn check_expression_with_type_arguments(&mut self, node: Node) -> TypeId {
        self.check_grammar_expression_with_type_arguments(node);
        self.check_source_elements(node.type_arguments());
        if is_expression_with_type_arguments(node) {
            let parent = walk_up_parenthesized_expressions(node.parent());
            if is_binary_expression(parent)
                && parent.operator_token().kind() == SyntaxKind::InstanceOfKeyword
                && is_node_descendant_of(node, parent.right())
            {
                self.error(
                    node,
                    diag::The_right_hand_side_of_an_instanceof_expression_must_not_be_an_instantiation_expression,
                    args![],
                );
            }
        }
        let expr_type;
        if is_expression_with_type_arguments(node) {
            expr_type = self.check_expression(node.expression());
        } else {
            let expr_name = node.expr_name();
            if is_this_identifier(expr_name) {
                expr_type = self.check_this_expression(node.expr_name());
            } else {
                expr_type = self.check_expression(node.expr_name());
            }
        }
        self.get_instantiation_expression_type(expr_type, node)
    }

    // Go: checker/checker.go:10857 getInstantiationExpressionType
    pub fn get_instantiation_expression_type(&mut self, expr_type: TypeId, node: Node) -> TypeId {
        let type_arguments = node.type_argument_list();
        if expr_type == self.silent_never_type
            || self.is_error_type(expr_type)
            || type_arguments.is_nil()
        {
            return expr_type;
        }
        let key = InstantiationExpressionKey {
            node_id: node,
            type_id: expr_type,
        };
        if let Some(cached) = self.instantiation_expression_types.get(&key).copied() {
            if cached.is_some() {
                return cached;
            }
        }
        let mut state = InstantiationExpressionTypeState {
            node,
            type_arguments: type_arguments.nodes().to_vec(),
            has_some_applicable_signature: false,
            non_applicable_type: TypeId::NIL,
        };
        let result = self.instantiation_expression_get_instantiated_type(&mut state, expr_type);
        self.instantiation_expression_types.insert(key, result);
        let error_type = if state.has_some_applicable_signature {
            state.non_applicable_type
        } else {
            expr_type
        };
        if error_type.is_some() {
            let source_file = get_source_file_of_node(node);
            let loc = TextRange::new(
                skip_trivia(&source_file_text(source_file), type_arguments.pos()),
                type_arguments.end(),
            );
            let type_string = self.type_to_string_exported(error_type);
            self.add_diagnostic(new_diagnostic(
                source_file,
                loc,
                diag::Type_0_has_no_signatures_for_which_the_type_argument_list_is_applicable,
                args![type_string],
            ));
        }
        result
    }

    // PORT: Go closure `getInstantiatedSignatures` inside getInstantiationExpressionType.
    // Go `core.Filter` + `core.SameMap` return the input slice when nothing
    // changed; callers compare with `core.Same`, which here is content
    // equality (a changed result always differs in length or content).
    // Go: checker/checker.go:10868 getInstantiationExpressionType.getInstantiatedSignatures
    fn instantiation_expression_get_instantiated_signatures(
        &mut self,
        state: &mut InstantiationExpressionTypeState,
        signatures: &[SignatureId],
    ) -> Vec<SignatureId> {
        let type_arguments = state.type_arguments.clone();
        let mut applicable_signatures: Vec<SignatureId> = Vec::new();
        for sig in signatures.iter().copied() {
            if !self.sig(sig).type_parameters.is_empty()
                && self.has_correct_type_argument_arity(sig, &type_arguments)
            {
                applicable_signatures.push(sig);
            }
        }
        let mut result = Vec::with_capacity(applicable_signatures.len());
        for sig in applicable_signatures {
            let type_argument_types =
                self.check_type_arguments(sig, &type_arguments, true /*reportErrors*/, None);
            if let Some(type_argument_types) = type_argument_types {
                let declaration = self.sig(sig).declaration;
                result.push(self.get_signature_instantiation(
                    sig,
                    &type_argument_types,
                    is_in_js_file(declaration),
                    &[],
                    0,
                ));
            } else {
                result.push(sig);
            }
        }
        result
    }

    // PORT: Go closure `getInstantiatedType` inside getInstantiationExpressionType.
    // Go: checker/checker.go:10880 getInstantiationExpressionType.getInstantiatedType
    fn instantiation_expression_get_instantiated_type(
        &mut self,
        state: &mut InstantiationExpressionTypeState,
        t: TypeId,
    ) -> TypeId {
        let mut has_signatures = false;
        let mut has_applicable_signature = false;
        let result = self.instantiation_expression_get_instantiated_type_part(
            state,
            &mut has_signatures,
            &mut has_applicable_signature,
            t,
        );
        state.has_some_applicable_signature =
            state.has_some_applicable_signature || has_applicable_signature;
        if has_signatures && !has_applicable_signature {
            if state.non_applicable_type.is_nil() {
                state.non_applicable_type = t;
            }
        }
        result
    }

    // PORT: Go closure `getInstantiatedTypePart` inside getInstantiatedType.
    // `has_signatures` and `has_applicable_signature` are the enclosing
    // getInstantiatedType call's locals.
    // Go: checker/checker.go:10884 getInstantiationExpressionType.getInstantiatedTypePart
    fn instantiation_expression_get_instantiated_type_part(
        &mut self,
        state: &mut InstantiationExpressionTypeState,
        has_signatures: &mut bool,
        has_applicable_signature: &mut bool,
        t: TypeId,
    ) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            self.resolve_structured_type_members(t);
            let resolved_call_signatures =
                self.ty(t).as_structured_type().call_signatures().to_vec();
            let call_signatures = self.instantiation_expression_get_instantiated_signatures(
                state,
                &resolved_call_signatures,
            );
            let resolved_construct_signatures = self
                .ty(t)
                .as_structured_type()
                .construct_signatures()
                .to_vec();
            let construct_signatures = self.instantiation_expression_get_instantiated_signatures(
                state,
                &resolved_construct_signatures,
            );
            *has_signatures = *has_signatures
                || !resolved_call_signatures.is_empty()
                || !resolved_construct_signatures.is_empty();
            *has_applicable_signature = *has_applicable_signature
                || !call_signatures.is_empty()
                || !construct_signatures.is_empty();
            if call_signatures != resolved_call_signatures
                || construct_signatures != resolved_construct_signatures
            {
                let symbol = self.new_symbol(
                    SymbolFlags::NONE,
                    INTERNAL_SYMBOL_NAME_INSTANTIATION_EXPRESSION,
                );
                let t_symbol = self.ty(t).symbol;
                debug_assert!(
                    t_symbol.is_some(),
                    "Instantiation expression source type must have a symbol"
                );
                let declarations = self.sym(t_symbol).declarations.clone();
                self.sym_mut(symbol).declarations = declarations;
                let result = self.new_object_type(
                    ObjectFlags::ANONYMOUS | ObjectFlags::INSTANTIATION_EXPRESSION_TYPE,
                    symbol,
                );
                let (members, index_infos) = {
                    let resolved = self.ty(t).as_structured_type();
                    (resolved.members, resolved.index_infos_list())
                };
                self.set_structured_type_members(
                    result,
                    members,
                    &call_signatures,
                    &construct_signatures,
                    &index_infos,
                );
                self.ty_mut(result)
                    .as_instantiation_expression_type_mut()
                    .node = state.node;
                return result;
            }
        } else if flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            let constraint = self.get_base_constraint_of_type(t);
            if constraint.is_some() {
                let instantiated = self.instantiation_expression_get_instantiated_type_part(
                    state,
                    has_signatures,
                    has_applicable_signature,
                    constraint,
                );
                if instantiated != constraint {
                    return instantiated;
                }
            }
        } else if flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.instantiation_expression_get_instantiated_type(state, t)
            });
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            // PORT: Go `core.SameMap`; getIntersectionType sees the same elements either way.
            let types = self
                .ty(t)
                .as_intersection_type()
                .union_or_intersection
                .types
                .clone();
            let mut mapped = Vec::with_capacity(types.len());
            for member in types {
                mapped.push(self.instantiation_expression_get_instantiated_type_part(
                    state,
                    has_signatures,
                    has_applicable_signature,
                    member,
                ));
            }
            return self.get_intersection_type(&mapped);
        }
        t
    }

    // Go: checker/checker.go:10941 checkSatisfiesExpression
    pub fn check_satisfies_expression(&mut self, node: Node) -> TypeId {
        let type_node = node.type_();
        self.check_source_element(type_node);
        let expr_type = self.check_expression(node.expression());
        let target_type = self.get_type_from_type_node(type_node);
        if self.is_error_type(target_type) {
            return target_type;
        }
        self.check_type_assignable_to_and_optionally_elaborate(
            expr_type,
            target_type,
            node,
            node.expression(),
            Some(diag::Type_0_does_not_satisfy_the_expected_type_1),
            None,
        );
        expr_type
    }

    // Go: checker/checker.go:10953 checkMetaProperty
    pub fn check_meta_property(&mut self, node: Node) -> TypeId {
        self.check_grammar_meta_property(node);
        match node.keyword_token() {
            SyntaxKind::NewKeyword => {
                return self.check_new_target_meta_property(node);
            }
            SyntaxKind::ImportKeyword => {
                // ts#63915, Go N' checker.go:10993: `import.defer` and `import.source`
                if is_import_phase_meta_property(node) {
                    if is_call_expression(node.parent()) {
                        debug_assert!(
                            node.parent().expression() != node,
                            "Trying to get the type of a phase import meta-property in its call"
                        );
                    }
                    return self.error_type;
                }
                return self.check_import_meta_property(node);
            }
            _ => {}
        }
        panic!("Unhandled case in checkMetaProperty");
    }

    // Go: checker/checker.go:10968 checkNewTargetMetaProperty
    pub fn check_new_target_meta_property(&mut self, node: Node) -> TypeId {
        let container = get_new_target_container(node);
        if container.is_nil() {
            self.error(
                node,
                diag::Meta_property_0_is_only_allowed_in_the_body_of_a_function_declaration_function_expression_or_constructor,
                args!["new.target"],
            );
            return self.error_type;
        }
        if is_constructor_declaration(container) {
            let symbol = self.get_symbol_of_declaration(container.parent());
            return self.get_type_of_symbol(symbol);
        }
        let symbol = self.get_symbol_of_declaration(container);
        self.get_type_of_symbol(symbol)
    }

    // Go: checker/checker.go:10982 checkImportMetaProperty
    pub fn check_import_meta_property(&mut self, node: Node) -> TypeId {
        if ModuleKind::NODE16 <= self.module_kind && self.module_kind <= ModuleKind::NODE_NEXT {
            // PORT: Go `ast.GetSourceFileOfNode(node).Path()` is the SourceFile
            // `path` field, read through `source_file_info`.
            let source_file_meta_data =
                get_source_file_meta_data(&source_file_info(get_source_file_of_node(node)).path);
            if source_file_meta_data.implied_node_format != ModuleKind::ES_NEXT {
                self.error(
                    node,
                    diag::The_import_meta_meta_property_is_not_allowed_in_files_which_will_build_into_CommonJS_output,
                    args![],
                );
            }
        } else if self.module_kind < ModuleKind::ES2020 && self.module_kind != ModuleKind::SYSTEM {
            self.error(
                node,
                diag::The_import_meta_meta_property_is_only_allowed_when_the_module_option_is_es2020_es2022_esnext_system_node16_node18_node20_or_nodenext,
                args![],
            );
        }
        let file = get_source_file_of_node(node);
        debug_assert!(
            file.flags()
                .intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META),
            "Containing file is missing import meta node flag."
        );
        if node.name().text() == "meta" {
            return self.get_global_import_meta_type();
        }
        self.error_type
    }

    // Go: checker/checker.go:10999 checkMetaPropertyKeyword
    pub fn check_meta_property_keyword(&mut self, node: Node) -> TypeId {
        // !!! This is effectively a helper for GetSymbolAtLocation and GetTypeAtLocation
        self.error_type
    }

    // Go: checker/checker.go:11004 checkDeleteExpression
    pub fn check_delete_expression(&mut self, node: Node) -> TypeId {
        self.check_expression(node.expression());
        let expr = skip_parentheses(node.expression());
        if !is_access_expression(expr) {
            self.error(
                expr,
                diag::The_operand_of_a_delete_operator_must_be_a_property_reference,
                args![],
            );
            return self.boolean_type;
        }
        if is_property_access_expression(expr) && is_private_identifier(expr.name()) {
            self.error(
                expr,
                diag::The_operand_of_a_delete_operator_cannot_be_a_private_identifier,
                args![],
            );
        }
        let resolved = self.get_resolved_symbol_or_nil(expr);
        let symbol = self.get_export_symbol_of_value_symbol_if_exported(resolved);
        if symbol.is_some() {
            if self.is_readonly_symbol(symbol) {
                self.error(
                    expr,
                    diag::The_operand_of_a_delete_operator_cannot_be_a_read_only_property,
                    args![],
                );
            } else {
                self.check_delete_expression_must_be_optional(expr, symbol);
            }
        }
        self.boolean_type
    }

    // Go: checker/checker.go:11025 checkDeleteExpressionMustBeOptional
    pub fn check_delete_expression_must_be_optional(&mut self, expr: Node, symbol: SymbolId) {
        let t = self.get_type_of_symbol(symbol);
        if self.strict_null_checks
            && !self
                .ty(t)
                .flags
                .intersects(TypeFlags::ANY_OR_UNKNOWN | TypeFlags::NEVER)
        {
            let is_optional;
            if self.exact_optional_property_types {
                is_optional = self.sym(symbol).flags.intersects(SymbolFlags::OPTIONAL);
            } else {
                is_optional = self.has_type_facts(t, TypeFacts::IS_UNDEFINED);
            }
            if !is_optional {
                self.error(
                    expr,
                    diag::The_operand_of_a_delete_operator_must_be_optional,
                    args![],
                );
            }
        }
    }

    // Go: checker/checker.go:11040 checkVoidExpression
    pub fn check_void_expression(&mut self, node: Node) -> TypeId {
        self.check_node_deferred(node);
        self.undefined_widening_type
    }

    // Go: checker/checker.go:11045 checkAwaitExpression
    pub fn check_await_expression(&mut self, node: Node) -> TypeId {
        self.check_grammar_await_or_await_using(node);
        let operand_type = self.check_expression(node.expression());
        let awaited_type = self.check_awaited_type(
            operand_type,
            true, /*withAlias*/
            node,
            diag::Type_of_await_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member,
        );
        if awaited_type == operand_type
            && !self.is_error_type(awaited_type)
            && !self
                .ty(operand_type)
                .flags
                .intersects(TypeFlags::ANY_OR_UNKNOWN)
        {
            self.add_error_or_suggestion(
                false,
                create_diagnostic_for_node(
                    node,
                    diag::X_await_has_no_effect_on_the_type_of_this_expression,
                    args![],
                ),
            );
        }
        awaited_type
    }

    // Go: checker/checker.go:11055 checkPrefixUnaryExpression
    pub fn check_prefix_unary_expression(&mut self, node: Node) -> TypeId {
        let operand = node.operand();
        let operator = node.operator();
        let operand_type = self.check_expression(operand);
        if operand_type == self.silent_never_type {
            return self.silent_never_type;
        }
        match operand.kind() {
            SyntaxKind::NumericLiteral => match operator {
                SyntaxKind::MinusToken => {
                    let literal = self.get_number_literal_type(-crate::jsnum::Number::from_string(
                        operand.text(),
                    ));
                    return self.get_fresh_type_of_literal_type(literal);
                }
                SyntaxKind::PlusToken => {
                    let literal = self
                        .get_number_literal_type(crate::jsnum::Number::from_string(operand.text()));
                    return self.get_fresh_type_of_literal_type(literal);
                }
                _ => {}
            },
            SyntaxKind::BigIntLiteral => {
                if operator == SyntaxKind::MinusToken {
                    let literal = self.get_big_int_literal_type(crate::jsnum::PseudoBigInt::new(
                        &crate::jsnum::parse_pseudo_big_int(operand.text()),
                        true, /*negative*/
                    ));
                    return self.get_fresh_type_of_literal_type(literal);
                }
            }
            _ => {}
        }
        match operator {
            SyntaxKind::PlusToken | SyntaxKind::MinusToken | SyntaxKind::TildeToken => {
                self.check_non_null_type(operand_type, operand);
                if self.maybe_type_of_kind_considering_base_constraint(
                    operand_type,
                    TypeFlags::ES_SYMBOL_LIKE,
                ) {
                    self.error(
                        operand,
                        diag::The_0_operator_cannot_be_applied_to_type_symbol,
                        args![token_to_string(operator)],
                    );
                }
                if operator == SyntaxKind::PlusToken {
                    if self.maybe_type_of_kind_considering_base_constraint(
                        operand_type,
                        TypeFlags::BIG_INT_LIKE,
                    ) {
                        let base = self.get_base_type_of_literal_type(operand_type);
                        let base_string = self.type_to_string_exported(base);
                        self.error(
                            operand,
                            diag::Operator_0_cannot_be_applied_to_type_1,
                            args![token_to_string(operator), base_string],
                        );
                    }
                    return self.number_type;
                }
                return self.get_unary_result_type(operand_type);
            }
            SyntaxKind::ExclamationToken => {
                self.check_truthiness_of_type(operand_type, operand);
                let facts = self.get_type_facts(operand_type, TypeFacts::TRUTHY | TypeFacts::FALSY);
                if facts == TypeFacts::TRUTHY {
                    return self.false_type;
                } else if facts == TypeFacts::FALSY {
                    return self.true_type;
                } else {
                    return self.boolean_type;
                }
            }
            SyntaxKind::PlusPlusToken | SyntaxKind::MinusMinusToken => {
                let non_null = self.check_non_null_type(operand_type, operand);
                let ok = self.check_arithmetic_operand_type(
                    operand,
                    non_null,
                    diag::An_arithmetic_operand_must_be_of_type_any_number_bigint_or_an_enum_type,
                    false,
                );
                if ok {
                    // run check only if former checks succeeded to avoid reporting cascading errors
                    self.check_reference_expression(
                        operand,
                        diag::The_operand_of_an_increment_or_decrement_operator_must_be_a_variable_or_a_property_access,
                        diag::The_operand_of_an_increment_or_decrement_operator_may_not_be_an_optional_property_access,
                    );
                }
                return self.get_unary_result_type(operand_type);
            }
            _ => {}
        }
        self.error_type
    }

    // Go: checker/checker.go:11109 checkPostfixUnaryExpression
    pub fn check_postfix_unary_expression(&mut self, node: Node) -> TypeId {
        let operand = node.operand();
        let operand_type = self.check_expression(operand);
        if operand_type == self.silent_never_type {
            return self.silent_never_type;
        }
        let non_null = self.check_non_null_type(operand_type, operand);
        let ok = self.check_arithmetic_operand_type(
            operand,
            non_null,
            diag::An_arithmetic_operand_must_be_of_type_any_number_bigint_or_an_enum_type,
            false,
        );
        if ok {
            // run check only if former checks succeeded to avoid reporting cascading errors
            self.check_reference_expression(
                operand,
                diag::The_operand_of_an_increment_or_decrement_operator_must_be_a_variable_or_a_property_access,
                diag::The_operand_of_an_increment_or_decrement_operator_may_not_be_an_optional_property_access,
            );
        }
        self.get_unary_result_type(operand_type)
    }

    // Go: checker/checker.go:11123 getUnaryResultType
    pub fn get_unary_result_type(&mut self, operand_type: TypeId) -> TypeId {
        if self.maybe_type_of_kind(operand_type, TypeFlags::BIG_INT_LIKE) {
            if self.is_type_assignable_to_kind(operand_type, TypeFlags::ANY_OR_UNKNOWN)
                || self.maybe_type_of_kind(operand_type, TypeFlags::NUMBER_LIKE)
            {
                return self.number_or_big_int_type;
            }
            return self.bigint_type;
        }
        // If it's not a bigint type, implicit coercion will result in a number
        self.number_type
    }

    // Go: checker/checker.go:11134 checkConditionalExpression
    pub fn check_conditional_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let condition = node.condition();
        let when_true = node.when_true();
        let when_false = node.when_false();
        let t = self.check_truthiness_expression(condition, check_mode);
        self.check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(
            condition, t, when_true,
        );
        let type1 = self.check_expression_ex(when_true, check_mode);
        let type2 = self.check_expression_ex(when_false, check_mode);
        self.get_union_type_ex(&[type1, type2], UnionReduction::SUBTYPE, None, TypeId::NIL)
    }

    // Go: checker/checker.go:11143 checkTruthinessExpression
    pub fn check_truthiness_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let t = self.check_expression_ex(node, check_mode);
        self.check_truthiness_of_type(t, node)
    }

    // Go: checker/checker.go:11147 checkSpreadExpression
    pub fn check_spread_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let array_or_iterable_type = self.check_expression_ex(node.expression(), check_mode);
        let undefined_type = self.undefined_type;
        self.check_iterated_type_or_element_type(
            IterationUse::SPREAD,
            array_or_iterable_type,
            undefined_type,
            node.expression(),
        )
    }

    // Go: checker/checker.go:11152 checkYieldExpression
    pub fn check_yield_expression(&mut self, node: Node) -> TypeId {
        self.check_grammar_yield_expression(node);
        // Always check the operand so its identifiers are resolved even when the yield is
        // outside a generator, keeping diagnostics stable regardless of traversal order.
        let yield_expression_type;
        if node.expression().is_some() {
            yield_expression_type = self.check_expression(node.expression());
        } else {
            yield_expression_type = self.undefined_widening_type;
        }
        let fn_ = get_containing_function(node);
        if fn_.is_nil() {
            return self.any_type;
        }
        let function_flags = get_function_flags(fn_);
        if !function_flags.intersects(FunctionFlags::GENERATOR) {
            // If the user's code is syntactically correct, the func should always have a star. After all, we are in a yield context.
            return self.any_type;
        }
        let is_async = function_flags.intersects(FunctionFlags::ASYNC);
        if node.asterisk_token().is_some() {
            // Async generator functions prior to ES2018 require the __await, __asyncDelegator,
            // and __asyncValues helpers
            if is_async && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.async_generators
            {
                self.check_external_emit_helpers(
                    node,
                    ExternalEmitHelpers::ASYNC_DELEGATOR_INCLUDES,
                );
            }
        }
        // There is no point in doing an assignability check if the function
        // has no explicit return type because the return type is directly computed
        // from the yield expressions.
        let mut return_type = self.get_return_type_from_annotation(fn_);
        if return_type.is_some() && self.ty(return_type).flags.intersects(TypeFlags::UNION) {
            return_type = self.filter_type(return_type, &mut |c: &mut Checker, t: TypeId| {
                c.check_generator_instantiation_assignability_to_return_type(
                    t,
                    function_flags,
                    Node::NIL, /*errorNode*/
                )
            });
        }
        let mut iteration_types = IterationTypes::default();
        if return_type.is_some() {
            iteration_types =
                self.get_iteration_types_of_generator_function_return_type(return_type, is_async);
        }
        let signature_yield_type = if iteration_types.yield_type.is_some() {
            iteration_types.yield_type
        } else {
            self.any_type
        };
        let signature_next_type = if iteration_types.next_type.is_some() {
            iteration_types.next_type
        } else {
            self.any_type
        };
        let yielded_type = self.get_yielded_type_of_yield_expression(
            node,
            yield_expression_type,
            signature_next_type,
            is_async,
        );
        if return_type.is_some() && yielded_type.is_some() {
            let error_node = if node.expression().is_some() {
                node.expression()
            } else {
                node
            };
            self.check_type_assignable_to_and_optionally_elaborate(
                yielded_type,
                signature_yield_type,
                error_node,
                node.expression(),
                None,
                None,
            );
        }
        if node.asterisk_token().is_some() {
            let use_ = if is_async {
                IterationUse::ASYNC_YIELD_STAR
            } else {
                IterationUse::YIELD_STAR
            };
            let t = self.get_iteration_type_of_iterable(
                use_,
                IterationTypeKind::RETURN,
                yield_expression_type,
                node.expression(),
            );
            return if t.is_some() { t } else { self.any_type };
        }
        if return_type.is_some() {
            let t = self.get_iteration_type_of_generator_function_return_type(
                IterationTypeKind::NEXT,
                return_type,
                is_async,
            );
            return if t.is_some() { t } else { self.any_type };
        }
        let mut t = self.get_contextual_iteration_type(IterationTypeKind::NEXT, fn_);
        if t.is_nil() {
            t = self.any_type;
            if self.no_implicit_any && !expression_result_is_unused(node) {
                let contextual_type = self.get_contextual_type(node, ContextFlags::NONE);
                if contextual_type.is_nil() || self.is_type_any(contextual_type) {
                    self.error(
                        node,
                        diag::X_yield_expression_implicitly_results_in_an_any_type_because_its_containing_generator_lacks_a_return_type_annotation,
                        args![],
                    );
                }
            }
        }
        t
    }

    // Go: checker/checker.go:11218 getYieldedTypeOfYieldExpression
    pub fn get_yielded_type_of_yield_expression(
        &mut self,
        node: Node,
        expression_type: TypeId,
        sent_type: TypeId,
        is_async: bool,
    ) -> TypeId {
        let error_node = if node.expression().is_some() {
            node.expression()
        } else {
            node
        };
        let is_yield_star = node.asterisk_token().is_some();
        // A `yield*` expression effectively yields everything that its operand yields
        let mut yielded_type = expression_type;
        if is_yield_star {
            yielded_type = self.check_iterated_type_or_element_type(
                if is_async {
                    IterationUse::ASYNC_YIELD_STAR
                } else {
                    IterationUse::YIELD_STAR
                },
                expression_type,
                sent_type,
                error_node,
            );
        }
        if !is_async {
            return yielded_type;
        }
        let message: &'static Message = if is_yield_star {
            diag::Type_of_iterated_elements_of_a_yield_Asterisk_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member
        } else {
            diag::Type_of_yield_operand_in_an_async_generator_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member
        };
        self.get_awaited_type_ex(yielded_type, error_node, Some(message), args![])
    }

    // Go: checker/checker.go:11234 checkSyntheticExpression
    pub fn check_synthetic_expression(&mut self, node: Node) -> TypeId {
        // PORT: Go `node.AsSyntheticExpression().Type.(*Type)` is kept in the
        // synthetic node slot and read with `synthetic_expression_type`.
        let t: TypeId = synthetic_expression_type(node);
        if node.is_spread() {
            let number_type = self.number_type;
            return self.get_indexed_access_type(t, number_type);
        }
        t
    }

    // Go: checker/checker.go:11242 checkIdentifier
    pub fn check_identifier(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        if is_this_in_type_query(node) {
            return self.check_this_expression(node);
        }
        let symbol = self.get_resolved_symbol(node);
        if symbol == self.unknown_symbol {
            return self.error_type;
        }
        if symbol == self.arguments_symbol {
            if self.is_in_property_initializer_or_class_static_block(
                node, true, /*ignoreArrowFunctions*/
            ) {
                self.error(
                    node,
                    diag::X_arguments_cannot_be_referenced_in_property_initializers_or_class_static_initialization_blocks,
                    args![],
                );
                return self.error_type;
            }
            return self.get_type_of_symbol(symbol);
        }
        if should_mark_identifier_alias_referenced(node) {
            self.mark_linked_references(
                node,
                ReferenceHint::IDENTIFIER,
                SymbolId::NIL, /*propSymbol*/
                TypeId::NIL,   /*parentType*/
            );
        }
        let local_or_export_symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol);
        let target_symbol = self.resolve_alias_with_deprecation_check(local_or_export_symbol, node);
        if !self.sym(target_symbol).declarations.is_empty()
            && self.is_deprecated_symbol(target_symbol)
            && self.is_uncalled_function_reference(node, target_symbol)
        {
            let declarations = self.sym(target_symbol).declarations.clone();
            self.add_deprecated_suggestion(node, &declarations, node.text());
        }
        let mut declaration = self.sym(local_or_export_symbol).value_declaration;
        let immediate_declaration = declaration;
        // If the identifier is declared in a binding pattern for which we're currently computing the implied type and the
        // reference occurs with the same binding pattern, return the non-inferrable any type. This for example occurs in
        // 'const [a, b = a + 1] = [2]' when we're computing the contextual type for the array literal '[2]'.
        if declaration.is_some()
            && declaration.kind() == SyntaxKind::BindingElement
            && self
                .contextual_binding_patterns
                .contains(&declaration.parent())
            && find_ancestor(node, |parent| parent == declaration.parent()).is_some()
        {
            return self.non_inferrable_any_type;
        }
        let mut t = self.get_narrowed_type_of_symbol(local_or_export_symbol, node);
        let assignment_kind = get_assignment_target_kind(node);
        let local_flags = self.sym(local_or_export_symbol).flags;
        if assignment_kind != AssignmentKind::NONE {
            if !local_flags.intersects(SymbolFlags::VARIABLE)
                && !(is_in_js_file(node) && local_flags.intersects(SymbolFlags::VALUE_MODULE))
            {
                let assignment_error: &'static Message;
                if local_flags.intersects(SymbolFlags::ENUM) {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_an_enum;
                } else if local_flags.intersects(SymbolFlags::CLASS) {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_a_class;
                } else if local_flags.intersects(SymbolFlags::MODULE) {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_a_namespace;
                } else if local_flags.intersects(SymbolFlags::FUNCTION) {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_a_function;
                } else if local_flags.intersects(SymbolFlags::ALIAS) {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_an_import;
                } else {
                    assignment_error = diag::Cannot_assign_to_0_because_it_is_not_a_variable;
                }
                let symbol_string = self.symbol_to_string(symbol);
                self.error(node, assignment_error, args![symbol_string]);
                return self.error_type;
            }
            if self.is_readonly_symbol(local_or_export_symbol) {
                let symbol_string = self.symbol_to_string(symbol);
                if local_flags.intersects(SymbolFlags::VARIABLE) {
                    self.error(
                        node,
                        diag::Cannot_assign_to_0_because_it_is_a_constant,
                        args![symbol_string],
                    );
                } else {
                    self.error(
                        node,
                        diag::Cannot_assign_to_0_because_it_is_a_read_only_property,
                        args![symbol_string],
                    );
                }
                return self.error_type;
            }
        }
        let is_alias = local_flags.intersects(SymbolFlags::ALIAS);
        // We only narrow variables and parameters occurring in a non-assignment position. For all other
        // entities we simply return the declared type.
        if local_flags.intersects(SymbolFlags::VARIABLE) {
            if assignment_kind == AssignmentKind::DEFINITE {
                if is_in_compound_like_assignment(node) {
                    return self.get_base_type_of_literal_type(t);
                }
                return t;
            }
        } else if is_alias {
            declaration = self.get_declaration_of_alias_symbol(symbol);
        } else {
            return t;
        }
        if declaration.is_nil() {
            return t;
        }
        t = self.get_narrowable_type_for_reference(t, node, check_mode);
        // The declaration container is the innermost function that encloses the declaration of the variable
        // or parameter. The flow container is the innermost function starting with which we analyze the control
        // flow graph to determine the control flow based type.
        let is_parameter = get_root_declaration(declaration).kind() == SyntaxKind::Parameter;
        let declaration_container = self.get_control_flow_container(declaration);
        let mut flow_container = self.get_control_flow_container(node);
        let is_outer_variable = flow_container != declaration_container;
        // PERF: chkA. The parent and its kind are read once, for the tests
        // below that read `node.Parent` in Go.
        let (parent, parent_kind) = node_parent_and_kind(node);
        let is_spread_destructuring_assignment_target = parent_kind == SyntaxKind::SpreadAssignment
            && {
                let grandparent = parent.parent();
                grandparent.is_some() && self.is_destructuring_assignment_target(grandparent)
            };
        let is_module_exports = self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::MODULE_EXPORTS);
        let type_is_automatic = t == self.auto_type || t == self.auto_array_type;
        let is_automatic_type_in_non_null =
            type_is_automatic && parent_kind == SyntaxKind::NonNullExpression;
        // When the control flow originates in a function expression, arrow function, method, or accessor, and
        // we are referencing a closed-over const variable or parameter or mutable local variable past its last
        // assignment, we extend the origin of the control flow analysis to include the immediately enclosing
        // control flow container.
        while flow_container != declaration_container
            && (is_function_expression_or_arrow_function(flow_container)
                || is_object_literal_or_class_expression_method_or_accessor(flow_container))
            && (self.is_constant_variable(local_or_export_symbol) && t != self.auto_array_type
                || self.is_parameter_or_mutable_local_variable(local_or_export_symbol)
                    && self.is_past_last_assignment(local_or_export_symbol, node))
        {
            flow_container = self.get_control_flow_container(flow_container);
        }
        // We only look for uninitialized variables in strict null checking mode, and only when we can analyze
        // the entire control flow graph from the variable's declaration (i.e. when the flow container and
        // declaration container are the same).
        let is_never_initialized = immediate_declaration.is_some()
            && is_variable_declaration(immediate_declaration)
            && !is_for_in_or_of_statement(immediate_declaration.parent().parent())
            && immediate_declaration.initializer().is_nil()
            && immediate_declaration.exclamation_token().is_nil()
            && self.is_mutable_local_variable_declaration(immediate_declaration)
            && !self.is_symbol_assigned_definitely(symbol);
        let assume_initialized = is_parameter
            || is_alias
            || (is_outer_variable && !is_never_initialized)
            || is_spread_destructuring_assignment_target
            || is_module_exports
            || self.is_same_scoped_binding_element(node, declaration)
            || t != self.auto_type
                && t != self.auto_array_type
                && (!self.strict_null_checks
                    || self
                        .ty(t)
                        .flags
                        .intersects(TypeFlags::ANY_OR_UNKNOWN | TypeFlags::VOID)
                    || is_in_type_query(node)
                    || self.is_in_ambient_or_type_node(node)
                    || parent_kind == SyntaxKind::ExportSpecifier)
            || parent_kind == SyntaxKind::NonNullExpression
            || is_variable_declaration(declaration) && declaration.exclamation_token().is_some()
            || declaration.flags().intersects(NodeFlags::AMBIENT);
        let initial_type;
        if is_automatic_type_in_non_null {
            initial_type = self.undefined_type;
        } else if assume_initialized && is_parameter {
            initial_type = self.remove_optionality_from_declared_type(t, declaration);
        } else if assume_initialized {
            initial_type = t;
        } else if type_is_automatic {
            initial_type = self.undefined_type;
        } else {
            initial_type = self.get_optional_type(t, false /*isProperty*/);
        }
        let flow_type;
        if is_automatic_type_in_non_null {
            let flow = self.get_flow_type_of_reference_ex(
                node,
                t,
                initial_type,
                flow_container,
                FlowNodeId::NIL,
            );
            flow_type = self.get_non_nullable_type(flow);
        } else {
            flow_type = self.get_flow_type_of_reference_ex(
                node,
                t,
                initial_type,
                flow_container,
                FlowNodeId::NIL,
            );
        }
        // A variable is considered uninitialized when it is possible to analyze the entire control flow graph
        // from declaration to use, and when the variable's declared type doesn't include undefined but the
        // control flow based type does include undefined.
        // PERF: chkA. Go calls `isEvolvingArrayOperationTarget(node)` first,
        // but its answer matters only for an automatic type. For any other
        // type only its element assignment part runs, at the same point,
        // because that part can make types. The length, push and unshift part
        // only reads the tree.
        let is_evolving_array_operation_target = if t == self.auto_type || t == self.auto_array_type
        {
            self.is_evolving_array_operation_target(node)
        } else {
            let root = self.get_reference_root(node);
            self.is_evolving_array_element_assignment(root, root.parent());
            false
        };
        if !is_evolving_array_operation_target && (t == self.auto_type || t == self.auto_array_type)
        {
            if flow_type == self.auto_type || flow_type == self.auto_array_type {
                if self.no_implicit_any {
                    let symbol_string = self.symbol_to_string(symbol);
                    let flow_type_string = self.type_to_string_exported(flow_type);
                    self.error(
                        get_name_of_declaration(declaration),
                        diag::Variable_0_implicitly_has_type_1_in_some_locations_where_its_type_cannot_be_determined,
                        args![symbol_string, flow_type_string],
                    );
                    let symbol_string = self.symbol_to_string(symbol);
                    let flow_type_string = self.type_to_string_exported(flow_type);
                    self.error(
                        node,
                        diag::Variable_0_implicitly_has_an_1_type,
                        args![symbol_string, flow_type_string],
                    );
                }
                return self.convert_auto_to_any(flow_type);
            }
        } else if !assume_initialized
            && !self.contains_undefined_type(t)
            && self.contains_undefined_type(flow_type)
        {
            let symbol_string = self.symbol_to_string(symbol);
            self.error(
                node,
                diag::Variable_0_is_used_before_being_assigned,
                args![symbol_string],
            );
            // Return the declared type to reduce follow-on errors
            return t;
        }
        if assignment_kind != AssignmentKind::NONE {
            // Identifier is target of a compound assignment
            return self.get_base_type_of_literal_type(flow_type);
        }
        flow_type
    }
}

/// The parent of `node` and the parent's kind: one store lookup for a node of
/// a published store (`frozen_store_parent_kind`). The kind of a nil parent
/// is `SyntaxKind::Unknown`.
#[inline]
pub fn node_parent_and_kind(node: Node) -> (Node, SyntaxKind) {
    match crate::ast::store::frozen_store_parent_kind(node) {
        Some(parent_and_kind) => parent_and_kind,
        None => {
            let parent = node.parent();
            let kind = if parent.is_some() {
                parent.kind()
            } else {
                SyntaxKind::Unknown
            };
            (parent, kind)
        }
    }
}
