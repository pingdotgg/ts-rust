//! Port of Go `checker/checker.go` lines 9355-10268.

use crate::diagnostics::Message;
use crate::prelude::*;
use smallvec::SmallVec;

// PORT: Go `core.FirstOrNil` / `core.ElementOrNil` over a `NodeSlice`.
fn node_slice_element_or_nil(nodes: NodeSlice, index: usize) -> Node {
    if index < nodes.len() {
        nodes.get(index)
    } else {
        Node::NIL
    }
}

impl Checker {
    // Go: checker/checker.go:9573 getEffectiveCheckNode
    pub fn get_effective_check_node(&mut self, argument: Node) -> Node {
        let flags = if is_in_js_file(argument) {
            OuterExpressionKinds::OEK_PARENTHESES
                | OuterExpressionKinds::OEK_SATISFIES
                | OuterExpressionKinds::OEK_EXCLUDE_JS_DOC_TYPE_ASSERTION
        } else {
            OuterExpressionKinds::OEK_PARENTHESES | OuterExpressionKinds::OEK_SATISFIES
        };
        skip_outer_expressions(argument, flags)
    }

    // Go: checker/checker.go:9582 inferTypeArguments
    pub fn infer_type_arguments(
        &mut self,
        node: Node,
        signature: SignatureId,
        args: &[Node],
        check_mode: CheckMode,
        context: InferenceContextId,
    ) -> SmallVec<[TypeId; 4]> {
        if is_jsx_opening_like_element(node) {
            return SmallVec::from_vec(
                self.infer_jsx_type_arguments(node, signature, check_mode, context),
            );
        }
        // If a contextual type is available, infer from that type to the return type of the call expression. For
        // example, given a 'function wrap<T, U>(cb: (x: T) => U): (x: T) => U' and a call expression
        // 'let f: (x: string) => number = wrap(s => s.length)', we infer from the declared type of 'f' to the
        // return type of 'wrap'.
        if !is_decorator(node) && !is_binary_expression(node) {
            // PERF: the type parameters are read from the signature in place,
            // not copied.
            let mut skip_binding_patterns = true;
            for i in 0..self.sig(signature).type_parameters.len() {
                let p = self.sig(signature).type_parameters[i];
                if self.get_default_from_type_parameter(p).is_nil() {
                    skip_binding_patterns = false;
                    break;
                }
            }
            let contextual_type = self.get_contextual_type(
                node,
                if skip_binding_patterns {
                    ContextFlags::SKIP_BINDING_PATTERNS
                } else {
                    ContextFlags::NONE
                },
            );
            if contextual_type.is_some() {
                let inference_target_type = self.get_return_type_of_signature(signature);
                if self.could_contain_type_variables(inference_target_type) {
                    let outer_context = self.get_inference_context(node);
                    let is_from_binding_pattern = !skip_binding_patterns
                        && self.get_contextual_type(node, ContextFlags::SKIP_BINDING_PATTERNS)
                            != contextual_type;
                    // A return type inference from a binding pattern can be used in instantiating the contextual
                    // type of an argument later in inference, but cannot stand on its own as the final return type.
                    // It is incorporated into `context.returnMapper` which is used in `instantiateContextualType`,
                    // but doesn't need to go into `context.inferences`. This allows a an array binding pattern to
                    // produce a tuple for `T` in
                    //   declare function f<T>(cb: () => T): T;
                    //   const [e1, e2, e3] = f(() => [1, "hi", true]);
                    // but does not produce any inference for `T` in
                    //   declare function f<T>(): T;
                    //   const [e1, e2, e3] = f();
                    if !is_from_binding_pattern {
                        // We clone the inference context to avoid disturbing a resolution in progress for an
                        // outer call expression. Effectively we just want a snapshot of whatever has been
                        // inferred for any outer call expression so far.
                        let cloned =
                            self.clone_inference_context(outer_context, InferenceFlags::NO_DEFAULT);
                        let outer_mapper = self.get_mapper_from_context(cloned);
                        let instantiated_type =
                            self.instantiate_type(contextual_type, outer_mapper);
                        // If the contextual type is a generic function type with a single call signature, we
                        // instantiate the type with its own type parameters and type arguments. This ensures that
                        // the type parameters are not erased to type any during type inference such that they can
                        // be inferred as actual types from the contextual type. For example:
                        //   declare function arrayMap<T, U>(f: (x: T) => U): (a: T[]) => U[];
                        //   const boxElements: <A>(a: A[]) => { value: A }[] = arrayMap(value => ({ value }));
                        // Above, the type of the 'value' parameter is inferred to be 'A'.
                        let contextual_signature =
                            self.get_single_call_signature(instantiated_type);
                        let inference_source_type = if contextual_signature.is_some()
                            && !self.sig(contextual_signature).type_parameters.is_empty()
                        {
                            let type_parameters =
                                self.sig(contextual_signature).type_parameters.clone();
                            let instantiated = self
                                .get_signature_instantiation_without_filling_in_type_arguments(
                                    contextual_signature,
                                    &type_parameters,
                                );
                            self.get_or_create_type_from_signature(instantiated)
                        } else {
                            instantiated_type
                        };
                        // Inferences made from return types have lower priority than all other inferences.
                        self.infer_types(
                            context,
                            inference_source_type,
                            inference_target_type,
                            InferencePriority::RETURN_TYPE,
                            false,
                        );
                    }
                    // Create a type mapper for instantiating generic contextual types using the inferences made
                    // from the return type. We need a separate inference pass here because (a) instantiation of
                    // the source type uses the outer context's return mapper (which excludes inferences made from
                    // outer arguments), and (b) we don't want any further inferences going into this context.
                    // We use `createOuterReturnMapper` to ensure that all occurrences of outer type parameters are
                    // replaced with inferences produced from the outer return type or preceding outer arguments.
                    // This protects against circular inferences, i.e. avoiding situations where inferences reference
                    // type parameters for which the inferences are being made.
                    let context_flags = self.inference_context(context).flags;
                    let return_context =
                        self.new_inference_context_of_signature(signature, context_flags);
                    let outer_return_mapper = if outer_context.is_some() {
                        self.create_outer_return_mapper(outer_context)
                    } else {
                        MapperId::NIL
                    };
                    let return_source_type =
                        self.instantiate_type(contextual_type, outer_return_mapper);
                    self.infer_types(
                        return_context,
                        return_source_type,
                        inference_target_type,
                        InferencePriority::NONE,
                        false,
                    );
                    let count = self.inference_context(return_context).inferences.len();
                    let mut any_candidates = false;
                    for i in 0..count {
                        if self.has_inference_candidates(return_context, i) {
                            any_candidates = true;
                            break;
                        }
                    }
                    if any_candidates {
                        let cloned = self.clone_inferred_part_of_context(return_context);
                        let return_mapper = self.get_mapper_from_context(cloned);
                        self.inference_context_mut(context).rare_mut().return_mapper =
                            return_mapper;
                    } else if let Some(rare) = &mut self.inference_context_mut(context).rare {
                        rare.return_mapper = MapperId::NIL;
                    }
                }
            }
        }
        let rest_type = self.get_non_array_rest_type(signature);
        let mut arg_count = args.len() as i32;
        if rest_type.is_some() {
            arg_count = (self.get_parameter_count(signature) - 1).min(arg_count);
        }
        if rest_type.is_some()
            && self
                .ty(rest_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            let info = self
                .inference_context(context)
                .inferences
                .iter()
                .position(|info| info.type_parameter == rest_type);
            if let Some(info) = info {
                if !args[arg_count as usize..]
                    .iter()
                    .any(|&arg| is_spread_argument(arg))
                {
                    self.inference_context_mut(context).inferences[info].implied_arity =
                        args.len() as i32 - arg_count;
                }
            }
        }
        let this_type = self.get_this_type_of_signature(signature);
        if this_type.is_some() && self.could_contain_type_variables(this_type) {
            let this_argument_node = self.get_this_argument_of_call(node);
            let this_argument_type = self.get_this_argument_type(this_argument_node);
            self.infer_types(
                context,
                this_argument_type,
                this_type,
                InferencePriority::NONE,
                false,
            );
        }
        for i in 0..arg_count {
            let arg = args[i as usize];
            if arg.kind() != SyntaxKind::OmittedExpression {
                let param_type = self.get_type_at_position(signature, i);
                if self.could_contain_type_variables(param_type) {
                    let arg_type = self.check_expression_with_contextual_type(
                        arg, param_type, context, check_mode,
                    );
                    self.infer_types(
                        context,
                        arg_type,
                        param_type,
                        InferencePriority::NONE,
                        false,
                    );
                }
            }
        }
        if rest_type.is_some() && self.could_contain_type_variables(rest_type) {
            let spread_type = self.get_spread_argument_type(
                args,
                arg_count,
                args.len() as i32,
                rest_type,
                context,
                check_mode,
            );
            self.infer_types(
                context,
                spread_type,
                rest_type,
                InferencePriority::NONE,
                false,
            );
        }
        self.get_inferred_type_list(context)
    }

