//! Port of Go `checker/checker.go` lines 7528-8444 (checkExpression through
//! resolveSignature).

use crate::prelude::*;

// PORT: Go `hasInferenceCandidates(info)` (inference.go) inlined as a local
// helper on the Rust `InferenceInfo` value, because the Go `[]*InferenceInfo`
// slices used here are not all owned by one inference context.
fn info_has_inference_candidates(info: &InferenceInfo) -> bool {
    !info.candidates().is_empty() || !info.contra_candidates().is_empty()
}

impl Checker {
    // Go: checker/checker.go:7725 checkExpression
    pub fn check_expression(&mut self, node: Node) -> TypeId {
        self.check_expression_ex(node, CheckMode::NORMAL)
    }

    // Go: checker/checker.go:7729 checkExpressionEx
    pub fn check_expression_ex(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let _trace = self.tracer.map(|tr| {
            tr.push(
                crate::tracing::Phase::Check,
                "checkExpression",
                crate::tracing::node_args(node),
                false,
            )
        });
        let save_current_node = self.current_node;
        self.current_node = node;
        self.instantiation_count = 0;
        let uninstantiated_type = self.check_expression_worker(node, check_mode);
        let t = self.instantiate_type_with_single_generic_call_signature(
            node,
            uninstantiated_type,
            check_mode,
        );
        if self.is_const_enum_object_type(t) {
            self.check_const_enum_access(node, t);
        }
        self.current_node = save_current_node;
        t
    }

    // Go: checker/checker.go:7745 checkConstEnumAccess
    pub fn check_const_enum_access(&mut self, node: Node, t: TypeId) {
        // enum object type for const enums are only permitted in:
        // - 'left' in property access
        // - 'object' in indexed access
        // - target in rhs of import statement
        let parent = node.parent();
        let ok = is_property_access_expression(parent) && parent.expression() == node
            || is_element_access_expression(parent) && parent.expression() == node
            || ((is_identifier(node) || is_qualified_name(node))
                && is_in_right_side_of_import_or_export_assignment(node)
                || is_type_query_node(parent) && parent.expr_name() == node)
            || is_export_specifier(parent); // We allow reexporting const enums
        if !ok {
            self.error(
                node,
                diag::X_const_enums_can_only_be_used_in_property_or_index_access_expressions_or_the_right_hand_side_of_an_import_declaration_or_export_assignment_or_type_query,
                args![],
            );
        }
        // --verbatimModuleSyntax only gets checked here when the enum usage does not
        // resolve to an import, because imports of ambient const enums get checked
        // separately in `checkAliasSymbol`.
        if self.compiler_options.isolated_modules.is_true()
            || self.compiler_options.verbatim_module_syntax.is_true()
                && ok
                && self
                    .resolve_name(
                        node,
                        get_first_identifier(node).text(),
                        SymbolFlags::ALIAS,
                        None,
                        false,
                        true,
                    )
                    .is_nil()
        {
            let t_symbol = self.ty(t).symbol;
            debug_assert!(self.sym(t_symbol).flags.intersects(SymbolFlags::CONST_ENUM));
            let const_enum_declaration = self.sym(t_symbol).value_declaration;
            // PORT: Go `c.program.GetProjectReferenceFromOutputDts(file.Path())` is a
            // Program method, ported as a free function; `Path()` reads the
            // `SourceFileInfo.path` field.
            let redirect = get_project_reference_from_output_dts(
                &source_file_info(get_source_file_of_node(const_enum_declaration)).path,
            );
            if const_enum_declaration
                .flags()
                .intersects(NodeFlags::AMBIENT)
                && !is_valid_type_only_alias_use_site(node)
                && (redirect.is_none()
                    || !redirect
                        .as_ref()
                        .unwrap()
                        .resolved
                        .compiler_options()
                        .should_preserve_const_enums())
            {
                let flag_name = self.get_isolated_modules_like_flag_name();
                self.error(
                    node,
                    diag::Cannot_access_ambient_const_enums_when_0_is_enabled,
                    args![flag_name],
                );
            }
        }
    }

    // Go: checker/checker.go:7771 instantiateTypeWithSingleGenericCallSignature
    pub fn instantiate_type_with_single_generic_call_signature(
        &mut self,
        node: Node,
        t: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        if !check_mode.intersects(CheckMode::INFERENTIAL | CheckMode::SKIP_GENERIC_FUNCTIONS) {
            return t;
        }
        let call_signature =
            self.get_single_signature(t, SignatureKind::CALL, true /*allowMembers*/);
        let construct_signature =
            self.get_single_signature(t, SignatureKind::CONSTRUCT, true /*allowMembers*/);
        let signature = if call_signature.is_some() {
            call_signature
        } else {
            construct_signature
        };
        if signature.is_nil() || self.sig(signature).type_parameters.is_empty() {
            return t;
        }
        let contextual_type =
            self.get_apparent_type_of_contextual_type(node, ContextFlags::NO_CONSTRAINTS);
        if contextual_type.is_nil() {
            return t;
        }
        let non_nullable_contextual_type = self.get_non_nullable_type(contextual_type);
        let contextual_signature = self.get_single_signature(
            non_nullable_contextual_type,
            if call_signature.is_some() {
                SignatureKind::CALL
            } else {
                SignatureKind::CONSTRUCT
            },
            false, /*allowMembers*/
        );
        if contextual_signature.is_nil()
            || !self.sig(contextual_signature).type_parameters.is_empty()
        {
            return t;
        }
        if check_mode.intersects(CheckMode::SKIP_GENERIC_FUNCTIONS) {
            self.skipped_generic_function(node, check_mode);
            return self.any_function_type;
        }
        let context = self.get_inference_context(node);
        // We have an expression that is an argument of a generic function for which we are performing
        // type argument inference. The expression is of a function type with a single generic call
        // signature and a contextual function type with a single non-generic call signature. Now check
        // if the outer function returns a function type with a single non-generic call signature and
        // if some of the outer function type parameters have no inferences so far. If so, we can
        // potentially add inferred type parameters to the outer function return type.
        let mut return_signature = SignatureId::NIL;
        let context_signature = self.inference_context(context).signature;
        if context_signature.is_some() {
            let return_type = self.get_return_type_of_signature(context_signature);
            if return_type.is_some() {
                return_signature = self.get_single_call_or_construct_signature(return_type);
            }
        }
        if return_signature.is_some()
            && self.sig(return_signature).type_parameters.is_empty()
            && !self
                .inference_context(context)
                .inferences
                .iter()
                .all(info_has_inference_candidates)
        {
            // Instantiate the signature with its own type parameters as type arguments, possibly
            // renaming the type parameters to ensure they have unique names.
            let signature_type_parameters = self.sig(signature).type_parameters.clone();
            let unique_type_parameters =
                self.get_unique_type_parameters(context, &signature_type_parameters);
            let instantiated_signature = self
                .get_signature_instantiation_without_filling_in_type_arguments(
                    signature,
                    &unique_type_parameters,
                );
            // Infer from the parameters of the instantiated signature to the parameters of the
            // contextual signature starting with an empty set of inference candidates.
            let fresh_inferences: Vec<InferenceInfo> = self
                .inference_context(context)
                .inferences
                .iter()
                .map(|info| new_inference_info(info.type_parameter))
                .collect();
            // PORT: Go passes a fresh `[]*InferenceInfo` slice to `inferTypes`. Rust
            // `inferTypes` takes an inference context id and infers into its
            // `inferences`, so the fresh list lives in a scratch context cloned from
            // `context`. `inferTypes` reads only the inference list.
            let mut scratch = self.inference_context(context).clone();
            scratch.inferences = fresh_inferences.into_boxed_slice();
            let inferences = InferenceContextId(self.inference_contexts.len() as u32);
            self.inference_contexts.push(scratch);
            self.apply_to_parameter_types(
                instantiated_signature,
                contextual_signature,
                &mut |c: &mut Checker, source: TypeId, target: TypeId| {
                    c.infer_types(
                        inferences,
                        source,
                        target,
                        InferencePriority::NONE,
                        true, /*contravariant*/
                    );
                },
            );
            if self
                .inference_context(inferences)
                .inferences
                .iter()
                .any(info_has_inference_candidates)
            {
                // We have inference candidates, indicating that one or more type parameters are referenced
                // in the parameter types of the contextual signature. Now also infer from the return type.
                self.apply_to_return_types(
                    instantiated_signature,
                    contextual_signature,
                    &mut |c: &mut Checker, source: TypeId, target: TypeId| {
                        c.infer_types(inferences, source, target, InferencePriority::NONE, false);
                    },
                );
                // If the type parameters for which we produced candidates do not have any inferences yet,
                // we adopt the new inference candidates and add the type parameters of the expression type
                // to the set of inferred type parameters for the outer function return type.
                // PORT: Go `hasOverlappingInferences(context.inferences, inferences)` inlined.
                let has_overlapping = {
                    let a = &self.inference_context(context).inferences;
                    let b = &self.inference_context(inferences).inferences;
                    (0..a.len()).any(|i| {
                        info_has_inference_candidates(&a[i]) && info_has_inference_candidates(&b[i])
                    })
                };
                if !has_overlapping {
                    // PORT: Go `c.mergeInferences(context.inferences, inferences)` inlined.
                    // Go shares the `*InferenceInfo` pointer; the scratch context is
                    // discarded, so a clone of the value is equivalent.
                    let source_infos = self.inference_context(inferences).inferences.clone();
                    {
                        let target_infos = &mut self.inference_context_mut(context).inferences;
                        for i in 0..target_infos.len() {
                            if !info_has_inference_candidates(&target_infos[i])
                                && info_has_inference_candidates(&source_infos[i])
                            {
                                target_infos[i] = source_infos[i].clone();
                            }
                        }
                    }
                    // PORT: Go `core.Concatenate` returns a new slice whenever
                    // `uniqueTypeParameters` is not empty (the fresh slice from
                    // getUniqueTypeParameters itself, or a copy).
                    // An empty append leaves the list as it is, so it makes no
                    // `rare` box.
                    if !unique_type_parameters.is_empty() {
                        let origin = self.new_type_parameters_origin();
                        let rare = self.inference_context_mut(context).rare_mut();
                        rare.inferred_type_parameters_origin = origin;
                        rare.inferred_type_parameters
                            .extend(unique_type_parameters.iter().copied());
                    }
                    return self.get_or_create_type_from_signature(instantiated_signature);
                }
            }
        }
        // TODO: The signature may reference any outer inference contexts, but we map pop off and then apply new inference contexts,
        // and thus get different inferred types. That this is cached on the *first* such attempt is not currently an issue, since expression
        // types *also* get cached on the first pass. If we ever properly speculate, though, the cached "isolatedSignatureType" signature
        // field absolutely needs to be included in the list of speculative caches.
        let instantiated = self.instantiate_signature_in_context_of(
            signature,
            contextual_signature,
            context,
            None,
        );
        self.get_or_create_type_from_signature(instantiated)
    }

    // Go: checker/checker.go:7843 getOuterInferenceTypeParameters
    pub fn get_outer_inference_type_parameters(&self) -> Vec<TypeId> {
        let mut result: Vec<TypeId> = Vec::new();
        for i in 0..self.inference_context_infos.len() {
            let context = self.inference_context_infos[i].context;
            if context.is_some() {
                for info in &self.inference_context(context).inferences {
                    result.push(info.type_parameter);
                }
            }
        }
        result
    }

    // Go: checker/checker.go:7856 getUniqueTypeParameters
    pub fn get_unique_type_parameters(
        &mut self,
        context: InferenceContextId,
        type_parameters: &[TypeId],
    ) -> Vec<TypeId> {
        let mut old_type_parameters: Vec<TypeId> = Vec::new();
        let mut new_type_parameters: Vec<TypeId> = Vec::new();
        let mut result: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for &tp in type_parameters {
            let name = self.sym(self.ty(tp).symbol).name.clone();
            let inferred_type_parameters = self
                .inference_context(context)
                .inferred_type_parameters()
                .to_vec();
            if self.has_type_parameter_by_name(&inferred_type_parameters, &name)
                || self.has_type_parameter_by_name(&result, &name)
            {
                let mut combined = inferred_type_parameters.clone();
                combined.extend(result.iter().copied());
                let new_name = self.get_unique_type_parameter_name(&combined, &name);
                let symbol = self.new_symbol(SymbolFlags::TYPE_PARAMETER, &new_name);
                let new_type_parameter = self.new_type_parameter(symbol);
                self.ty_mut(new_type_parameter)
                    .as_type_parameter_mut()
                    .target = tp;
                old_type_parameters.push(tp);
                new_type_parameters.push(new_type_parameter);
                result.push(new_type_parameter);
            } else {
                result.push(tp);
            }
        }
        if !new_type_parameters.is_empty() {
            let mapper = self.new_type_mapper(&old_type_parameters, &new_type_parameters);
            for &tp in &new_type_parameters {
                self.ty_mut(tp).as_type_parameter_mut().mapper = mapper;
            }
        }
        result
    }

    // Go: checker/checker.go:7883 hasTypeParameterByName
    pub fn has_type_parameter_by_name(&self, type_parameters: &[TypeId], name: &str) -> bool {
        type_parameters
            .iter()
            .any(|&tp| self.sym(self.ty(tp).symbol).name == name)
    }

    // Go: checker/checker.go:7889 getUniqueTypeParameterName
    pub fn get_unique_type_parameter_name(
        &self,
        type_parameters: &[TypeId],
        base_name: &str,
    ) -> String {
        let mut base_name = base_name;
        while base_name.len() > 1 && {
            let last = base_name.as_bytes()[base_name.len() - 1];
            last >= b'0' && last <= b'9'
        } {
            base_name = &base_name[..base_name.len() - 1];
        }
        let mut index = 1;
        loop {
            let augmented_name = format!("{}{}", base_name, index);
            if !self.has_type_parameter_by_name(type_parameters, &augmented_name) {
                return augmented_name;
            }
            index += 1;
        }
    }