    // Go: checker/checker.go:9690 getCandidateForOverloadFailure
    // No signature was applicable. We have already reported the errors for the invalid signature.
    // PORT: Go mutates the caller's `candidates` slice (see pickLongestCandidateSignature), so it
    // is `&mut [SignatureId]`.
    pub fn get_candidate_for_overload_failure(
        &mut self,
        node: Node,
        candidates: &mut [SignatureId],
        args: &[Node],
        has_candidates_out_array: bool,
        check_mode: CheckMode,
    ) -> SignatureId {
        // Else should not have called this.
        self.check_node_deferred(node);
        // Normally we will combine overloads. Skip this if they have type parameters since that's hard to combine.
        // Don't do this if there is a `candidatesOutArray`,
        // because then we want the chosen best candidate to be one of the overloads, not a combination.
        if has_candidates_out_array
            || candidates.len() == 1
            || candidates
                .iter()
                .any(|&s| !self.sig(s).type_parameters.is_empty())
        {
            return self.pick_longest_candidate_signature(node, candidates, args, check_mode);
        }
        self.create_union_of_signatures_for_overload_failure(candidates)
    }

    // Go: checker/checker.go:9702 pickLongestCandidateSignature
    pub fn pick_longest_candidate_signature(
        &mut self,
        node: Node,
        candidates: &mut [SignatureId],
        args: &[Node],
        check_mode: CheckMode,
    ) -> SignatureId {
        // Pick the longest signature. This way we can get a contextual type for cases like:
        //     declare function f(a: { xa: number; xb: number; }, b: number);
        //     f({ |
        // Also, use explicitly-supplied type arguments if they are provided, so we can get a contextual signature in cases like:
        //     declare function f<T>(k: keyof T);
        //     f<Foo>("
        let mut arg_count = args.len() as i32;
        if let Some(apparent_argument_count) = self.apparent_argument_count {
            arg_count = apparent_argument_count;
        }
        let best_index = self.get_longest_candidate_index(candidates, arg_count);
        let candidate = candidates[best_index as usize];
        let type_parameters = self.sig(candidate).type_parameters.clone();
        if type_parameters.is_empty() {
            return candidate;
        }
        let type_argument_nodes: Vec<Node> =
            if self.call_like_expression_may_have_type_arguments(node) {
                node.type_arguments().to_vec()
            } else {
                Vec::new()
            };
        let instantiated = if !type_argument_nodes.is_empty() {
            let type_arguments =
                self.get_type_arguments_from_nodes(&type_argument_nodes, &type_parameters);
            self.create_signature_instantiation(candidate, &type_arguments)
        } else {
            self.infer_signature_instantiation_for_overload_failure(
                node,
                &type_parameters,
                candidate,
                args,
                check_mode,
            )
        };
        candidates[best_index as usize] = instantiated;
        instantiated
    }

    // Go: checker/checker.go:9733 getLongestCandidateIndex
    pub fn get_longest_candidate_index(
        &mut self,
        candidates: &[SignatureId],
        args_count: i32,
    ) -> i32 {
        let mut max_params_index: i32 = -1;
        let mut max_params: i32 = -1;
        for (i, &candidate) in candidates.iter().enumerate() {
            let param_count = self.get_parameter_count(candidate);
            if self.has_effective_rest_parameter(candidate) || param_count >= args_count {
                return i as i32;
            }
            if param_count > max_params {
                max_params = param_count;
                max_params_index = i as i32;
            }
        }
        max_params_index
    }

    // Go: checker/checker.go:9749 getTypeArgumentsFromNodes
    pub fn get_type_arguments_from_nodes(
        &mut self,
        type_argument_nodes: &[Node],
        type_parameters: &[TypeId],
    ) -> Vec<TypeId> {
        let mut type_argument_nodes = type_argument_nodes;
        if type_argument_nodes.len() > type_parameters.len() {
            type_argument_nodes = &type_argument_nodes[..type_parameters.len()];
        }
        let mut type_arguments: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for &n in type_argument_nodes {
            type_arguments.push(self.get_type_from_type_node(n));
        }
        while type_arguments.len() < type_parameters.len() {
            let mut t = self.get_default_from_type_parameter(type_parameters[type_arguments.len()]);
            if t.is_nil() {
                t = self.get_constraint_of_type_parameter(type_parameters[type_arguments.len()]);
                if t.is_nil() {
                    t = self.unknown_type;
                }
            }
            type_arguments.push(t);
        }
        type_arguments
    }

    // Go: checker/checker.go:9767 inferSignatureInstantiationForOverloadFailure
    pub fn infer_signature_instantiation_for_overload_failure(
        &mut self,
        node: Node,
        type_parameters: &[TypeId],
        candidate: SignatureId,
        args: &[Node],
        check_mode: CheckMode,
    ) -> SignatureId {
        let inference_context = self.new_inference_context(
            type_parameters,
            candidate,
            if is_in_js_file(node) {
                InferenceFlags::ANY_DEFAULT
            } else {
                InferenceFlags::NONE
            },
            None,
        );
        let type_argument_types = self.infer_type_arguments(
            node,
            candidate,
            args,
            check_mode | CheckMode::SKIP_CONTEXT_SENSITIVE | CheckMode::SKIP_GENERIC_FUNCTIONS,
            inference_context,
        );
        self.create_signature_instantiation(candidate, &type_argument_types)
    }