    // Go: checker/checker.go:7903 checkExpressionWorker
    pub fn check_expression_worker(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        match node.kind() {
            SyntaxKind::Identifier => return self.check_identifier(node, check_mode),
            SyntaxKind::PrivateIdentifier => return self.check_private_identifier_expression(node),
            SyntaxKind::ThisKeyword => return self.check_this_expression(node),
            SyntaxKind::SuperKeyword => return self.check_super_expression(node),
            SyntaxKind::NullKeyword => return self.null_widening_type,
            SyntaxKind::StringLiteral | SyntaxKind::NoSubstitutionTemplateLiteral => {
                if self.is_skip_direct_inference_node(node) {
                    return self.blocked_string_type;
                }
                let t = self.get_string_literal_type(node.text());
                return self.get_fresh_type_of_literal_type(t);
            }
            SyntaxKind::NumericLiteral => {
                self.check_grammar_numeric_literal(node);
                let t = self.get_number_literal_type(crate::jsnum::from_string(node.text()));
                return self.get_fresh_type_of_literal_type(t);
            }
            SyntaxKind::BigIntLiteral => {
                self.check_grammar_big_int_literal(node);
                let t = self.get_big_int_literal_type(crate::jsnum::PseudoBigInt::new(
                    &crate::jsnum::parse_pseudo_big_int(node.text()),
                    false, /*negative*/
                ));
                return self.get_fresh_type_of_literal_type(t);
            }
            SyntaxKind::TrueKeyword => return self.true_type,
            SyntaxKind::FalseKeyword => return self.false_type,
            SyntaxKind::TemplateExpression => return self.check_template_expression(node),
            SyntaxKind::RegularExpressionLiteral => {
                return self.check_regular_expression_literal(node);
            }
            SyntaxKind::ArrayLiteralExpression => {
                return self.check_array_literal(node, check_mode);
            }
            SyntaxKind::ObjectLiteralExpression => {
                return self.check_object_literal(node, check_mode);
            }
            SyntaxKind::PropertyAccessExpression => {
                return self
                    .check_property_access_expression(node, check_mode, false /*writeOnly*/);
            }
            SyntaxKind::QualifiedName => return self.check_qualified_name(node, check_mode),
            SyntaxKind::ElementAccessExpression => {
                return self.check_indexed_access(node, check_mode);
            }
            SyntaxKind::CallExpression => {
                if is_import_call(node) {
                    return self.check_import_call_expression(node);
                }
                return self.check_call_expression(node, check_mode);
            }
            SyntaxKind::NewExpression => return self.check_call_expression(node, check_mode),
            SyntaxKind::TaggedTemplateExpression => {
                return self.check_tagged_template_expression(node);
            }
            SyntaxKind::ParenthesizedExpression => {
                return self.check_parenthesized_expression(node, check_mode);
            }
            SyntaxKind::ClassExpression => return self.check_class_expression(node),
            SyntaxKind::FunctionExpression | SyntaxKind::ArrowFunction => {
                return self.check_function_expression_or_object_literal_method(node, check_mode);
            }
            SyntaxKind::TypeAssertionExpression | SyntaxKind::AsExpression => {
                return self.check_assertion(node, check_mode);
            }
            SyntaxKind::TypeOfExpression => return self.check_type_of_expression(node),
            SyntaxKind::NonNullExpression => return self.check_non_null_assertion(node),
            SyntaxKind::ExpressionWithTypeArguments => {
                return self.check_expression_with_type_arguments(node);
            }
            SyntaxKind::SatisfiesExpression => return self.check_satisfies_expression(node),
            SyntaxKind::MetaProperty => return self.check_meta_property(node),
            SyntaxKind::DeleteExpression => return self.check_delete_expression(node),
            SyntaxKind::VoidExpression => return self.check_void_expression(node),
            SyntaxKind::AwaitExpression => return self.check_await_expression(node),
            SyntaxKind::PrefixUnaryExpression => return self.check_prefix_unary_expression(node),
            SyntaxKind::PostfixUnaryExpression => return self.check_postfix_unary_expression(node),
            SyntaxKind::BinaryExpression => return self.check_binary_expression(node, check_mode),
            SyntaxKind::ConditionalExpression => {
                return self.check_conditional_expression(node, check_mode);
            }
            SyntaxKind::SpreadElement => return self.check_spread_expression(node, check_mode),
            SyntaxKind::OmittedExpression => return self.undefined_widening_type,
            SyntaxKind::YieldExpression => return self.check_yield_expression(node),
            SyntaxKind::SyntheticExpression => return self.check_synthetic_expression(node),
            SyntaxKind::JsxExpression => return self.check_jsx_expression(node, check_mode),
            SyntaxKind::JsxElement => return self.check_jsx_element(node, check_mode),
            SyntaxKind::JsxSelfClosingElement => {
                return self.check_jsx_self_closing_element(node, check_mode);
            }
            SyntaxKind::JsxFragment => return self.check_jsx_fragment(node),
            SyntaxKind::JsxAttributes => return self.check_jsx_attributes(node, check_mode),
            SyntaxKind::JsxOpeningElement => {
                panic!("Should never directly check a JsxOpeningElement")
            }
            _ => {}
        }
        self.error_type
    }

    // Go: checker/checker.go:8009 checkPrivateIdentifierExpression
    pub fn check_private_identifier_expression(&mut self, node: Node) -> TypeId {
        self.check_grammar_private_identifier_expression(node);
        let symbol = self.get_symbol_for_private_identifier_expression(node);
        if symbol.is_some() {
            self.mark_property_as_referenced(
                symbol,
                Node::NIL, /*nodeForCheckWriteOnly*/
                false,     /*isSelfTypeAccess*/
            );
        }
        self.any_type
    }

    // Go: checker/checker.go:8018 getSymbolForPrivateIdentifierExpression
    pub fn get_symbol_for_private_identifier_expression(&mut self, node: Node) -> SymbolId {
        if self.symbol_node_links.get(node).resolved_symbol.is_nil() {
            let resolved = self.lookup_symbol_for_private_identifier_declaration(node.text(), node);
            self.symbol_node_links.get(node).resolved_symbol = resolved;
        }
        self.symbol_node_links.get(node).resolved_symbol
    }

    // Go: checker/checker.go:8026 checkSuperExpression
    pub fn check_super_expression(&mut self, node: Node) -> TypeId {
        let is_call_expression =
            is_call_expression(node.parent()) && node.parent().expression() == node;
        let immediate_container = get_super_container(node, true /*stopOnFunctions*/);
        let mut container = immediate_container;

        // adjust the container reference in case if super is used inside arrow functions with arbitrarily deep nesting
        if !is_call_expression {
            while container.is_some() && is_arrow_function(container) {
                container = get_super_container(container, true /*stopOnFunctions*/);
            }
        }

        let is_legal_usage_of_super_expression = |container: Node| -> bool {
            if is_call_expression {
                // TS 1.0 SPEC (April 2014): 4.8.1
                // Super calls are only permitted in constructors of derived classes
                return is_constructor_declaration(container);
            }
            // TS 1.0 SPEC (April 2014)
            // 'super' property access is allowed
            // - In a constructor, instance member function, instance member accessor, or instance member variable initializer where this references a derived class instance
            // - In a static member function or static member accessor

            // topmost container must be something that is directly nested in the class declaration\object literal expression
            if is_class_like(container.parent()) || is_object_literal_expression(container.parent())
            {
                if is_static(container) {
                    return node_kind_is(
                        container,
                        &[
                            SyntaxKind::MethodDeclaration,
                            SyntaxKind::MethodSignature,
                            SyntaxKind::GetAccessor,
                            SyntaxKind::SetAccessor,
                            SyntaxKind::PropertyDeclaration,
                            SyntaxKind::ClassStaticBlockDeclaration,
                        ],
                    );
                }
                return node_kind_is(
                    container,
                    &[
                        SyntaxKind::MethodDeclaration,
                        SyntaxKind::MethodSignature,
                        SyntaxKind::GetAccessor,
                        SyntaxKind::SetAccessor,
                        SyntaxKind::PropertyDeclaration,
                        SyntaxKind::PropertySignature,
                        SyntaxKind::Constructor,
                    ],
                );
            }
            false
        };

        if container.is_nil() || !is_legal_usage_of_super_expression(container) {
            // issue more specific error if super is used in computed property name
            // class A { foo() { return "1" }}
            // class B {
            //     [super.foo()]() {}
            // }
            let current = find_ancestor_or_quit(node, |n: Node| {
                if n == container {
                    return FindAncestorResult::FIND_ANCESTOR_QUIT;
                }
                if is_computed_property_name(n) {
                    return FindAncestorResult::FIND_ANCESTOR_TRUE;
                }
                FindAncestorResult::FIND_ANCESTOR_FALSE
            });
            if current.is_some() && is_computed_property_name(current) {
                self.error(
                    node,
                    diag::X_super_cannot_be_referenced_in_a_computed_property_name,
                    args![],
                );
            } else if is_call_expression {
                self.error(
                    node,
                    diag::Super_calls_are_not_permitted_outside_constructors_or_in_nested_functions_inside_constructors,
                    args![],
                );
            } else if container.is_nil()
                || container.parent().is_nil()
                || !(is_class_like(container.parent())
                    || is_object_literal_expression(container.parent()))
            {
                self.error(
                    node,
                    diag::X_super_can_only_be_referenced_in_members_of_derived_classes_or_object_literal_expressions,
                    args![],
                );
            } else {
                self.error(
                    node,
                    diag::X_super_property_access_is_permitted_only_in_a_constructor_member_function_or_member_accessor_of_a_derived_class,
                    args![],
                );
            }
            return self.error_type;
        }
        if !is_call_expression && is_constructor_declaration(immediate_container) {
            self.check_this_before_super(
                node,
                container,
                diag::X_super_must_be_called_before_accessing_a_property_of_super_in_the_constructor_of_a_derived_class,
            );
        }
        if container.parent().kind() == SyntaxKind::ObjectLiteralExpression {
            // for object literal assume that type of 'super' is 'any'
            return self.any_type;
        }
        // at this point the only legal case for parent is ClassLikeDeclaration
        let class_like_declaration = container.parent();
        if get_class_extends_heritage_element(class_like_declaration).is_nil() {
            self.error(
                node,
                diag::X_super_can_only_be_referenced_in_a_derived_class,
                args![],
            );
            return self.error_type;
        }
        if self.class_declaration_extends_null(class_like_declaration) {
            if is_call_expression {
                return self.error_type;
            }
            return self.null_widening_type;
        }
        let class_symbol = self.get_symbol_of_declaration(class_like_declaration);
        let class_type = self.get_declared_type_of_symbol(class_symbol);
        let mut base_class_type = TypeId::NIL;
        if class_type.is_some() {
            let base_types = self.get_base_types(class_type);
            base_class_type = base_types.first().copied().unwrap_or(TypeId::NIL);
        }
        if base_class_type.is_nil() {
            return self.error_type;
        }
        if is_constructor_declaration(container)
            && self.is_in_constructor_argument_initializer(node, container)
        {
            // issue custom error message for super property access in constructor arguments (to be aligned with old compiler)
            self.error(
                node,
                diag::X_super_cannot_be_referenced_in_constructor_arguments,
                args![],
            );
            return self.error_type;
        }
        if is_static(container) || is_call_expression {
            if !is_call_expression
                && self.language_version <= ScriptTarget::ES2021
                && (is_property_declaration(container)
                    || is_class_static_block_declaration(container))
            {
                // for `super.x` or `super[x]` in a static initializer, mark all enclosing
                // block scope containers so that we can report potential collisions with
                // `Reflect`.
                let mut current = get_enclosing_block_scope_container(node.parent());
                while current.is_some() {
                    if !is_source_file(current) || is_external_or_common_js_module(current) {
                        self.node_links.get(current).flags |=
                            NodeCheckFlags::CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER;
                    }
                    current = get_enclosing_block_scope_container(current);
                }
            }
            return self.get_base_constructor_type_of_class(class_type);
        }
        let this_type = self.ty(class_type).as_interface_type().this_type;
        self.get_type_with_this_argument(base_class_type, this_type, false)
    }

    // Go: checker/checker.go:8136 isInConstructorArgumentInitializer
    pub fn is_in_constructor_argument_initializer(
        &mut self,
        node: Node,
        constructor_decl: Node,
    ) -> bool {
        find_ancestor_or_quit(node, |n: Node| {
            if is_function_like_declaration(n) {
                return FindAncestorResult::FIND_ANCESTOR_QUIT;
            }
            if is_parameter_declaration(n) && n.parent() == constructor_decl {
                return FindAncestorResult::FIND_ANCESTOR_TRUE;
            }
            FindAncestorResult::FIND_ANCESTOR_FALSE
        })
        .is_some()
    }