    // Go: checker/checker.go:9773 createUnionOfSignaturesForOverloadFailure
    pub fn create_union_of_signatures_for_overload_failure(
        &mut self,
        candidates: &[SignatureId],
    ) -> SignatureId {
        let this_parameters: Vec<SymbolId> = candidates
            .iter()
            .map(|&c| self.sig(c).this_parameter)
            .filter(|s| s.is_some())
            .collect();
        let mut this_parameter = SymbolId::NIL;
        if !this_parameters.is_empty() {
            let mut types = Vec::with_capacity(this_parameters.len());
            for &p in &this_parameters {
                types.push(self.get_type_of_parameter(p));
            }
            this_parameter = self.create_combined_symbol_from_types(&this_parameters, &types);
        }
        // PORT: Go `minAndMax(candidates, getNonRestParameterCount)`, inlined because the
        // value function needs the checker.
        let mut min_argument_count: i32 = 0;
        let mut max_non_rest_param: i32 = 0;
        for (i, &s) in candidates.iter().enumerate() {
            let value = self.get_non_rest_parameter_count(s);
            if i == 0 {
                min_argument_count = value;
                max_non_rest_param = value;
            } else {
                min_argument_count = min_argument_count.min(value);
                max_non_rest_param = max_non_rest_param.max(value);
            }
        }
        let mut parameters: Vec<SymbolId> = Vec::with_capacity(max_non_rest_param.max(0) as usize);
        for i in 0..max_non_rest_param {
            let iu = i as usize;
            let mut symbols: Vec<SymbolId> = Vec::new();
            for &s in candidates {
                let has_rest = self.signature_has_rest_parameter(s);
                let sig_parameters = &self.sig(s).parameters;
                let symbol = if has_rest {
                    if iu + 1 < sig_parameters.len() {
                        sig_parameters[iu]
                    } else {
                        sig_parameters.last().copied().unwrap_or(SymbolId::NIL)
                    }
                } else if iu < sig_parameters.len() {
                    sig_parameters[iu]
                } else {
                    SymbolId::NIL
                };
                if symbol.is_some() {
                    symbols.push(symbol);
                }
            }
            let mut types: Vec<TypeId> = Vec::new();
            for &s in candidates {
                let t = self.try_get_type_at_position(s, i);
                if t.is_some() {
                    types.push(t);
                }
            }
            let combined = self.create_combined_symbol_from_types(&symbols, &types);
            parameters.push(combined);
        }
        let mut rest_parameter_symbols: Vec<SymbolId> = Vec::new();
        for &s in candidates {
            if self.signature_has_rest_parameter(s) {
                let last = self
                    .sig(s)
                    .parameters
                    .last()
                    .copied()
                    .unwrap_or(SymbolId::NIL);
                if last.is_some() {
                    rest_parameter_symbols.push(last);
                }
            }
        }
        let mut flags = SignatureFlags::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE;
        if !rest_parameter_symbols.is_empty() {
            let mut rest_types: Vec<TypeId> = Vec::new();
            for &s in candidates {
                let t = self.try_get_rest_type_of_signature(s);
                if t.is_some() {
                    rest_types.push(t);
                }
            }
            let union =
                self.get_union_type_ex(&rest_types, UnionReduction::SUBTYPE, None, TypeId::NIL);
            let t = self.create_array_type(union);
            let combined =
                self.create_combined_symbol_for_overload_failure(&rest_parameter_symbols, t);
            parameters.push(combined);
            flags |= SignatureFlags::HAS_REST_PARAMETER;
        }
        for &s in candidates {
            if self.signature_has_literal_types(s) {
                flags |= SignatureFlags::HAS_LITERAL_TYPES;
                break;
            }
        }
        let declaration = self.sig(candidates[0]).declaration;
        let mut return_types = Vec::with_capacity(candidates.len());
        for &s in candidates {
            return_types.push(self.get_return_type_of_signature(s));
        }
        let return_type = self.get_intersection_type(&return_types);
        self.new_signature(
            flags,
            declaration,
            &[],
            this_parameter,
            &parameters,
            return_type,
            TypePredicateId::NIL,
            min_argument_count,
        )
    }

    // Go: checker/checker.go:9814 createCombinedSymbolFromTypes
    pub fn create_combined_symbol_from_types(
        &mut self,
        sources: &[SymbolId],
        types: &[TypeId],
    ) -> SymbolId {
        let union = self.get_union_type_ex(types, UnionReduction::SUBTYPE, None, TypeId::NIL);
        self.create_combined_symbol_for_overload_failure(sources, union)
    }

    // Go: checker/checker.go:9818 createCombinedSymbolForOverloadFailure
    pub fn create_combined_symbol_for_overload_failure(
        &mut self,
        sources: &[SymbolId],
        t: TypeId,
    ) -> SymbolId {
        // This function is currently only used for erroneous overloads, so it's good enough to just use the first source.
        let first = sources.first().copied().unwrap_or(SymbolId::NIL);
        self.create_symbol_with_type(first, t)
    }

    // Go: checker/checker.go:9823 getRestTypeOfSignature
    pub fn get_rest_type_of_signature(&mut self, signature: SignatureId) -> TypeId {
        let t = self.try_get_rest_type_of_signature(signature);
        if t.is_some() { t } else { self.any_type }
    }

    // Go: checker/checker.go:9827 tryGetRestTypeOfSignature
    pub fn try_get_rest_type_of_signature(&mut self, signature: SignatureId) -> TypeId {
        if !self.signature_has_rest_parameter(signature) {
            return TypeId::NIL;
        }
        let last = *self
            .sig(signature)
            .parameters
            .last()
            .expect("rest signature has parameters");
        let mut rest_type = self.get_type_of_symbol(last);
        if self.is_tuple_type(rest_type) {
            rest_type = self.get_rest_type_of_tuple_type(rest_type);
            if rest_type.is_nil() {
                return TypeId::NIL;
            }
        }
        let number_type = self.number_type;
        self.get_index_type_of_type(rest_type, number_type)
    }

    // Go: checker/checker.go:9841 reportCallResolutionErrors
    pub fn report_call_resolution_errors(
        &mut self,
        node: Node,
        s: &CallState,
        signatures: &[SignatureId],
        head_message: Option<&'static Message>,
    ) {
        if !s.candidates_for_argument_error.is_empty() {
            let last = *s.candidates_for_argument_error.last().unwrap();
            let mut diags: Vec<Diagnostic> = Vec::new();
            let assignable_relation = self.assignable_relation.clone();
            self.is_signature_applicable(
                s.node,
                &s.args,
                last,
                &assignable_relation,
                CheckMode::NORMAL,
                true, /*reportErrors*/
                Some(&mut diags),
            );
            for diagnostic in diags {
                let mut diagnostic = diagnostic;
                if s.candidates_for_argument_error.len() > 1 {
                    diagnostic = new_diagnostic_chain(
                        Some(diagnostic),
                        diag::The_last_overload_gave_the_following_error,
                        args![],
                    );
                    diagnostic = new_diagnostic_chain(
                        Some(diagnostic),
                        diag::No_overload_matches_this_call,
                        args![],
                    );
                }
                if let Some(head_message) = head_message {
                    diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
                }
                let last_declaration = self.sig(last).declaration;
                if last_declaration.is_some() && s.candidates_for_argument_error.len() > 1 {
                    diagnostic.add_related_info(Some(new_diagnostic_for_node(
                        last_declaration,
                        diag::The_last_overload_is_declared_here,
                        args![],
                    )));
                }
                self.add_implementation_success_elaboration(s, last, &mut diagnostic);
                self.add_diagnostic(diagnostic);
            }
        } else if s.candidate_for_argument_arity_error.is_some() {
            let d = self.get_argument_arity_error(
                s.node,
                &[s.candidate_for_argument_arity_error],
                &s.args,
                head_message,
            );
            self.add_diagnostic(d);
        } else if s.candidate_for_type_argument_error.is_some() {
            let type_arguments = s.node.type_arguments().to_vec();
            self.check_type_arguments(
                s.candidate_for_type_argument_error,
                &type_arguments,
                true, /*reportErrors*/
                head_message,
            );
        } else if !is_jsx_opening_fragment(node) {
            let mut signatures_with_correct_type_argument_arity: Vec<SignatureId> = Vec::new();
            for &sig in signatures {
                if self.has_correct_type_argument_arity(sig, &s.type_arguments) {
                    signatures_with_correct_type_argument_arity.push(sig);
                }
            }
            if signatures_with_correct_type_argument_arity.is_empty() {
                let d = self.get_type_argument_arity_error(
                    s.node,
                    signatures,
                    &s.type_arguments,
                    head_message,
                );
                self.add_diagnostic(d);
            } else {
                let d = self.get_argument_arity_error(
                    s.node,
                    &signatures_with_correct_type_argument_arity,
                    &s.args,
                    head_message,
                );
                self.add_diagnostic(d);
            }
        }
    }