    // Go: checker/checker.go:8148 checkTemplateExpression
    pub fn check_template_expression(&mut self, node: Node) -> TypeId {
        let template_spans = node.template_spans().nodes().to_vec();
        let length = template_spans.len();
        let mut texts: Vec<String> = vec![String::new(); length + 1];
        let mut types: Vec<TypeId> = vec![TypeId::NIL; length];
        texts[0] = node.head().text().to_string();
        for (i, &span) in template_spans.iter().enumerate() {
            let t = self.check_expression(span.expression());
            if self.maybe_type_of_kind_considering_base_constraint(t, TypeFlags::ES_SYMBOL_LIKE) {
                self.error(
                    span.expression(),
                    diag::Implicit_conversion_of_a_symbol_to_a_string_will_fail_at_runtime_Consider_wrapping_this_expression_in_String,
                    args![],
                );
            }
            texts[i + 1] = span.literal().text().to_string();
            types[i] = if self.is_type_assignable_to(t, self.template_constraint_type) {
                t
            } else {
                self.string_type
            };
        }
        let mut evaluated: Option<LiteralValue> = None;
        if !is_tagged_template_expression(node.parent()) {
            let ev = self.evaluate.clone();
            evaluated = ev(self, node, node).value;
        }
        if let Some(value) = evaluated {
            // Go: `evaluated.(string)` type assertion; panics on any other value.
            let text = match value {
                LiteralValue::String(s) => s,
                _ => panic!("interface conversion: interface {{}} is not string"),
            };
            let t = self.get_string_literal_type(&text);
            return self.get_fresh_type_of_literal_type(t);
        }
        if self.is_const_context(node) || self.is_template_literal_context(node) || {
            let contextual_type = self.get_contextual_type(node, ContextFlags::NONE);
            let t = if contextual_type.is_some() {
                contextual_type
            } else {
                self.unknown_type
            };
            self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.is_template_literal_contextual_type(t)
            })
        } {
            return self.get_template_literal_type(&texts, &types);
        }
        self.string_type
    }

    // Go: checker/checker.go:8175 isTemplateLiteralContext
    pub fn is_template_literal_context(&mut self, node: Node) -> bool {
        let parent = node.parent();
        is_parenthesized_expression(parent) && self.is_template_literal_context(parent)
            || is_element_access_expression(parent) && parent.argument_expression() == node
    }

    // Go: checker/checker.go:8180 isTemplateLiteralContextualType
    pub fn is_template_literal_contextual_type(&mut self, t: TypeId) -> bool {
        let flags = self.ty(t).flags;
        flags.intersects(TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL)
            || flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) && {
                let constraint = self.get_base_constraint_of_type(t);
                let constraint = if constraint.is_some() {
                    constraint
                } else {
                    self.unknown_type
                };
                self.maybe_type_of_kind(constraint, TypeFlags::STRING_LIKE)
            }
    }

    // Go: checker/checker.go:8184 checkRegularExpressionLiteral
    pub fn check_regular_expression_literal(&mut self, node: Node) -> TypeId {
        if !self
            .node_links
            .get(node)
            .flags
            .intersects(NodeCheckFlags::TYPE_CHECKED)
        {
            self.node_links.get(node).flags |= NodeCheckFlags::TYPE_CHECKED;
            self.check_grammar_regular_expression_literal(node);
        }
        self.global_reg_exp_type
    }

    // Go: checker/checker.go:8193 checkArrayLiteral
    pub fn check_array_literal(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let elements = node.elements();
        let mut element_types: Vec<TypeId> = vec![TypeId::NIL; elements.len()];
        let mut element_infos: Vec<TupleElementInfo> =
            vec![TupleElementInfo::default(); elements.len()];
        self.push_cached_contextual_type(node);
        let in_destructuring_pattern = is_assignment_target(node);
        let in_const_context = self.is_const_context(node);
        let contextual_type = self.get_apparent_type_of_contextual_type(node, ContextFlags::NONE);
        let in_tuple_context = is_spread_into_call_or_new(node)
            || contextual_type.is_some()
                && self.some_type(contextual_type, &mut |c: &mut Checker, t: TypeId| {
                    c.is_tuple_like_type(t)
                        || c.is_generic_mapped_type(t)
                            && c.ty(t).as_mapped_type().name_type.is_nil()
                            && {
                                let target = c.ty(t).as_mapped_type().object.target;
                                let base = if target.is_some() { target } else { t };
                                c.get_homomorphic_type_variable(base).is_some()
                            }
                });
        let mut has_omitted_expression = false;
        for (i, e) in elements.iter().enumerate() {
            if is_spread_element(e) {
                let spread_type = self.check_expression_ex(e.expression(), check_mode);
                if self.is_array_like_type(spread_type) {
                    element_types[i] = spread_type;
                    element_infos[i] = TupleElementInfo {
                        flags: ElementFlags::VARIADIC,
                        ..Default::default()
                    };
                } else if in_destructuring_pattern {
                    // Given the following situation:
                    //    var c: {};
                    //    [...c] = ["", 0];
                    //
                    // c is represented in the tree as a spread element in an array literal.
                    // But c really functions as a rest element, and its purpose is to provide
                    // a contextual type for the right hand side of the assignment. Therefore,
                    // instead of calling checkExpression on "...c", which will give an error
                    // if c is not iterable/array-like, we need to act as if we are trying to
                    // get the contextual element type from it. So we do something similar to
                    // getContextualTypeForElementExpression, which will crucially not error
                    // if there is no index type / iterated type.
                    let mut rest_element_type =
                        self.get_index_type_of_type(spread_type, self.number_type);
                    if rest_element_type.is_nil() {
                        rest_element_type = self.get_iterated_type_or_element_type(
                            IterationUse::DESTRUCTURING,
                            spread_type,
                            self.undefined_type,
                            Node::NIL, /*errorNode*/
                            false,     /*checkAssignability*/
                        );
                        if rest_element_type.is_nil() {
                            rest_element_type = self.unknown_type;
                        }
                    }
                    element_types[i] = rest_element_type;
                    element_infos[i] = TupleElementInfo {
                        flags: ElementFlags::REST,
                        ..Default::default()
                    };
                } else {
                    element_types[i] = self.check_iterated_type_or_element_type(
                        IterationUse::SPREAD,
                        spread_type,
                        self.undefined_type,
                        e.expression(),
                    );
                    element_infos[i] = TupleElementInfo {
                        flags: ElementFlags::REST,
                        ..Default::default()
                    };
                }
            } else if self.exact_optional_property_types && is_omitted_expression(e) {
                has_omitted_expression = true;
                element_types[i] = self.undefined_or_missing_type;
                element_infos[i] = TupleElementInfo {
                    flags: ElementFlags::OPTIONAL,
                    ..Default::default()
                };
            } else {
                let t = self.check_expression_for_mutable_location(e, check_mode);
                element_types[i] =
                    self.add_optionality_ex(t, true /*isProperty*/, has_omitted_expression);
                element_infos[i] = TupleElementInfo {
                    flags: if has_omitted_expression {
                        ElementFlags::OPTIONAL
                    } else {
                        ElementFlags::REQUIRED
                    },
                    ..Default::default()
                };
                if in_tuple_context
                    && check_mode.intersects(CheckMode::INFERENTIAL)
                    && !check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
                    && self.is_context_sensitive(e)
                {
                    let inference_context = self.get_inference_context(node);
                    // In CheckMode.Inferential we should always have an inference context
                    self.add_intra_expression_inference_site(inference_context, e, t);
                }
            }
        }
        self.pop_contextual_type();
        if in_destructuring_pattern {
            return self.create_tuple_type_ex(&element_types, &element_infos, false);
        }
        if check_mode.intersects(CheckMode::FORCE_TUPLE) || in_const_context || in_tuple_context {
            let readonly = in_const_context
                && !(contextual_type.is_some()
                    && self.some_type(contextual_type, &mut |c: &mut Checker, t: TypeId| {
                        c.is_mutable_array_like_type(t)
                    }));
            let tuple = self.create_tuple_type_ex(
                &element_types,
                &element_infos,
                readonly, /*readonly*/
            );
            return self.create_array_literal_type(tuple);
        }
        let element_type;
        if !element_types.is_empty() {
            for i in 0..element_types.len() {
                if element_infos[i].flags.intersects(ElementFlags::VARIADIC) {
                    let e = element_types[i];
                    let indexed = self.get_indexed_access_type_or_undefined(
                        e,
                        self.number_type,
                        AccessFlags::NONE,
                        Node::NIL,
                        None,
                    );
                    element_types[i] = if indexed.is_some() {
                        indexed
                    } else {
                        self.any_type
                    };
                }
            }
            element_type =
                self.get_union_type_ex(&element_types, UnionReduction::SUBTYPE, None, TypeId::NIL);
        } else {
            element_type = if self.strict_null_checks {
                self.implicit_never_type
            } else {
                self.undefined_widening_type
            };
        }
        let array_type = self.create_array_type_ex(element_type, in_const_context);
        self.create_array_literal_type(array_type)
    }

    // Go: checker/checker.go:8275 createArrayLiteralType
    pub fn create_array_literal_type(&mut self, t: TypeId) -> TypeId {
        if !self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::ARRAY_LITERAL_TYPE,
            type_id: self.ty(t).id,
        };
        if let Some(&cached) = self.cached_types.get(&key) {
            return cached;
        }
        let literal_type = self.clone_type_reference(t);
        self.ty_mut(literal_type).object_flags |=
            ObjectFlags::ARRAY_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        self.cached_types.insert(key, literal_type);
        literal_type
    }
}

// Go: checker/checker.go:8289 isSpreadIntoCallOrNew
pub fn is_spread_into_call_or_new(node: Node) -> bool {
    let parent = walk_up_parenthesized_expressions(node.parent());
    is_spread_element(parent) && is_call_or_new_expression(parent.parent())
}

impl Checker {
    // Go: checker/checker.go:8294 checkQualifiedName
    pub fn check_qualified_name(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let left = node.left();
        let left_type;
        if is_part_of_type_query(node) && is_this_identifier(left) {
            let this_type = self.check_this_expression(left);
            left_type = self.check_non_null_type(this_type, left);
        } else {
            left_type = self.check_non_null_expression(left);
        }
        self.check_property_access_expression_or_qualified_name(
            node,
            left,
            left_type,
            node.right(),
            check_mode,
            false,
        )
    }

    // Go: checker/checker.go:8305 checkIndexedAccess
    pub fn check_indexed_access(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        if node.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
            return self.check_element_access_chain(node, check_mode);
        }
        let expr_type = self.check_non_null_expression(node.expression());
        self.check_element_access_expression(node, expr_type, check_mode)
    }

    // Go: checker/checker.go:8312 checkElementAccessChain
    pub fn check_element_access_chain(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        let expr_type = self.check_expression(node.expression());
        let non_optional_type = self.get_optional_expression_type(expr_type, node.expression());
        let non_null_type = self.check_non_null_type(non_optional_type, node.expression());
        let access_type = self.check_element_access_expression(node, non_null_type, check_mode);
        self.propagate_optional_type_marker(access_type, node, non_optional_type != expr_type)
    }

    // Go: checker/checker.go:8318 checkElementAccessExpression
    pub fn check_element_access_expression(
        &mut self,
        node: Node,
        expr_type: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut object_type = expr_type;
        if get_assignment_target_kind(node) != AssignmentKind::NONE
            || self.is_method_access_for_call(node)
        {
            object_type = self.get_widened_type(object_type);
        }
        let index_expression = node.argument_expression();
        let index_type = self.check_expression(index_expression);
        if self.is_error_type(object_type) || object_type == self.silent_never_type {
            return object_type;
        }
        if self.is_const_enum_object_type(object_type) && !is_string_literal_like(index_expression)
        {
            self.error(
                index_expression,
                diag::A_const_enum_member_can_only_be_accessed_using_a_string_literal,
                args![],
            );
            return self.error_type;
        }
        let mut effective_index_type = index_type;
        if self.is_for_in_variable_for_numeric_property_names(index_expression) {
            effective_index_type = self.number_type;
        }
        let assignment_target_kind = get_assignment_target_kind(node);
        let access_flags;
        if assignment_target_kind == AssignmentKind::NONE {
            access_flags = AccessFlags::EXPRESSION_POSITION;
        } else {
            let mut flags = AccessFlags::WRITING;
            if assignment_target_kind == AssignmentKind::COMPOUND {
                flags |= AccessFlags::EXPRESSION_POSITION;
            }
            if self.is_generic_object_type(object_type) && !self.is_this_type_parameter(object_type)
            {
                flags |= AccessFlags::NO_INDEX_SIGNATURES;
            }
            access_flags = flags;
        }
        let indexed = self.get_indexed_access_type_or_undefined(
            object_type,
            effective_index_type,
            access_flags,
            node,
            None,
        );
        let indexed_access_type = if indexed.is_some() {
            indexed
        } else {
            self.error_type
        };
        let resolved_symbol = self.get_resolved_symbol_or_nil(node);
        let flow_type = self.get_flow_type_of_access_expression(
            node,
            resolved_symbol,
            indexed_access_type,
            index_expression,
            check_mode,
        );
        self.check_indexed_access_index_type(flow_type, node)
    }

    // Return true if given node is an expression consisting of an identifier (possibly parenthesized)
    // that references a for-in variable for an object with numeric property names.
    // Go: checker/checker.go:8351 isForInVariableForNumericPropertyNames
    pub fn is_for_in_variable_for_numeric_property_names(&mut self, expr: Node) -> bool {
        let e = skip_parentheses(expr);
        if is_identifier(e) {
            let symbol = self.get_resolved_symbol(e);
            if self.sym(symbol).flags.intersects(SymbolFlags::VARIABLE) {
                let mut child = expr;
                let mut node = expr.parent();
                while node.is_some() {
                    if is_for_in_statement(node)
                        && child == node.statement()
                        && self.get_for_in_variable_symbol(node) == symbol
                        && {
                            let t = self.get_type_of_expression(node.expression());
                            self.has_numeric_property_names(t)
                        }
                    {
                        return true;
                    }
                    child = node;
                    node = node.parent();
                }
            }
        }
        false
    }

    // Return the symbol of the for-in variable declared or referenced by the given for-in statement.
    // Go: checker/checker.go:8371 getForInVariableSymbol
    pub fn get_for_in_variable_symbol(&mut self, node: Node) -> SymbolId {
        let initializer = node.initializer();
        if is_variable_declaration_list(initializer) {
            let declarations = initializer.declarations().nodes();
            if declarations.len() > 0 {
                let variable = declarations.get(0);
                if !variable.is_nil() && !is_binding_pattern(variable.name()) {
                    return self.get_symbol_of_declaration(variable);
                }
            }
        } else if is_identifier(initializer) {
            return self.get_resolved_symbol(initializer);
        }
        SymbolId::NIL
    }

    // Return true if the given type is considered to have numeric property names.
    // Go: checker/checker.go:8388 hasNumericPropertyNames
    pub fn has_numeric_property_names(&mut self, t: TypeId) -> bool {
        self.get_index_infos_of_type(t).len() == 1
            && self.get_index_info_of_type(t, self.number_type).is_some()
    }

    // Go: checker/checker.go:8392 checkIndexedAccessIndexType
    pub fn check_indexed_access_index_type(&mut self, t: TypeId, access_node: Node) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return t;
        }
        // Check if the index type is assignable to 'keyof T' for the object type.
        let object_type = self.ty(t).as_indexed_access_type().object_type;
        let index_type = self.ty(t).as_indexed_access_type().index_type;
        // skip index type deferral on remapping mapped types
        let object_index_type;
        if self.is_generic_mapped_type(object_type)
            && self.get_mapped_type_name_type_kind(object_type) == MappedTypeNameTypeKind::REMAPPING
        {
            object_index_type = self.get_index_type_for_mapped_type(object_type, IndexFlags::NONE);
        } else {
            object_index_type = self.get_index_type_ex(object_type, IndexFlags::NONE);
        }
        let has_number_index_info = self
            .get_index_info_of_type(object_type, self.number_type)
            .is_some();
        if self.every_type(index_type, &mut |c: &mut Checker, t: TypeId| {
            c.is_type_assignable_to(t, object_index_type)
                || has_number_index_info && c.is_applicable_index_type(t, c.number_type)
        }) {
            if access_node.kind() == SyntaxKind::ElementAccessExpression
                && is_assignment_target(access_node)
                && self
                    .ty(object_type)
                    .object_flags
                    .intersects(ObjectFlags::MAPPED)
                && self
                    .get_mapped_type_modifiers(object_type)
                    .intersects(MappedTypeModifiers::INCLUDE_READONLY)
            {
                let type_string = self.type_to_string_exported(object_type);
                self.error(
                    access_node,
                    diag::Index_signature_in_type_0_only_permits_reading,
                    args![type_string],
                );
            }
            return t;
        }
        if self.is_generic_object_type(object_type) {
            let property_name = self.get_property_name_from_index(index_type, access_node);
            if property_name != INTERNAL_SYMBOL_NAME_MISSING {
                let property_symbol = self.get_constituent_property(object_type, &property_name);
                if property_symbol.is_some()
                    && self
                        .get_declaration_modifier_flags_from_symbol(property_symbol)
                        .intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER)
                {
                    self.error(
                        access_node,
                        diag::Private_or_protected_member_0_cannot_be_accessed_on_a_type_parameter,
                        args![property_name],
                    );
                    return self.error_type;
                }
            }
        }
        let index_string = self.type_to_string_exported(index_type);
        let object_string = self.type_to_string_exported(object_type);
        self.error(
            access_node,
            diag::Type_0_cannot_be_used_to_index_type_1,
            args![index_string, object_string],
        );
        self.error_type
    }

    // Go: checker/checker.go:8429 getConstituentProperty
    pub fn get_constituent_property(
        &mut self,
        object_type: TypeId,
        property_name: &str,
    ) -> SymbolId {
        let apparent_type = self.get_apparent_type(object_type);
        let distributed = self.ty(apparent_type).distributed();
        for t in distributed {
            let prop = self.get_property_of_type(t, property_name);
            if prop.is_some() {
                return prop;
            }
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:8439 checkImportCallExpression
    pub fn check_import_call_expression(&mut self, node: Node) -> TypeId {
        // Check grammar of dynamic import
        self.check_grammar_import_call_expression(node);
        let args = node.arguments().to_vec();
        if args.is_empty() {
            // No call arguments exist, so there are no child expressions to check.
            return self.create_promise_return_type(node, self.any_type);
        }
        let specifier = args[0];
        let specifier_type = self.check_expression_cached(specifier);
        let mut options_type = TypeId::NIL;
        if args.len() > 1 {
            options_type = self.check_expression_cached(args[1]);
        }
        // Even though multiple arguments is grammatically incorrect, type-check extra arguments for completion
        for i in 2..args.len() {
            self.check_expression_cached(args[i]);
        }
        if self
            .ty(specifier_type)
            .flags
            .intersects(TypeFlags::NULLABLE)
            || !self.is_type_assignable_to(specifier_type, self.string_type)
        {
            let type_string = self.type_to_string_exported(specifier_type);
            self.error(
                specifier,
                diag::Dynamic_import_s_specifier_must_be_of_type_string_but_here_has_type_0,
                args![type_string],
            );
        }
        let mut import_attributes_type = TypeId::NIL;
        if options_type.is_some() {
            let import_call_options_type = self.get_global_import_call_options_type_checked();
            if import_call_options_type != self.empty_object_type {
                let nullable =
                    self.get_nullable_type(import_call_options_type, TypeFlags::UNDEFINED);
                self.check_type_assignable_to(options_type, nullable, args[1], None);
            }
            if is_object_literal_expression(args[1]) {
                for prop in args[1].properties() {
                    if is_property_assignment(prop)
                        && is_identifier(prop.name())
                        && prop.name().text() == "assert"
                    {
                        self.error(
                            prop.name(),
                            diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert,
                            args![],
                        );
                        break;
                    }
                }
            }
            import_attributes_type = self.get_type_of_property_of_type(options_type, "with");
        }
        // ts#63915, Go N' checker.go:8506
        if is_source_phase_import_call(node) {
            let abstract_module_source_type = self.get_global_abstract_module_source_type();
            return self.create_promise_return_type(node, abstract_module_source_type);
        }
        // resolveExternalModuleName will return undefined if the moduleReferenceExpression is not a string literal
        let module_symbol = self.resolve_external_module_name(
            node,
            specifier,
            false, /*ignoreErrors*/
            import_attributes_type,
        );
        if module_symbol.is_some() {
            let es_module_symbol =
                self.resolve_external_module_symbol(module_symbol, true /*dontResolveAlias*/);
            if es_module_symbol.is_some() {
                let es_module_type = self.get_type_of_symbol(es_module_symbol);
                let mut synthetic_type = self.get_type_with_synthetic_default_only(
                    es_module_type,
                    es_module_symbol,
                    module_symbol,
                    specifier,
                    import_attributes_type,
                );
                if synthetic_type.is_nil() {
                    let es_module_type = self.get_type_of_symbol(es_module_symbol);
                    synthetic_type = self.get_type_with_synthetic_default_import_type(
                        es_module_type,
                        es_module_symbol,
                        module_symbol,
                        specifier,
                    );
                }
                return self.create_promise_return_type(node, synthetic_type);
            }
        }
        self.create_promise_return_type(node, self.any_type)
    }

    /**
     * Syntactically and semantically checks a call or new expression.
     * @param node The call/new expression to be checked.
     * @returns On success, the expression's signature's return type. On failure, anyType.
     */
    // Go: checker/checker.go:8496 checkCallExpression
    pub fn check_call_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_grammar_type_arguments(node, node.type_argument_list());
        let signature =
            self.get_resolved_signature(node, None /*candidatesOutArray*/, check_mode);
        if signature == self.resolving_signature {
            // CheckMode.SkipGenericFunctions is enabled and this is a call to a generic function that
            // returns a function type. We defer checking and return silentNeverType.
            return self.silent_never_type;
        }
        self.check_deprecated_signature(signature, node);
        if node.expression().kind() == SyntaxKind::SuperKeyword {
            return self.void_type;
        }
        if is_new_expression(node) {
            let declaration = self.sig(signature).declaration;
            if declaration.is_some()
                && !is_constructor_declaration(declaration)
                && !is_construct_signature_declaration(declaration)
                && !is_constructor_type_node(declaration)
            {
                // When resolved signature is a call signature (and not a construct signature) the result type is any
                if self.no_implicit_any {
                    self.error(
                        node,
                        diag::X_new_expression_whose_target_lacks_a_construct_signature_implicitly_has_an_any_type,
                        args![],
                    );
                }
                return self.any_type;
            }
        }
        if is_in_js_file(node) && self.is_common_js_require(node) {
            return self.resolve_external_module_type_by_literal(node.arguments().get(0));
        }
        let return_type = self.get_return_type_of_signature(signature);
        // Treat any call to the global 'Symbol' function that is part of a const variable or readonly property
        // as a fresh unique symbol literal type.
        if self
            .ty(return_type)
            .flags
            .intersects(TypeFlags::ES_SYMBOL_LIKE)
            && self.is_symbol_or_symbol_for_call(node)
        {
            return self.get_es_symbol_like_type_for_node(walk_up_parenthesized_expressions(
                node.parent(),
            ));
        }
        if is_call_expression(node)
            && node.question_dot_token().is_nil()
            && is_expression_statement(node.parent())
            && self.ty(return_type).flags.intersects(TypeFlags::VOID)
            && self.get_type_predicate_of_signature(signature).is_some()
        {
            if !is_dotted_name(node.expression()) {
                self.error(
                    node.expression(),
                    diag::Assertions_require_the_call_target_to_be_an_identifier_or_qualified_name,
                    args![],
                );
            } else if self.get_effects_signature(node).is_nil() {
                // PORT: Go adds the diagnostic (`c.error`), then `getTypeOfDottedName`
                // adds related info to the stored one (#4825: it can be an equal one
                // added before). The stored `&mut Diagnostic` cannot be kept across
                // that walk, so the walk runs first and collects the related info,
                // and it goes on the stored diagnostic after the add. The add still
                // compares the diagnostic without this info, as in Go.
                let mut related_info = Vec::new();
                self.get_type_of_dotted_name(node.expression(), Some(&mut related_info));
                let diagnostic = new_diagnostic_for_node(
                    node.expression(),
                    diag::Assertions_require_every_name_in_the_call_target_to_be_declared_with_an_explicit_type_annotation,
                    args![],
                );
                if let Some(diagnostic) = self.add_diagnostic(diagnostic) {
                    for related in related_info {
                        diagnostic.add_related_info(Some(related));
                    }
                }
            }
        }
        return_type
    }

    // Go: checker/checker.go:8538 checkDeprecatedSignature
    pub fn check_deprecated_signature(&mut self, sig: SignatureId, node: Node) {
        if self
            .sig(sig)
            .flags
            .intersects(SignatureFlags::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE)
        {
            return;
        }
        let declaration = self.sig(sig).declaration;
        if declaration.is_some() && self.is_deprecated_declaration(declaration) {
            let suggestion_node = self.get_deprecated_suggestion_node(node);
            let name =
                try_get_property_access_or_identifier_to_string(get_invoked_expression(node));
            let signature_string = self.signature_to_string(sig);
            self.add_deprecated_suggestion_with_signature(
                suggestion_node,
                declaration,
                &name,
                &signature_string,
            );
        }
    }

    // Go: checker/checker.go:8549 addDeprecatedSuggestionWithSignature
    pub fn add_deprecated_suggestion_with_signature(
        &mut self,
        location: Node,
        declaration: Node,
        deprecated_entity: &str,
        signature_string: &str,
    ) -> Diagnostic {
        let message = if !deprecated_entity.is_empty() {
            diag::The_signature_0_of_1_is_deprecated
        } else {
            diag::X_0_is_deprecated
        };
        let diagnostic = new_diagnostic_for_node(
            location,
            message,
            args![signature_string, deprecated_entity],
        );
        self.add_deprecated_suggestion_worker(&[declaration], diagnostic)
    }

    // Go: checker/checker.go:8555 isSymbolOrSymbolForCall
    pub fn is_symbol_or_symbol_for_call(&mut self, node: Node) -> bool {
        if !is_call_expression(node) {
            return false;
        }
        let mut left = node.expression();
        if is_property_access_expression(left) && left.name().text() == "for" {
            left = left.expression();
        }
        if !is_identifier(left) || left.text() != "Symbol" {
            return false;
        }
        // make sure `Symbol` is the global symbol
        let global_es_symbol = self.get_global_es_symbol_constructor_symbol_or_nil();
        if global_es_symbol.is_nil() {
            return false;
        }
        global_es_symbol
            == self.resolve_name(
                left,
                "Symbol",
                SymbolFlags::VALUE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false,
            )
    }

    /**
     * Resolve a signature of a given call-like expression.
     * @param node a call-like expression to try resolve a signature for
     * @param candidatesOutArray an array of signature to be filled in by the function. It is passed by signature help in the language service;
     *    the function will fill it up with appropriate candidate signatures
     * @return a signature of the call-like expression or undefined if one can't be found
     */
    // Go: checker/checker.go:8581 getResolvedSignature
    pub fn get_resolved_signature(
        &mut self,
        node: Node,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> SignatureId {
        // If getResolvedSignature has already been called, we will have cached the resolvedSignature.
        // However, it is possible that either candidatesOutArray was not passed in the first time,
        // or that a different candidatesOutArray was passed in. Therefore, we need to redo the work
        // to correctly fill the candidatesOutArray.
        let cached = self.signature_links.get(node).resolved_signature;
        if cached.is_some() && cached != self.resolving_signature && candidates_out_array.is_none()
        {
            return cached;
        }
        let save_resolution_start = self.resolution_start;
        if cached.is_nil() {
            // If we haven't already done so, temporarily reset the resolution stack. This allows us to
            // handle "inverted" situations where, for example, an API client asks for the type of a symbol
            // containined in a function call argument whose contextual type depends on the symbol itself
            // through resolution of the containing function call. By resetting the resolution stack we'll
            // retry the symbol type resolution with the resolvingSignature marker in place to suppress
            // the contextual type circularity.
            self.resolution_start = self.type_resolutions.len() as i32;
        }
        let resolving_signature = self.resolving_signature;
        self.signature_links.get(node).resolved_signature = resolving_signature;
        let mut result = self.resolve_signature(node, candidates_out_array, check_mode);
        self.resolution_start = save_resolution_start;
        // When CheckMode.SkipGenericFunctions is set we use resolvingSignature to indicate that call
        // resolution should be deferred.
        if result != self.resolving_signature {
            // if the signature resolution originated on a node that itself depends on the contextual type
            // then it's possible that the resolved signature might not be the same as the one that would be computed in source order
            // since resolving such signature leads to resolving the potential outer signature, its arguments and thus the very same signature
            // it's possible that this inner resolution sets the resolvedSignature first.
            // In such a case we ignore the local result and reuse the correct one that was cached.
            let links_resolved = self.signature_links.get(node).resolved_signature;
            if links_resolved != self.resolving_signature {
                result = links_resolved;
            }
            // If signature resolution originated in control flow type analysis (for example to compute the
            // assigned type in a flow assignment) we don't cache the result as it may be based on temporary
            // types from the control flow analysis.
            if self.flow_loop_stack.is_empty() {
                self.signature_links.get(node).resolved_signature = result;
            } else {
                self.signature_links.get(node).resolved_signature = cached;
            }
        }
        result
    }

    // Go: checker/checker.go:8627 resolveSignature
    pub fn resolve_signature(
        &mut self,
        node: Node,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> SignatureId {
        match node.kind() {
            SyntaxKind::CallExpression => {
                return self.resolve_call_expression(node, candidates_out_array, check_mode);
            }
            SyntaxKind::NewExpression => {
                return self.resolve_new_expression(node, candidates_out_array, check_mode);
            }
            SyntaxKind::TaggedTemplateExpression => {
                return self.resolve_tagged_template_expression(
                    node,
                    candidates_out_array,
                    check_mode,
                );
            }
            SyntaxKind::Decorator => {
                return self.resolve_decorator(node, candidates_out_array, check_mode);
            }
            SyntaxKind::JsxOpeningFragment
            | SyntaxKind::JsxOpeningElement
            | SyntaxKind::JsxSelfClosingElement => {
                return self.resolve_jsx_opening_like_element(
                    node,
                    candidates_out_array,
                    check_mode,
                );
            }
            SyntaxKind::BinaryExpression => {
                return self.resolve_instanceof_expression(node, candidates_out_array, check_mode);
            }
            _ => {}
        }
        panic!("Unhandled case in resolveSignature")
    }
}