    // Go: checker/checker.go:9877 addImplementationSuccessElaboration
    pub fn add_implementation_success_elaboration(
        &mut self,
        s: &CallState,
        failed: SignatureId,
        diagnostic: &mut Diagnostic,
    ) {
        let failed_declaration = self.sig(failed).declaration;
        if failed_declaration.is_some() && failed_declaration.symbol().is_some() {
            let declarations = self.sym(failed_declaration.symbol()).declarations.clone();
            if declarations.len() > 1 {
                let implementation = declarations
                    .iter()
                    .copied()
                    .find(|&d| is_function_like_declaration(d) && node_is_present(d.body()))
                    .unwrap_or(Node::NIL);
                if implementation.is_some() {
                    let candidate = self.get_signature_from_declaration(implementation);
                    let mut local_state = s.clone();
                    local_state.candidates = smallvec::smallvec![candidate];
                    local_state.is_single_non_generic_candidate =
                        self.sig(candidate).type_parameters.is_empty();
                    let assignable_relation = self.assignable_relation.clone();
                    if self
                        .choose_overload(&mut local_state, &assignable_relation)
                        .is_some()
                    {
                        diagnostic.add_related_info(Some(new_diagnostic_for_node(
                            implementation,
                            diag::The_call_would_have_succeeded_against_this_implementation_but_implementation_signatures_of_overloads_are_not_externally_visible,
                            args![],
                        )));
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:9897 getArgumentArityError
    // PORT: Go `int` sentinels (math.MaxInt/MinInt) are kept as i64 locals.
    pub fn get_argument_arity_error(
        &mut self,
        node: Node,
        signatures: &[SignatureId],
        args: &[Node],
        head_message: Option<&'static Message>,
    ) -> Diagnostic {
        let spread_index = self.get_spread_argument_index(args);
        if spread_index > -1 {
            return new_diagnostic_for_node(
                args[spread_index as usize],
                diag::A_spread_argument_must_either_have_a_tuple_type_or_be_passed_to_a_rest_parameter,
                args![],
            );
        }
        let args_len = args.len() as i64;
        let mut min_count = i64::MAX; // smallest parameter count
        let mut max_count = i64::MIN; // largest parameter count
        let mut max_below = i64::MIN; // largest parameter count that is smaller than the number of arguments
        let mut min_above = i64::MAX; // smallest parameter count that is larger than the number of arguments
        let mut closest_signature = SignatureId::NIL;
        for &sig in signatures {
            let min_parameter = self.get_min_argument_count(sig) as i64;
            let max_parameter = self.get_parameter_count(sig) as i64;
            // smallest/largest parameter counts
            if min_parameter < min_count {
                min_count = min_parameter;
                closest_signature = sig;
            }
            max_count = max_count.max(max_parameter);
            // shortest parameter count *longer than the call*/longest parameter count *shorter than the call*
            if min_parameter < args_len && min_parameter > max_below {
                max_below = min_parameter;
            }
            if args_len < max_parameter && max_parameter < min_above {
                min_above = max_parameter;
            }
        }
        let mut has_rest_parameter = false;
        for &sig in signatures {
            if self.has_effective_rest_parameter(sig) {
                has_rest_parameter = true;
                break;
            }
        }
        let parameter_range = if has_rest_parameter {
            min_count.to_string()
        } else if min_count < max_count {
            min_count.to_string() + "-" + &max_count.to_string()
        } else {
            min_count.to_string()
        };
        let is_void_promise_error = !has_rest_parameter
            && parameter_range == "1"
            && args.is_empty()
            && self.is_promise_resolve_arity_error(node);
        let error_node = get_error_node_for_call_node(node);
        if is_void_promise_error && is_in_js_file(node) {
            return new_diagnostic_for_node(
                error_node,
                diag::Expected_1_argument_but_got_0_new_Promise_needs_a_JSDoc_hint_to_produce_a_resolve_that_can_be_called_without_arguments,
                args![],
            );
        }
        let message: &'static Message = if is_decorator(node) {
            if has_rest_parameter {
                diag::The_runtime_will_invoke_the_decorator_with_1_arguments_but_the_decorator_expects_at_least_0
            } else {
                diag::The_runtime_will_invoke_the_decorator_with_1_arguments_but_the_decorator_expects_0
            }
        } else if has_rest_parameter {
            diag::Expected_at_least_0_arguments_but_got_1
        } else if is_void_promise_error {
            diag::Expected_0_arguments_but_got_1_Did_you_forget_to_include_void_in_your_type_argument_to_Promise
        } else {
            diag::Expected_0_arguments_but_got_1
        };
        if min_count < args_len && args_len < max_count {
            // between min and max, but with no matching overload
            let mut diagnostic = new_diagnostic_for_node(
                error_node,
                diag::No_overload_expects_0_arguments_but_overloads_do_exist_that_expect_either_1_or_2_arguments,
                args![args_len, max_below, min_above],
            );
            if let Some(head_message) = head_message {
                diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
            }
            diagnostic
        } else if args_len < min_count {
            // too short: put the error span on the call expression, not any of the args
            let mut diagnostic =
                new_diagnostic_for_node(error_node, message, args![parameter_range, args_len]);
            if let Some(head_message) = head_message {
                diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
            }
            let mut parameter = Node::NIL;
            if closest_signature.is_some() && self.sig(closest_signature).declaration.is_some() {
                let closest_declaration = self.sig(closest_signature).declaration;
                let this_offset = if self.sig(closest_signature).this_parameter.is_some() {
                    1
                } else {
                    0
                };
                parameter = node_slice_element_or_nil(
                    closest_declaration.parameters(),
                    args.len() + this_offset,
                );
            }
            if parameter.is_some() {
                let related = if is_binding_pattern(parameter.name()) {
                    new_diagnostic_for_node(
                        parameter,
                        diag::An_argument_matching_this_binding_pattern_was_not_provided,
                        args![],
                    )
                } else if is_rest_parameter(parameter) {
                    new_diagnostic_for_node(
                        parameter,
                        diag::Arguments_for_the_rest_parameter_0_were_not_provided,
                        args![parameter.name().text()],
                    )
                } else {
                    new_diagnostic_for_node(
                        parameter,
                        diag::An_argument_for_0_was_not_provided,
                        args![parameter.name().text()],
                    )
                };
                diagnostic.add_related_info(Some(related));
            }
            diagnostic
        } else {
            // Guard against out-of-bounds access when maxCount >= len(args).
            // This can happen when we reach this fallback error path but the argument
            // count actually matches the parameter count (e.g., due to trailing commas
            // causing signature resolution to fail for other reasons).
            if max_count >= args_len {
                let mut diagnostic =
                    new_diagnostic_for_node(error_node, message, args![parameter_range, args_len]);
                if let Some(head_message) = head_message {
                    diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
                }
                return diagnostic;
            }
            let source_file = get_source_file_of_node(node);
            let mut pos = args[max_count as usize].pos();
            let mut end = args[args.len() - 1].end();
            if end == pos {
                end += 1;
            }
            pos = skip_trivia(&source_file_text(source_file), pos);
            if end < pos {
                end = pos;
            }
            let mut diagnostic = new_diagnostic(
                source_file,
                TextRange::new(pos, end),
                message,
                args![parameter_range, args_len],
            );
            if let Some(head_message) = head_message {
                diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
            }
            diagnostic
        }
    }

    // Go: checker/checker.go:10015 isPromiseResolveArityError
    pub fn is_promise_resolve_arity_error(&mut self, node: Node) -> bool {
        if !is_call_expression(node) || !is_identifier(node.expression()) {
            return false;
        }
        let symbol = self.resolve_name(
            node.expression(),
            node.expression().text(),
            SymbolFlags::VALUE,
            None,  /*nameNotFoundMessage*/
            false, /*isUse*/
            false,
        );
        if symbol.is_nil() {
            return false;
        }
        let decl = self.sym(symbol).value_declaration;
        if decl.is_nil()
            || !is_parameter_declaration(decl)
            || !is_function_expression_or_arrow_function(decl.parent())
            || !is_new_expression(decl.parent().parent())
            || !is_identifier(decl.parent().parent().expression())
        {
            return false;
        }
        let global_promise_symbol = self.get_global_promise_constructor_symbol_or_nil();
        if global_promise_symbol.is_nil() {
            return false;
        }
        let constructor_symbol = self.get_resolved_symbol(decl.parent().parent().expression());
        constructor_symbol == global_promise_symbol
    }
}

// Go: checker/checker.go:10035 getErrorNodeForCallNode
pub fn get_error_node_for_call_node(node: Node) -> Node {
    let mut node = node;
    if is_call_expression(node) {
        node = node.expression();
        if is_property_access_expression(node) {
            node = node.name();
        }
    }
    node
}

impl Checker {
    // Go: checker/checker.go:10045 getTypeArgumentArityError
    // PORT: Go `int` sentinels (math.MaxInt/MinInt) are kept as i64 locals.
    pub fn get_type_argument_arity_error(
        &mut self,
        node: Node,
        signatures: &[SignatureId],
        type_arguments: &[Node],
        head_message: Option<&'static Message>,
    ) -> Diagnostic {
        let mut diagnostic: Diagnostic;
        let arg_count = type_arguments.len() as i64;
        let source_file = get_source_file_of_node(node);
        let type_argument_list = node.type_argument_list();
        let loc = TextRange::new(
            skip_trivia(&source_file_text(source_file), type_argument_list.pos()),
            type_argument_list.end(),
        );
        if signatures.len() == 1 {
            // No overloads exist
            let sig = signatures[0];
            let type_parameters = self.sig(sig).type_parameters.clone();
            let min_count = self.get_min_type_argument_count(&type_parameters) as i64;
            let max_count = type_parameters.len() as i64;
            let mut expected = min_count.to_string();
            if min_count < max_count {
                expected = expected + "-" + &max_count.to_string();
            }
            diagnostic = new_diagnostic(
                source_file,
                loc,
                diag::Expected_0_type_arguments_but_got_1,
                args![expected, arg_count],
            );
        } else {
            // Overloads exist
            let mut below_arg_count = i64::MIN;
            let mut above_arg_count = i64::MAX;
            for &sig in signatures {
                let type_parameters = self.sig(sig).type_parameters.clone();
                let min_count = self.get_min_type_argument_count(&type_parameters) as i64;
                let max_count = type_parameters.len() as i64;
                if min_count > arg_count {
                    above_arg_count = above_arg_count.min(min_count);
                } else if max_count < arg_count {
                    below_arg_count = below_arg_count.max(max_count);
                }
            }
            if below_arg_count != i64::MIN && above_arg_count != i64::MAX {
                diagnostic = new_diagnostic(
                    source_file,
                    loc,
                    diag::No_overload_expects_0_type_arguments_but_overloads_do_exist_that_expect_either_1_or_2_type_arguments,
                    args![arg_count, below_arg_count, above_arg_count],
                );
            } else {
                diagnostic = new_diagnostic(
                    source_file,
                    loc,
                    diag::Expected_0_type_arguments_but_got_1,
                    args![
                        if below_arg_count == i64::MIN {
                            above_arg_count
                        } else {
                            below_arg_count
                        },
                        arg_count
                    ],
                );
            }
        }
        if let Some(head_message) = head_message {
            diagnostic = new_diagnostic_chain(Some(diagnostic), head_message, args![]);
        }
        diagnostic
    }

    // Go: checker/checker.go:10086 reportCannotInvokePossiblyNullOrUndefinedError
    pub fn report_cannot_invoke_possibly_null_or_undefined_error(
        &mut self,
        node: Node,
        facts: TypeFacts,
    ) {
        let message = if facts.intersects(TypeFacts::IS_UNDEFINED) {
            if facts.intersects(TypeFacts::IS_NULL) {
                diag::Cannot_invoke_an_object_which_is_possibly_null_or_undefined
            } else {
                diag::Cannot_invoke_an_object_which_is_possibly_undefined
            }
        } else {
            diag::Cannot_invoke_an_object_which_is_possibly_null
        };
        self.error(node, message, args![]);
    }

    // Go: checker/checker.go:10094 resolveUntypedCall
    pub fn resolve_untyped_call(&mut self, node: Node) -> SignatureId {
        if self.call_like_expression_may_have_type_arguments(node) {
            // Check type arguments even though we will give an error that untyped calls may not accept type arguments.
            // This gets us diagnostics for the type arguments and marks them as referenced.
            self.check_source_elements(node.type_arguments());
        }
        match node.kind() {
            SyntaxKind::TaggedTemplateExpression => {
                self.check_expression(node.template());
            }
            SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => {
                self.check_expression(node.attributes());
            }
            SyntaxKind::BinaryExpression => {
                self.check_expression(node.left());
            }
            SyntaxKind::CallExpression | SyntaxKind::NewExpression => {
                for argument in node.arguments() {
                    self.check_expression(argument);
                }
            }
            _ => {}
        }
        self.any_signature
    }

    // Go: checker/checker.go:10115 resolveErrorCall
    pub fn resolve_error_call(&mut self, node: Node) -> SignatureId {
        self.resolve_untyped_call(node);
        self.unknown_signature
    }

    // Go: checker/checker.go:10125 isUntypedFunctionCall
    /**
     * TS 1.0 spec: 4.12
     * If FuncExpr is of type Any, or of an object type that has no call or construct signatures
     * but is a subtype of the Function interface, the call is an untyped function call.
     */
    pub fn is_untyped_function_call(
        &mut self,
        func_type: TypeId,
        apparent_func_type: TypeId,
        num_call_signatures: i32,
        num_construct_signatures: i32,
    ) -> bool {
        // We exclude union types because we may have a union of function types that happen to have no common signatures.
        if self.is_type_any(func_type) {
            return true;
        }
        if self.is_type_any(apparent_func_type)
            && self
                .ty(func_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            return true;
        }
        if num_call_signatures == 0
            && num_construct_signatures == 0
            && !self
                .ty(apparent_func_type)
                .flags
                .intersects(TypeFlags::UNION)
        {
            let reduced = self.get_reduced_type(apparent_func_type);
            if !self.ty(reduced).flags.intersects(TypeFlags::NEVER) {
                let global_function_type = self.global_function_type;
                return self.is_type_assignable_to(func_type, global_function_type);
            }
        }
        false
    }

    // Go: checker/checker.go:10132 invocationErrorDetails
    pub fn invocation_error_details(
        &mut self,
        error_target: Node,
        apparent_type: TypeId,
        kind: SignatureKind,
    ) -> Diagnostic {
        let mut diagnostic: Option<Diagnostic> = None;
        let is_call = kind == SignatureKind::CALL;
        let awaited_type = self.get_awaited_type(apparent_type);
        let maybe_missing_await =
            awaited_type.is_some() && !self.get_signatures_of_type(awaited_type, kind).is_empty();
        let mut target = error_target;
        if is_property_access_expression(error_target) && is_call_expression(error_target.parent())
        {
            target = error_target.name();
        }
        if self.ty(apparent_type).flags.intersects(TypeFlags::UNION) {
            let types = self.ty(apparent_type).types_list();
            let mut has_signatures = false;
            for constituent in types {
                let signatures = self.get_signatures_of_type(constituent, kind);
                if !signatures.is_empty() {
                    has_signatures = true;
                    if diagnostic.is_some() {
                        // Bail early if we already have an error, no chance of "No constituent of type is callable"
                        break;
                    }
                } else {
                    // Error on the first non callable constituent only
                    if diagnostic.is_none() {
                        let constituent_string = self.type_to_string(constituent);
                        let d = new_diagnostic_for_node(
                            target,
                            if is_call {
                                diag::Type_0_has_no_call_signatures
                            } else {
                                diag::Type_0_has_no_construct_signatures
                            },
                            args![constituent_string],
                        );
                        let apparent_string = self.type_to_string(apparent_type);
                        diagnostic = Some(new_diagnostic_chain_for_node(
                            Some(d),
                            target,
                            if is_call {
                                diag::Not_all_constituents_of_type_0_are_callable
                            } else {
                                diag::Not_all_constituents_of_type_0_are_constructable
                            },
                            args![apparent_string],
                        ));
                    }
                    if has_signatures {
                        // Bail early if we already found a signature, no chance of "No constituent of type is callable"
                        break;
                    }
                }
            }
            if !has_signatures {
                let apparent_string = self.type_to_string(apparent_type);
                diagnostic = Some(new_diagnostic_for_node(
                    target,
                    if is_call {
                        diag::No_constituent_of_type_0_is_callable
                    } else {
                        diag::No_constituent_of_type_0_is_constructable
                    },
                    args![apparent_string],
                ));
            }
            if diagnostic.is_none() {
                let apparent_string = self.type_to_string(apparent_type);
                diagnostic = Some(new_diagnostic_for_node(
                    target,
                    if is_call {
                        diag::Each_member_of_the_union_type_0_has_signatures_but_none_of_those_signatures_are_compatible_with_each_other
                    } else {
                        diag::Each_member_of_the_union_type_0_has_construct_signatures_but_none_of_those_signatures_are_compatible_with_each_other
                    },
                    args![apparent_string],
                ));
            }
        } else {
            let apparent_string = self.type_to_string(apparent_type);
            diagnostic = Some(new_diagnostic_chain_for_node(
                diagnostic,
                target,
                if is_call {
                    diag::Type_0_has_no_call_signatures
                } else {
                    diag::Type_0_has_no_construct_signatures
                },
                args![apparent_string],
            ));
        }
        let mut head_message = if is_call {
            diag::This_expression_is_not_callable
        } else {
            diag::This_expression_is_not_constructable
        };
        // Diagnose get accessors incorrectly called as functions
        if is_call_expression(error_target.parent()) && error_target.parent().arguments().is_empty()
        {
            let resolved_symbol = self.get_resolved_symbol_or_nil(error_target);
            if resolved_symbol.is_some()
                && self
                    .sym(resolved_symbol)
                    .flags
                    .intersects(SymbolFlags::GET_ACCESSOR)
            {
                head_message = diag::This_expression_is_not_callable_because_it_is_a_get_accessor_Did_you_mean_to_use_it_without;
            }
        }
        let mut diagnostic =
            new_diagnostic_chain_for_node(diagnostic, target, head_message, args![]);
        if maybe_missing_await {
            diagnostic.add_related_info(Some(new_diagnostic_for_node(
                error_target,
                diag::Did_you_forget_to_use_await,
                args![],
            )));
        }
        diagnostic
    }

    // Go: checker/checker.go:10188 invocationError
    // PORT: Go (#4825) adds the diagnostic, then `invocationErrorRecovery` adds
    // related info to the stored one (an equal diagnostic added before, or this
    // one). See `invocation_error_recovery` for the order here.
    pub fn invocation_error(
        &mut self,
        error_target: Node,
        apparent_type: TypeId,
        kind: SignatureKind,
        related_information: Option<Diagnostic>,
    ) {
        let mut diagnostic = self.invocation_error_details(error_target, apparent_type, kind);
        if related_information.is_some() {
            diagnostic.add_related_info(related_information);
        }
        let recovery = self.invocation_error_recovery(apparent_type, kind);
        if let Some(diagnostic) = self.add_diagnostic(diagnostic) {
            diagnostic.add_related_info(recovery);
        }
    }

    // Go: checker/checker.go:10197 invocationErrorRecovery
    // PORT: Go adds the related info to the diagnostic that `addDiagnostic`
    // returned. The stored `&mut Diagnostic` cannot be kept across the checker
    // calls here, so this returns the related info, and the caller runs it before
    // the add and adds the info to the stored diagnostic after it. The add still
    // compares the diagnostic without this info, as in Go. This only resolves
    // types, and a diagnostic it adds cannot equal the caller's, so the dedup and
    // the sorted lists are the same.
    pub fn invocation_error_recovery(
        &mut self,
        apparent_type: TypeId,
        kind: SignatureKind,
    ) -> Option<Diagnostic> {
        let symbol = self.ty(apparent_type).symbol;
        if symbol.is_nil() {
            return None;
        }
        let import_node = self.export_type_links.get(symbol).originating_import;
        // Create a diagnostic on the originating import if possible onto which we can attach a quickfix
        //  An import call expression cannot be rewritten into another form to correct the error - the only solution is to use `.default` at the use-site
        if import_node.is_some() && !is_import_call(import_node) {
            let target = self.export_type_links.get(symbol).target;
            let target_type = self.get_type_of_symbol(target);
            let sigs = self.get_signatures_of_type(target_type, kind);
            if sigs.is_empty() {
                return None;
            }
            return Some(new_diagnostic_for_node(
                import_node,
                diag::Type_originates_at_this_import_A_namespace_style_import_cannot_be_called_or_constructed_and_will_cause_a_failure_at_runtime_Consider_using_a_default_import_or_import_require_here_instead,
                args![],
            ));
        }
        None
    }

    // Go: checker/checker.go:10213 isGenericFunctionReturningFunction
    pub fn is_generic_function_returning_function(&mut self, signature: SignatureId) -> bool {
        if self.sig(signature).type_parameters.is_empty() {
            return false;
        }
        let return_type = self.get_return_type_of_signature(signature);
        self.is_function_type(return_type)
    }

    // Go: checker/checker.go:10217 skippedGenericFunction
    pub fn skipped_generic_function(&mut self, node: Node, check_mode: CheckMode) {
        if check_mode.intersects(CheckMode::INFERENTIAL) {
            // We have skipped a generic function during inferential typing. Obtain the inference context and
            // indicate this has occurred such that we know a second pass of inference is be needed.
            let context = self.get_inference_context(node);
            self.inference_context_mut(context).flags |= InferenceFlags::SKIPPED_GENERIC_FUNCTION;
        }
    }

    // Go: checker/checker.go:10226 checkTaggedTemplateExpression
    pub fn check_tagged_template_expression(&mut self, node: Node) -> TypeId {
        if !self.check_grammar_tagged_template_chain(node) {
            self.check_grammar_type_arguments(node, node.type_argument_list());
        }
        let signature = self.get_resolved_signature(node, None, CheckMode::NORMAL);
        self.check_deprecated_signature(signature, node);
        self.get_return_type_of_signature(signature)
    }

    // Go: checker/checker.go:10235 checkParenthesizedExpression
    pub fn check_parenthesized_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_expression_ex(node.expression(), check_mode)
    }

    // Go: checker/checker.go:10239 checkClassExpression
    pub fn check_class_expression(&mut self, node: Node) -> TypeId {
        self.check_class_like_declaration(node);
        self.check_node_deferred(node);
        self.check_class_expression_external_helpers(node);
        let symbol = self.get_symbol_of_declaration(node);
        self.get_type_of_symbol(symbol)
    }

    // Go: checker/checker.go:10246 getFirstTransformableStaticClassElement
    pub fn get_first_transformable_static_class_element(&mut self, node: Node) -> Node {
        let will_transform_static_elements_of_decorated_class = !self.legacy_decorators
            && self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators
            && class_or_constructor_parameter_is_decorated(false, node);
        let will_transform_private_elements_or_class_static_blocks = self.language_version
            < LANGUAGE_FEATURE_MINIMUM_TARGET.private_names_and_class_static_blocks
            || self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators;
        let will_transform_initializers = !self.emit_standard_class_fields;
        if will_transform_static_elements_of_decorated_class
            || will_transform_private_elements_or_class_static_blocks
        {
            for member in node.members() {
                if will_transform_static_elements_of_decorated_class
                    && class_element_or_class_element_parameter_is_decorated(false, member, node)
                {
                    let first_decorator = node_slice_element_or_nil(node.decorators(), 0);
                    if first_decorator.is_some() {
                        return first_decorator;
                    }
                    return node;
                } else if will_transform_private_elements_or_class_static_blocks {
                    if is_class_static_block_declaration(member) {
                        return member;
                    } else if is_static(member)
                        && (is_private_identifier_class_element_declaration(member)
                            || will_transform_initializers && is_initialized_property(member))
                    {
                        return member;
                    }
                }
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:10273 checkClassExpressionExternalHelpers
    pub fn check_class_expression_external_helpers(&mut self, node: Node) {
        if node.name().is_some() {
            return;
        }
        let parent = walk_up_outer_expressions(node);
        if !is_named_evaluation_source(parent) {
            return;
        }

        let will_transform_es_decorators = !self.legacy_decorators
            && self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators;
        let location;
        if will_transform_es_decorators && class_or_constructor_parameter_is_decorated(false, node)
        {
            let first_decorator = node_slice_element_or_nil(node.decorators(), 0);
            location = if first_decorator.is_some() {
                first_decorator
            } else {
                node
            };
        } else {
            location = self.get_first_transformable_static_class_element(node);
        }

        if location.is_some() {
            self.check_external_emit_helpers(location, ExternalEmitHelpers::SET_FUNCTION_NAME);
            if (is_property_assignment(parent)
                || is_property_declaration(parent)
                || is_binding_element(parent))
                && is_computed_property_name(parent.name())
            {
                self.check_external_emit_helpers(location, ExternalEmitHelpers::PROP_KEY);
            }
        }
    }

    // Go: checker/checker.go:10301 checkClassExpressionDeferred
    pub fn check_class_expression_deferred(&mut self, node: Node) {
        self.check_source_elements(node.members());
        // ts#64646, Go N' checker.go:10336
        self.check_constructor_declared_properties(node);
        self.register_for_unused_identifiers_check(node);
    }

    // Go: checker/checker.go:10306 checkFunctionExpressionOrObjectLiteralMethod
    pub fn check_function_expression_or_object_literal_method(
        &mut self,
        node: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        self.check_node_deferred(node);
        let full_signature = node.full_signature();
        if full_signature.is_some() {
            self.check_source_element(full_signature);
        }
        if is_function_expression(node) {
            self.check_collisions_for_declaration_name(node, node.name());
        }
        if check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
            && self.is_context_sensitive(node)
        {
            // Skip parameters, return signature with return type that retains noncontextual parts so inferences can still be drawn in an early stage
            if node.type_().is_nil() && !has_context_sensitive_parameters(node) {
                // Return plain anyFunctionType if there is no possibility we'll make inferences from the return type
                let contextual_signature = self.get_contextual_signature(node);
                if contextual_signature.is_some() {
                    let contextual_return_type =
                        self.get_return_type_of_signature(contextual_signature);
                    if self.could_contain_type_variables(contextual_return_type) {
                        if let Some(&cached) = self.context_free_types.get(&node) {
                            return cached;
                        }
                        let return_type = self.get_return_type_from_body(node, check_mode);
                        let return_only_signature = self.new_signature(
                            SignatureFlags::IS_NON_INFERRABLE,
                            Node::NIL,
                            &[],           /*typeParameters*/
                            SymbolId::NIL, /*thisParameter*/
                            &[],
                            return_type,
                            TypePredicateId::NIL, /*resolvedTypePredicate*/
                            0,
                        );
                        let return_only_type = self.new_anonymous_type(
                            node.symbol(),
                            SymbolTable::NIL,
                            &[return_only_signature],
                            &[],
                            &[],
                        );
                        self.ty_mut(return_only_type).object_flags |=
                            ObjectFlags::NON_INFERRABLE_TYPE;
                        self.context_free_types.insert(node, return_only_type);
                        return return_only_type;
                    }
                }
            }
            return self.any_function_type;
        }
        // Grammar checking
        let has_grammar_error = self.check_grammar_function_like_declaration(node);
        if !has_grammar_error && is_function_expression(node) {
            self.check_grammar_for_generator(node);
        }
        // PORT: Go `node.FunctionLikeData().FullSignature` -> fields.rs accessor `full_signature()`.
        let full_signature = node.full_signature();
        if full_signature.is_some() {
            let t = self.get_type_from_type_node(full_signature);
            if self.get_contextual_call_signature(t, node).is_nil() {
                self.error(
                    full_signature,
                    diag::A_JSDoc_type_tag_on_a_function_must_have_a_signature_with_the_correct_number_of_arguments,
                    args![],
                );
            }
        }
        self.contextually_check_function_expression_or_object_literal_method(node, check_mode);
        let symbol = self.get_symbol_of_declaration(node);
        self.get_type_of_symbol(symbol)
    }

    // Go: checker/checker.go:10347 contextuallyCheckFunctionExpressionOrObjectLiteralMethod
    pub fn contextually_check_function_expression_or_object_literal_method(
        &mut self,
        node: Node,
        check_mode: CheckMode,
    ) {
        // Check if function expression is contextually typed and assign parameter types if so.
        if !self
            .node_links
            .get(node)
            .flags
            .intersects(NodeCheckFlags::CONTEXT_CHECKED)
        {
            let contextual_signature = self.get_contextual_signature(node);
            // If a type check is started at a function expression that is an argument of a function call, obtaining the
            // contextual type may recursively get back to here during overload resolution of the call. If so, we will have
            // already assigned contextual types.
            if !self
                .node_links
                .get(node)
                .flags
                .intersects(NodeCheckFlags::CONTEXT_CHECKED)
            {
                self.node_links.get(node).flags |= NodeCheckFlags::CONTEXT_CHECKED;
                let symbol = self.get_symbol_of_declaration(node);
                let symbol_type = self.get_type_of_symbol(symbol);
                let signature = self
                    .get_signatures_of_type(symbol_type, SignatureKind::CALL)
                    .first()
                    .copied()
                    .unwrap_or(SignatureId::NIL);
                if signature.is_nil() {
                    return;
                }
                if self.is_context_sensitive(node) {
                    if contextual_signature.is_some() {
                        let inference_context = self.get_inference_context(node);
                        let mut instantiated_contextual_signature = SignatureId::NIL;
                        if check_mode.intersects(CheckMode::INFERENTIAL) {
                            self.infer_from_annotated_parameters_and_return(
                                signature,
                                contextual_signature,
                                inference_context,
                            );
                            let rest_type = self.get_effective_rest_type(contextual_signature);
                            if rest_type.is_some()
                                && self
                                    .ty(rest_type)
                                    .flags
                                    .intersects(TypeFlags::TYPE_PARAMETER)
                            {
                                let non_fixing_mapper =
                                    self.inference_context(inference_context).non_fixing_mapper;
                                instantiated_contextual_signature = self
                                    .instantiate_signature(contextual_signature, non_fixing_mapper);
                            }
                        }
                        if instantiated_contextual_signature.is_nil() {
                            if inference_context.is_some() {
                                let mapper = self.inference_context(inference_context).mapper;
                                instantiated_contextual_signature =
                                    self.instantiate_signature(contextual_signature, mapper);
                            } else {
                                instantiated_contextual_signature = contextual_signature;
                            }
                        }
                        self.assign_contextual_parameter_types(
                            signature,
                            instantiated_contextual_signature,
                        );
                    } else {
                        // Force resolution of all parameter types such that the absence of a contextual type is consistently reflected.
                        self.assign_non_contextual_parameter_types(signature);
                    }
                } else if contextual_signature.is_some()
                    && node.type_parameter_list().is_nil()
                    && self.sig(contextual_signature).parameters.len() > node.parameters().len()
                {
                    // PORT: Go `node.TypeParameters() == nil` is nil exactly when the list is nil.
                    let inference_context = self.get_inference_context(node);
                    if check_mode.intersects(CheckMode::INFERENTIAL) {
                        self.infer_from_annotated_parameters_and_return(
                            signature,
                            contextual_signature,
                            inference_context,
                        );
                    }
                }
                if contextual_signature.is_some()
                    && self.get_return_type_from_annotation(node).is_nil()
                    && self.sig(signature).resolved_return_type.is_nil()
                {
                    // resolvedReturnType is cached indefinitely, so the return type here has to be computed without CheckModeSkipContextSensitive;
                    // otherwise anyFunctionType could leak as part of the computed (and cached) return type.
                    let return_type = self.get_return_type_from_body(
                        node,
                        check_mode.without(CheckMode::SKIP_CONTEXT_SENSITIVE),
                    );
                    if self.sig(signature).resolved_return_type.is_nil() {
                        self.sig_mut(signature).resolved_return_type = return_type;
                    }
                }
                self.check_signature_declaration(node);
            }
        }
    }

    // Go: checker/checker.go:10403 checkFunctionExpressionOrObjectLiteralMethodDeferred
    pub fn check_function_expression_or_object_literal_method_deferred(&mut self, node: Node) {
        let function_flags = get_function_flags(node);
        let return_type = self.get_return_type_from_annotation(node);
        self.check_all_code_paths_in_non_void_function_return_or_throw(node, return_type);
        let body = node.body();
        if body.is_some() {
            if node.type_().is_nil() {
                // There are some checks that are only performed in getReturnTypeFromBody, that may produce errors
                // we need. An example is the noImplicitAny errors resulting from widening the return expression
                // of a function. Because checking of function expression bodies is deferred, there was never an
                // appropriate time to do this during the main walk of the file (see the comment at the top of
                // checkFunctionExpressionBodies). So it must be done now.
                let signature = self.get_signature_from_declaration(node);
                self.get_return_type_of_signature(signature);
            }
            if is_block(body) {
                self.check_source_element(body);
            } else {
                // From within an async function you can return either a non-promise value or a promise. Any
                // Promise/A+ compatible implementation will always assimilate any foreign promise, so we
                // should not be checking assignability of a promise to the return type. Instead, we need to
                // check assignability of the awaited type of the expression body against the promised type of
                // its return type annotation.
                let expr_type = self.check_expression(body);
                if return_type.is_some() {
                    let return_or_promised_type =
                        self.unwrap_return_type(return_type, function_flags);
                    if return_or_promised_type.is_some() {
                        self.check_return_expression(
                            node,
                            return_or_promised_type,
                            body,
                            body,
                            expr_type,
                            false,
                        );
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:10436 inferFromAnnotatedParametersAndReturn
    pub fn infer_from_annotated_parameters_and_return(
        &mut self,
        sig: SignatureId,
        context: SignatureId,
        inference_context: InferenceContextId,
    ) {
        let length = self.sig(sig).parameters.len()
            - if self.signature_has_rest_parameter(sig) {
                1
            } else {
                0
            };
        for i in 0..length {
            let parameter = self.sig(sig).parameters[i];
            let declaration = self.sym(parameter).value_declaration;
            let type_node = declaration.type_();
            if type_node.is_some() {
                let declared = self.get_type_from_type_node(type_node);
                let source = self.add_optionality_ex(
                    declared,
                    false, /*isProperty*/
                    is_optional_declaration(declaration),
                );
                let target = self.get_type_at_position(context, i as i32);
                self.infer_types(
                    inference_context,
                    source,
                    target,
                    InferencePriority::NONE,
                    false,
                );
            }
        }
        let declaration = self.sig(sig).declaration();
        if declaration.is_some() {
            let return_type_node = declaration.type_();
            if return_type_node.is_some() {
                let source = self.get_type_from_type_node(return_type_node);
                let target = self.get_return_type_of_signature(context);
                self.infer_types(
                    inference_context,
                    source,
                    target,
                    InferencePriority::NONE,
                    false,
                );
            }
        }
    }

    // Go: checker/checker.go:10461 getContextualSignature
    // Return the contextual signature for a given expression node. A contextual type provides a
    // contextual signature if it has a single call signature and if that call signature is non-generic.
    // If the contextual type is a union type, get the signature from each type possible and if they are
    // all identical ignoring their return type, the result is same signature but with return type as
    // union type of return types from these signatures
    pub fn get_contextual_signature(&mut self, node: Node) -> SignatureId {
        let t = self.get_apparent_type_of_contextual_type(node, ContextFlags::SIGNATURE);
        if t.is_nil() {
            return SignatureId::NIL;
        }
        if !self.ty(t).flags.intersects(TypeFlags::UNION) {
            return self.get_contextual_call_signature(t, node);
        }
        let mut signature_list: Vec<SignatureId> = Vec::new();
        let types = self.ty(t).types_list();
        for current in types {
            let signature = self.get_contextual_call_signature(current, node);
            if signature.is_some() {
                if !signature_list.is_empty()
                    && self.compare_signatures_identical(
                        signature_list[0],
                        signature,
                        false, /*partialMatch*/
                        true,  /*ignoreThisTypes*/
                        true,  /*ignoreReturnTypes*/
                        &mut |c: &mut Checker, s: TypeId, t: TypeId| {
                            c.compare_types_identical(s, t)
                        },
                    ) == Ternary::FALSE
                {
                    // Signatures aren't identical, do not use
                    return SignatureId::NIL;
                }
                // Use this signature for contextual union signature
                signature_list.push(signature);
            }
        }
        match signature_list.len() {
            0 => return SignatureId::NIL,
            1 => return signature_list[0],
            _ => {}
        }
        // Result is union of signatures collected (return type is union of return types of this signature set)
        self.create_union_signature(signature_list[0], &signature_list)
    }
}
