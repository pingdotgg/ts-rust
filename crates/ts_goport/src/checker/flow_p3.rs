//! Port of `checker/flow.go` lines 1855-2734: reference roots, switch
//! exhaustiveness, effects signatures, dotted names, initial and assigned
//! types, reachability, post-super analysis and assignment marking.

use crate::prelude::*;

impl Checker {
    // Go: checker/flow.go:1876 getReferenceRoot
    // PERF: chkA. The parent, its kind and its operator are read once
    // (`node_parent_and_kind`).
    pub fn get_reference_root(&mut self, node: Node) -> Node {
        let (parent, parent_kind) = node_parent_and_kind(node);
        if parent_kind == SyntaxKind::ParenthesizedExpression
            || parent_kind == SyntaxKind::BinaryExpression && {
                let operator = parent.operator_token().kind();
                operator == SyntaxKind::EqualsToken && parent.left() == node
                    || operator == SyntaxKind::CommaToken && parent.right() == node
            }
        {
            return self.get_reference_root(parent);
        }
        node
    }

    // Go: checker/flow.go:1886 hasMatchingArgument
    pub fn has_matching_argument(&mut self, expression: Node, reference: Node) -> bool {
        // PERF: the argument list is program data (`NodeSlice` is `Copy`), so
        // the loop reads it in place with no copy.
        for argument in expression.arguments() {
            if self.is_or_contains_matching_reference(reference, argument)
                || self.optional_chain_contains_reference(argument, reference)
            {
                return true;
            }
        }
        if is_property_access_expression(expression.expression())
            && self
                .is_or_contains_matching_reference(reference, expression.expression().expression())
        {
            return true;
        }
        false
    }

    // Go: checker/flow.go:1898 isOrContainsMatchingReference
    pub fn is_or_contains_matching_reference(&mut self, source: Node, target: Node) -> bool {
        self.is_matching_reference(source, target)
            || self.contains_matching_reference(source, target)
    }

    // Return a new type in which occurrences of the string, number and bigint primitives and placeholder template
    // literal types in typeWithPrimitives have been replaced with occurrences of compatible and more specific types
    // from typeWithLiterals. This is essentially a limited form of intersection between the two types. We avoid a
    // true intersection because it is more costly and, when applied to union types, generates a large number of
    // types we don't actually care about.
    // Go: checker/flow.go:1907 replacePrimitivesWithLiterals
    pub fn replace_primitives_with_literals(
        &mut self,
        type_with_primitives: TypeId,
        type_with_literals: TypeId,
    ) -> TypeId {
        if self.maybe_type_of_kind(
            type_with_primitives,
            TypeFlags::STRING
                | TypeFlags::TEMPLATE_LITERAL
                | TypeFlags::NUMBER
                | TypeFlags::BIG_INT,
        ) && self.maybe_type_of_kind(
            type_with_literals,
            TypeFlags::STRING_LITERAL
                | TypeFlags::TEMPLATE_LITERAL
                | TypeFlags::STRING_MAPPING
                | TypeFlags::NUMBER_LITERAL
                | TypeFlags::BIG_INT_LITERAL,
        ) {
            return self.map_type(type_with_primitives, &mut |c: &mut Checker,
                                                             t: TypeId|
             -> TypeId {
                let flags = c.ty(t).flags;
                if flags.intersects(TypeFlags::STRING) {
                    return c.extract_types_of_kind(
                        type_with_literals,
                        TypeFlags::STRING
                            | TypeFlags::STRING_LITERAL
                            | TypeFlags::TEMPLATE_LITERAL
                            | TypeFlags::STRING_MAPPING,
                    );
                }
                if c.is_pattern_literal_type(t)
                    && !c.maybe_type_of_kind(
                        type_with_literals,
                        TypeFlags::STRING | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
                    )
                {
                    return c.extract_types_of_kind(type_with_literals, TypeFlags::STRING_LITERAL);
                }
                if flags.intersects(TypeFlags::NUMBER) {
                    return c.extract_types_of_kind(
                        type_with_literals,
                        TypeFlags::NUMBER | TypeFlags::NUMBER_LITERAL,
                    );
                }
                if flags.intersects(TypeFlags::BIG_INT) {
                    return c.extract_types_of_kind(
                        type_with_literals,
                        TypeFlags::BIG_INT | TypeFlags::BIG_INT_LITERAL,
                    );
                }
                t
            });
        }
        type_with_primitives
    }

    // Go: checker/flow.go:1928 isCoercibleUnderDoubleEquals
    // PORT: Go package function on `*Type`; a `Checker` method here so it can
    // read the type arena.
    pub fn is_coercible_under_double_equals(&self, source: TypeId, target: TypeId) -> bool {
        self.ty(source)
            .flags
            .intersects(TypeFlags::NUMBER | TypeFlags::STRING | TypeFlags::BOOLEAN_LITERAL)
            && self
                .ty(target)
                .flags
                .intersects(TypeFlags::NUMBER | TypeFlags::STRING | TypeFlags::BOOLEAN)
    }

    // Go: checker/flow.go:1933 isExhaustiveSwitchStatement
    pub fn is_exhaustive_switch_statement(&mut self, node: Node) -> bool {
        // flowskip1 verify: an effect site (flow_skip.rs).
        self.flow_skip.effects += 1;
        let state = self.switch_statement_links.get(node).exhaustive_state;
        if state == ExhaustiveState::UNKNOWN {
            // Indicate resolution is in process
            self.switch_statement_links.get(node).exhaustive_state = ExhaustiveState::COMPUTING;
            let is_exhaustive = self.compute_exhaustive_switch_statement(node);
            let links = self.switch_statement_links.get(node);
            if links.exhaustive_state == ExhaustiveState::COMPUTING {
                links.exhaustive_state = if is_exhaustive {
                    ExhaustiveState::TRUE
                } else {
                    ExhaustiveState::FALSE
                };
            }
        } else if state == ExhaustiveState::COMPUTING {
            // Resolve circularity to false
            self.switch_statement_links.get(node).exhaustive_state = ExhaustiveState::FALSE;
        }
        self.switch_statement_links.get(node).exhaustive_state == ExhaustiveState::TRUE
    }

    // Go: checker/flow.go:1949 computeExhaustiveSwitchStatement
    pub fn compute_exhaustive_switch_statement(&mut self, node: Node) -> bool {
        if is_type_of_expression(node.expression()) {
            let witnesses = self.get_switch_clause_type_of_witnesses(node);
            // PORT: Go checks `witnesses == nil`. A Rust `Vec` cannot tell nil
            // from empty; an empty witness list only occurs for a switch with no
            // clauses, and then Go also returns false below (no not-equal facts
            // are collected, so every type fails the check).
            if witnesses.is_empty() {
                return false;
            }
            let expr_type = self.check_expression_cached(node.expression().expression());
            let operand_constraint = self.get_base_constraint_or_type(expr_type);
            // Get the not-equal flags for all handled cases.
            let not_equal_facts = self.get_not_equal_facts_from_typeof_switch(0, 0, &witnesses);
            if self
                .ty(operand_constraint)
                .flags
                .intersects(TypeFlags::ANY_OR_UNKNOWN)
            {
                // We special case the top types to be exhaustive when all cases are handled.
                return (TypeFacts::ALL_TYPEOF_NE & not_equal_facts) == TypeFacts::ALL_TYPEOF_NE;
            }
            // A missing not-equal flag indicates that the type wasn't handled by some case.
            return !self.some_type(operand_constraint, &mut |c: &mut Checker,
                                                             t: TypeId|
             -> bool {
                c.get_type_facts(t, not_equal_facts) == not_equal_facts
            });
        }
        let expr_type = self.check_expression_cached(node.expression());
        let t = self.get_base_constraint_or_type(expr_type);
        if !self.is_literal_type(t) {
            return false;
        }
        let switch_types = self.get_switch_clause_types(node);
        if switch_types.is_empty()
            || switch_types
                .iter()
                .any(|&t| self.is_neither_unit_type_nor_never(t))
        {
            return false;
        }
        let mapped = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.get_regular_type_of_literal_type(t)
        });
        self.each_type_contained_in(mapped, &switch_types)
    }

    // Go: checker/flow.go:1978 eachTypeContainedIn
    pub fn each_type_contained_in(&self, source: TypeId, types: &[TypeId]) -> bool {
        if self.ty(source).flags.intersects(TypeFlags::UNION) {
            return !self.ty(source).types().iter().any(|t| !types.contains(t));
        }
        types.contains(&source)
    }

    // Get the type names from all cases in a switch on `typeof`. The default clause and/or duplicate type names are
    // represented as empty strings. Return nil if one or more case clause expressions are not string literals.
    // Go: checker/flow.go:1989 getSwitchClauseTypeOfWitnesses
    // PORT: Go nil is an empty `Vec` (see computeExhaustiveSwitchStatement).
    pub fn get_switch_clause_type_of_witnesses(&mut self, node: Node) -> Vec<String> {
        if !self.switch_statement_links.get(node).witnesses_computed {
            let clauses = node.case_block().clauses().nodes().to_vec();
            let mut witnesses: Vec<String> = vec![String::new(); clauses.len()];
            for (i, clause) in clauses.iter().enumerate() {
                if clause.kind() == SyntaxKind::CaseClause {
                    if !is_string_literal_like(clause.expression()) {
                        witnesses = Vec::new();
                        break;
                    }
                    let text = clause.expression().text();
                    if !witnesses.iter().any(|w| w == text) {
                        witnesses[i] = text.to_string();
                    }
                }
            }
            let links = self.switch_statement_links.get(node);
            links.witnesses = witnesses;
            links.witnesses_computed = true;
        }
        self.switch_statement_links.get(node).witnesses.clone()
    }

    // Return the combined not-equal type facts for all cases except those between the start and end indices.
    // Go: checker/flow.go:2012 getNotEqualFactsFromTypeofSwitch
    pub fn get_not_equal_facts_from_typeof_switch(
        &mut self,
        start: i32,
        end: i32,
        witnesses: &[String],
    ) -> TypeFacts {
        let mut facts: TypeFacts = TypeFacts::NONE;
        for (i, witness) in witnesses.iter().enumerate() {
            let i = i as i32;
            if (i < start || i >= end) && !witness.is_empty() {
                // PORT: Go package var `typeofNEFacts` (flow.go:623) is
                // `TYPEOF_NE_FACTS`, a map keyed by the typeof name.
                let f = match TYPEOF_NE_FACTS.get(witness.as_str()) {
                    Some(f) => *f,
                    None => TypeFacts::TYPEOF_NE_HOST_OBJECT,
                };
                facts |= f;
            }
        }
        facts
    }

    // Go: checker/flow.go:2026 getSwitchClauseTypes
    pub fn get_switch_clause_types(&mut self, node: Node) -> Vec<TypeId> {
        if !self.switch_statement_links.get(node).switch_types_computed {
            let clauses = node.case_block().clauses().nodes().to_vec();
            let mut types: Vec<TypeId> = vec![TypeId::NIL; clauses.len()];
            for (i, clause) in clauses.iter().enumerate() {
                types[i] = self.get_type_of_switch_clause(*clause);
            }
            let links = self.switch_statement_links.get(node);
            links.switch_types = types;
            links.switch_types_computed = true;
        }
        self.switch_statement_links.get(node).switch_types.clone()
    }

    // Go: checker/flow.go:2040 getTypeOfSwitchClause
    pub fn get_type_of_switch_clause(&mut self, clause: Node) -> TypeId {
        if clause.kind() == SyntaxKind::CaseClause {
            let t = self.get_type_of_expression(clause.expression());
            return self.get_regular_type_of_literal_type(t);
        }
        self.never_type
    }

    // Go: checker/flow.go:2047 getEffectsSignature
    pub fn get_effects_signature(&mut self, node: Node) -> SignatureId {
        let mut signature = self.signature_links.get(node).effects_signature;
        if signature.is_nil() {
            // flowskip1 verify: an effect site (flow_skip.rs).
            self.flow_skip.effects += 1;
            // A call expression parented by an expression statement is a potential assertion. Other call
            // expressions are potential type predicate function calls. In order to avoid triggering
            // circularities in control flow analysis, we use getTypeOfDottedName when resolving the call
            // target expression of an assertion.
            let mut func_type = TypeId::NIL;
            if is_binary_expression(node) {
                let right_type = self.check_non_null_expression(node.right());
                func_type = self.get_symbol_has_instance_method_of_object_type(right_type);
            } else if is_expression_statement(node.parent()) {
                func_type =
                    self.get_type_of_dotted_name(node.expression(), None /*diagnostic*/);
            } else if node.expression().kind() != SyntaxKind::SuperKeyword {
                if is_optional_chain(node) {
                    let expr_type = self.check_expression(node.expression());
                    let optional_type =
                        self.get_optional_expression_type(expr_type, node.expression());
                    func_type = self.check_non_null_type(optional_type, node.expression());
                } else {
                    func_type = self.check_non_null_expression(node.expression());
                }
            }
            let mut apparent_type = TypeId::NIL;
            if func_type.is_some() {
                apparent_type = self.get_apparent_type(func_type);
            }
            let target = if apparent_type.is_some() {
                apparent_type
            } else {
                self.unknown_type
            };
            let signatures = self.get_signatures_of_type(target, SignatureKind::CALL);
            if signatures.len() == 1 && self.sig(signatures[0]).type_parameters.is_empty() {
                signature = signatures[0];
            } else if signatures
                .iter()
                .any(|&s| self.has_type_predicate_or_never_return_type(s))
            {
                signature = self.get_resolved_signature(node, None, CheckMode::NORMAL);
            }
            if !(signature.is_some() && self.has_type_predicate_or_never_return_type(signature)) {
                signature = self.unknown_signature;
            }
            self.signature_links.get(node).effects_signature = signature;
        }
        if signature == self.unknown_signature {
            return SignatureId::NIL;
        }
        signature
    }

    /**
     * Get the type of the `[Symbol.hasInstance]` method of an object type.
     */
    // Go: checker/flow.go:2093 getSymbolHasInstanceMethodOfObjectType
    pub fn get_symbol_has_instance_method_of_object_type(&mut self, t: TypeId) -> TypeId {
        let has_instance_property_name =
            self.get_property_name_for_known_symbol_name("hasInstance");
        if self.all_types_assignable_to_kind(t, TypeFlags::NON_PRIMITIVE) {
            let has_instance_property = self.get_property_of_type(t, &has_instance_property_name);
            if has_instance_property.is_some() {
                let has_instance_property_type = self.get_type_of_symbol(has_instance_property);
                if has_instance_property_type.is_some()
                    && !self
                        .get_signatures_of_type(has_instance_property_type, SignatureKind::CALL)
                        .is_empty()
                {
                    return has_instance_property_type;
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/flow.go:2107 getPropertyNameForKnownSymbolName
    pub fn get_property_name_for_known_symbol_name(&mut self, symbol_name: &str) -> String {
        let get_ctor = self.get_global_es_symbol_constructor_symbol_or_nil.clone();
        let ctor_type = get_ctor(self);
        if ctor_type.is_some() {
            let ctor_symbol_type = self.get_type_of_symbol(ctor_type);
            let unique_type = self.get_type_of_property_of_type(ctor_symbol_type, symbol_name);
            if unique_type.is_some() && self.is_type_usable_as_property_name(unique_type) {
                return self.get_property_name_from_type(unique_type);
            }
        }
        format!("{}@{}", INTERNAL_SYMBOL_NAME_PREFIX, symbol_name)
    }

    // We require the dotted function name in an assertion expression to be comprised of identifiers
    // that reference function, method, class or value module symbols; or variable, property or
    // parameter symbols with declarations that have explicit type annotations. Such references are
    // resolvable with no possibility of triggering circularities in control flow analysis.
    // Go: checker/flow.go:2122 getTypeOfDottedName
    // PORT: Go passes the `*ast.Diagnostic` that `c.error` returned, so
    // `getExplicitTypeOfSymbol` can add related info to the stored diagnostic.
    // The stored `&mut Diagnostic` cannot be kept across the checker calls here,
    // so `diagnostic` collects that related info (`None` is Go nil), and the
    // caller adds it to the stored diagnostic (see `check_call_expression`).
    pub fn get_type_of_dotted_name(
        &mut self,
        node: Node,
        mut diagnostic: Option<&mut Vec<Diagnostic>>,
    ) -> TypeId {
        if !node.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            match node.kind() {
                SyntaxKind::Identifier => {
                    let resolved = self.get_resolved_symbol(node);
                    let symbol = self.get_export_symbol_of_value_symbol_if_exported(resolved);
                    return self.get_explicit_type_of_symbol(symbol, diagnostic);
                }
                SyntaxKind::ThisKeyword => {
                    return self.get_explicit_this_type(node);
                }
                SyntaxKind::SuperKeyword => {
                    return self.check_super_expression(node);
                }
                SyntaxKind::PropertyAccessExpression => {
                    let t =
                        self.get_type_of_dotted_name(node.expression(), diagnostic.as_deref_mut());
                    if t.is_some() {
                        let name = node.name();
                        let mut prop = SymbolId::NIL;
                        if is_private_identifier(name) {
                            let t_symbol = self.ty(t).symbol;
                            if t_symbol.is_some() {
                                let prop_name =
                                    self.private_identifier_symbol_name(t_symbol, name.text());
                                prop = self.get_property_of_type(t, &prop_name);
                            }
                        } else {
                            prop = self.get_property_of_type(t, name.text());
                        }
                        if prop.is_some() {
                            return self.get_explicit_type_of_symbol(prop, diagnostic);
                        }
                    }
                }
                SyntaxKind::ParenthesizedExpression => {
                    return self.get_type_of_dotted_name(node.expression(), diagnostic);
                }
                _ => {}
            }
        }
        TypeId::NIL
    }

    // Go: checker/flow.go:2155 getExplicitTypeOfSymbol
    // PORT: `diagnostic` collects the related info (see `get_type_of_dotted_name`).
    pub fn get_explicit_type_of_symbol(
        &mut self,
        symbol: SymbolId,
        diagnostic: Option<&mut Vec<Diagnostic>>,
    ) -> TypeId {
        let symbol = self.resolve_symbol(symbol);
        if !self.resolving_explicit_type_of_symbol.insert(symbol) {
            return TypeId::NIL;
        }
        // PORT: Go `defer c.resolvingExplicitTypeOfSymbol.Delete(symbol)`. The
        // body is in `get_explicit_type_of_symbol_worker`, so every return path
        // deletes the symbol here. A panic skips the delete, as it skips the
        // pops of the other checker stacks (`type_resolutions`).
        let result = self.get_explicit_type_of_symbol_worker(symbol, diagnostic);
        self.resolving_explicit_type_of_symbol.remove(&symbol);
        result
    }

    // PORT: Rust-only: the part of Go `getExplicitTypeOfSymbol` after the
    // `resolvingExplicitTypeOfSymbol` guard. `symbol` is already resolved.
    fn get_explicit_type_of_symbol_worker(
        &mut self,
        symbol: SymbolId,
        mut diagnostic: Option<&mut Vec<Diagnostic>>,
    ) -> TypeId {
        let flags = self.sym(symbol).flags;
        if flags.intersects(
            SymbolFlags::FUNCTION
                | SymbolFlags::METHOD
                | SymbolFlags::CLASS
                | SymbolFlags::VALUE_MODULE,
        ) {
            return self.get_type_of_symbol(symbol);
        }
        if flags.intersects(SymbolFlags::VARIABLE | SymbolFlags::PROPERTY) {
            if self.sym(symbol).check_flags.intersects(CheckFlags::MAPPED) {
                let origin = self.mapped_symbol_links.get(symbol).synthetic_origin;
                if origin.is_some()
                    && self
                        .get_explicit_type_of_symbol(origin, diagnostic.as_deref_mut())
                        .is_some()
                {
                    return self.get_type_of_symbol(symbol);
                }
            }
            let declaration = self.sym(symbol).value_declaration;
            if declaration.is_some() {
                if self.is_declaration_with_explicit_type_annotation(declaration) {
                    return self.get_type_of_symbol(symbol);
                }
                if is_variable_declaration(declaration)
                    && is_for_of_statement(declaration.parent().parent())
                {
                    let statement = declaration.parent().parent();
                    let expression_type = self
                        .get_type_of_dotted_name(statement.expression(), None /*diagnostic*/);
                    if expression_type.is_some() {
                        let use_ = if statement.await_modifier().is_some() {
                            IterationUse::FOR_AWAIT_OF
                        } else {
                            IterationUse::FOR_OF
                        };
                        let undefined_type = self.undefined_type;
                        return self.check_iterated_type_or_element_type(
                            use_,
                            expression_type,
                            undefined_type,
                            Node::NIL, /*errorNode*/
                        );
                    }
                }
                if let Some(diagnostic) = diagnostic {
                    let symbol_name = self.symbol_to_string(symbol);
                    let related = create_diagnostic_for_node(
                        declaration,
                        diag::X_0_needs_an_explicit_type_annotation,
                        args![symbol_name],
                    );
                    diagnostic.push(related);
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/flow.go:2197 isDeclarationWithExplicitTypeAnnotation
    pub fn is_declaration_with_explicit_type_annotation(&mut self, node: Node) -> bool {
        (is_variable_declaration(node)
            || is_property_declaration(node)
            || is_property_signature_declaration(node)
            || is_parameter_declaration(node))
            && node.type_().is_some()
            || self.is_expando_property_function_with_return_type_annotation(node)
    }

    // Go: checker/flow.go:2202 isExpandoPropertyFunctionWithReturnTypeAnnotation
    pub fn is_expando_property_function_with_return_type_annotation(&mut self, node: Node) -> bool {
        if is_binary_expression(node) {
            let expr = node.right();
            if is_function_like(expr) && expr.type_().is_some() {
                return true;
            }
        }
        false
    }

    // Go: checker/flow.go:2211 hasTypePredicateOrNeverReturnType
    pub fn has_type_predicate_or_never_return_type(&mut self, sig: SignatureId) -> bool {
        if self.get_type_predicate_of_signature(sig).is_some() {
            return true;
        }
        let declaration = self.sig(sig).declaration;
        if declaration.is_some() {
            let mut return_type = self.get_return_type_from_annotation(declaration);
            if return_type.is_nil() {
                return_type = self.unknown_type;
            }
            return self.ty(return_type).flags.intersects(TypeFlags::NEVER);
        }
        false
    }

    // Go: checker/flow.go:2215 getExplicitThisType
    pub fn get_explicit_this_type(&mut self, node: Node) -> TypeId {
        let container = get_this_container(
            node, false, /*includeArrowFunctions*/
            false, /*includeClassComputedPropertyName*/
        );
        if is_function_like(container) {
            let signature = self.get_signature_from_declaration(container);
            let this_parameter = self.sig(signature).this_parameter;
            if this_parameter.is_some() {
                return self.get_explicit_type_of_symbol(this_parameter, None);
            }
        }
        if container.parent().is_some() && is_class_like(container.parent()) {
            let symbol = self.get_symbol_of_declaration(container.parent());
            if is_static(container) {
                return self.get_type_of_symbol(symbol);
            } else {
                let declared = self.get_declared_type_of_symbol(symbol);
                return self.ty(declared).as_interface_type().this_type;
            }
        }
        TypeId::NIL
    }

    // Go: checker/flow.go:2234 getInitialType
    pub fn get_initial_type(&mut self, node: Node) -> TypeId {
        match node.kind() {
            SyntaxKind::VariableDeclaration => {
                return self.get_initial_type_of_variable_declaration(node);
            }
            SyntaxKind::BindingElement => return self.get_initial_type_of_binding_element(node),
            _ => {}
        }
        panic!("Unhandled case in getInitialType")
    }

    // Go: checker/flow.go:2244 getInitialTypeOfVariableDeclaration
    pub fn get_initial_type_of_variable_declaration(&mut self, node: Node) -> TypeId {
        if node.initializer().is_some() {
            return self.get_type_of_initializer(node.initializer());
        }
        if is_for_in_statement(node.parent().parent()) {
            return self.string_type;
        }
        if is_for_of_statement(node.parent().parent()) {
            let t = self.check_right_hand_side_of_for_of(node.parent().parent());
            if t.is_some() {
                return t;
            }
        }
        self.error_type
    }

    // Go: checker/flow.go:2260 getTypeOfInitializer
    pub fn get_type_of_initializer(&mut self, node: Node) -> TypeId {
        // Return the cached type if one is available. If the type of the variable was inferred
        // from its initializer, we'll already have cached the type. Otherwise we compute it now
        // without caching such that transient types are reflected.
        if self.type_node_links.has(node) {
            let t = self.type_node_links.get(node).resolved_type;
            if t.is_some() {
                return t;
            }
        }
        self.get_type_of_expression(node)
    }

    // Go: checker/flow.go:2273 getInitialTypeOfBindingElement
    pub fn get_initial_type_of_binding_element(&mut self, node: Node) -> TypeId {
        let pattern = node.parent();
        let parent_type = self.get_initial_type(pattern.parent());
        let t = if is_object_binding_pattern(pattern) {
            self.get_type_of_destructured_property(
                parent_type,
                get_binding_element_property_name(node),
            )
        } else if !has_dot_dot_dot_token(node) {
            let index = pattern
                .elements()
                .iter()
                .position(|e| e == node)
                .map_or(-1, |i| i as i32);
            self.get_type_of_destructured_array_element(parent_type, index)
        } else {
            self.get_type_of_destructured_spread_expression(parent_type)
        };
        self.get_type_with_default(t, node.initializer())
    }

    // Go: checker/flow.go:2288 getAssignedType
    pub fn get_assigned_type(&mut self, node: Node) -> TypeId {
        let parent = node.parent();
        match parent.kind() {
            SyntaxKind::ForInStatement => return self.string_type,
            SyntaxKind::ForOfStatement => {
                let t = self.check_right_hand_side_of_for_of(parent);
                if t.is_some() {
                    return t;
                }
            }
            SyntaxKind::BinaryExpression => {
                return self.get_assigned_type_of_binary_expression(parent);
            }
            SyntaxKind::DeleteExpression => return self.undefined_type,
            SyntaxKind::ArrayLiteralExpression => {
                return self.get_assigned_type_of_array_literal_element(parent, node);
            }
            SyntaxKind::SpreadElement => {
                return self.get_assigned_type_of_spread_expression(parent);
            }
            SyntaxKind::PropertyAssignment => {
                return self.get_assigned_type_of_property_assignment(parent);
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                return self.get_assigned_type_of_shorthand_property_assignment(parent);
            }
            _ => {}
        }
        self.error_type
    }

    // Go: checker/flow.go:2314 getAssignedTypeOfBinaryExpression
    pub fn get_assigned_type_of_binary_expression(&mut self, node: Node) -> TypeId {
        let is_destructuring_default_assignment = is_array_literal_expression(node.parent())
            && self.is_destructuring_assignment_target(node.parent())
            || is_property_assignment(node.parent())
                && self.is_destructuring_assignment_target(node.parent().parent());
        if is_destructuring_default_assignment {
            let t = self.get_assigned_type(node);
            return self.get_type_with_default(t, node.right());
        }
        self.get_type_of_expression(node.right())
    }

    // Go: checker/flow.go:2323 getAssignedTypeOfArrayLiteralElement
    pub fn get_assigned_type_of_array_literal_element(
        &mut self,
        node: Node,
        element: Node,
    ) -> TypeId {
        let t = self.get_assigned_type(node);
        let index = node
            .elements()
            .iter()
            .position(|e| e == element)
            .map_or(-1, |i| i as i32);
        self.get_type_of_destructured_array_element(t, index)
    }

    // Go: checker/flow.go:2327 getTypeOfDestructuredArrayElement
    pub fn get_type_of_destructured_array_element(&mut self, t: TypeId, index: i32) -> TypeId {
        if self.every_type(t, &mut |c: &mut Checker, t: TypeId| c.is_tuple_like_type(t)) {
            let element_type = self.get_tuple_element_type(t, index);
            if element_type.is_some() {
                return element_type;
            }
        }
        let undefined_type = self.undefined_type;
        let element_type = self.check_iterated_type_or_element_type(
            IterationUse::DESTRUCTURING,
            t,
            undefined_type,
            Node::NIL, /*errorNode*/
        );
        if element_type.is_some() {
            return self.include_undefined_in_index_signature(element_type);
        }
        self.error_type
    }

    // Go: checker/flow.go:2339 includeUndefinedInIndexSignature
    pub fn include_undefined_in_index_signature(&mut self, t: TypeId) -> TypeId {
        if t.is_nil() {
            return TypeId::NIL;
        }
        if self.compiler_options.no_unchecked_indexed_access == Tristate::True {
            let missing_type = self.missing_type;
            return self.get_union_type(&[t, missing_type]);
        }
        t
    }

    // Go: checker/flow.go:2349 getAssignedTypeOfSpreadExpression
    pub fn get_assigned_type_of_spread_expression(&mut self, node: Node) -> TypeId {
        let t = self.get_assigned_type(node.parent());
        self.get_type_of_destructured_spread_expression(t)
    }

    // Go: checker/flow.go:2353 getTypeOfDestructuredSpreadExpression
    pub fn get_type_of_destructured_spread_expression(&mut self, t: TypeId) -> TypeId {
        let undefined_type = self.undefined_type;
        let mut element_type = self.check_iterated_type_or_element_type(
            IterationUse::DESTRUCTURING,
            t,
            undefined_type,
            Node::NIL, /*errorNode*/
        );
        if element_type.is_nil() {
            element_type = self.error_type;
        }
        self.create_array_type(element_type)
    }

    // Go: checker/flow.go:2361 getAssignedTypeOfPropertyAssignment
    pub fn get_assigned_type_of_property_assignment(&mut self, node: Node) -> TypeId {
        let t = self.get_assigned_type(node.parent());
        self.get_type_of_destructured_property(t, node.name())
    }

    // Go: checker/flow.go:2365 getTypeOfDestructuredProperty
    pub fn get_type_of_destructured_property(&mut self, t: TypeId, name: Node) -> TypeId {
        let name_type = self.get_literal_type_from_property_name(name);
        if !self.is_type_usable_as_property_name(name_type) {
            return self.error_type;
        }
        let text = self.get_property_name_from_type(name_type);
        let prop_type = self.get_type_of_property_of_type(t, &text);
        if prop_type.is_some() {
            return prop_type;
        }
        let index_info = self.get_applicable_index_info_for_name(t, &text);
        if index_info.is_some() {
            let value_type = self.index_info(index_info).value_type;
            return self.include_undefined_in_index_signature(value_type);
        }
        self.error_type
    }

    // Go: checker/flow.go:2380 getAssignedTypeOfShorthandPropertyAssignment
    pub fn get_assigned_type_of_shorthand_property_assignment(&mut self, node: Node) -> TypeId {
        let t = self.get_assigned_type_of_property_assignment(node);
        self.get_type_with_default(t, node.object_assignment_initializer())
    }

    // Go: checker/flow.go:2384 isDestructuringAssignmentTarget
    pub fn is_destructuring_assignment_target(&mut self, parent: Node) -> bool {
        is_binary_expression(parent.parent()) && parent.parent().left() == parent
            || is_for_of_statement(parent.parent()) && parent.parent().initializer() == parent
    }

    // Go: checker/flow.go:2389 getTypeWithDefault
    pub fn get_type_with_default(&mut self, t: TypeId, default_expression: Node) -> TypeId {
        if default_expression.is_some() {
            let non_undefined = self.get_non_undefined_type(t);
            let default_type = self.get_type_of_expression(default_expression);
            return self.get_union_type(&[non_undefined, default_type]);
        }
        t
    }

    // Remove those constituent types of declaredType to which no constituent type of assignedType is assignable.
    // For example, when a variable of type number | string | boolean is assigned a value of type number | boolean,
    // we remove type string.
    // Go: checker/flow.go:2399 getAssignmentReducedType
    pub fn get_assignment_reduced_type(
        &mut self,
        declared_type: TypeId,
        assigned_type: TypeId,
    ) -> TypeId {
        if declared_type == assigned_type {
            return declared_type;
        }
        if self.ty(assigned_type).flags.intersects(TypeFlags::NEVER) {
            return assigned_type;
        }
        let key = AssignmentReducedKey {
            id1: self.ty(declared_type).id,
            id2: self.ty(assigned_type).id,
        };
        let mut result = self
            .assignment_reduced_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if result.is_nil() {
            result = self.get_assignment_reduced_type_worker(declared_type, assigned_type);
            self.assignment_reduced_types.insert(key, result);
        }
        result
    }

    // Go: checker/flow.go:2415 getAssignmentReducedTypeWorker
    pub fn get_assignment_reduced_type_worker(
        &mut self,
        declared_type: TypeId,
        assigned_type: TypeId,
    ) -> TypeId {
        let filtered_type =
            self.filter_type(declared_type, &mut |c: &mut Checker, t: TypeId| -> bool {
                c.type_maybe_assignable_to(assigned_type, t)
            });
        // Ensure that we narrow to fresh types if the assignment is a fresh boolean literal type.
        let mut reduced_type = filtered_type;
        if self
            .ty(assigned_type)
            .flags
            .intersects(TypeFlags::BOOLEAN_LITERAL)
            && self.is_fresh_literal_type(assigned_type)
        {
            reduced_type = self.map_type(filtered_type, &mut |c: &mut Checker, t: TypeId| {
                c.get_fresh_type_of_literal_type(t)
            });
        }
        // Our crude heuristic produces an invalid result in some cases: see GH#26130.
        // For now, when that happens, we give up and don't narrow at all.  (This also
        // means we'll never narrow for erroneous assignments where the assigned type
        // is not assignable to the declared type.)
        if self.is_type_assignable_to(assigned_type, reduced_type) {
            return reduced_type;
        }
        declared_type
    }

    // Go: checker/flow.go:2434 typeMaybeAssignableTo
    pub fn type_maybe_assignable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        if !self.ty(source).flags.intersects(TypeFlags::UNION) {
            return self.is_type_assignable_to(source, target);
        }
        // Quick exit when source union contains the target type
        if self.contains_type(self.ty(source).types(), target) {
            return true;
        }
        // Otherwise, check if any constituent type of the source union is assignable to the target type
        for t in self.ty(source).types_list() {
            if self.is_type_assignable_to(t, target) {
                return true;
            }
        }
        false
    }

    // Go: checker/flow.go:2451 getTypePredicateArgument
    pub fn get_type_predicate_argument(
        &mut self,
        predicate: TypePredicateId,
        call_expression: Node,
    ) -> Node {
        let kind = self.pred(predicate).kind;
        let parameter_index = self.pred(predicate).parameter_index;
        if kind == TypePredicateKind::IDENTIFIER || kind == TypePredicateKind::ASSERTS_IDENTIFIER {
            let arguments = call_expression.arguments();
            if parameter_index >= 0 && (parameter_index as usize) < arguments.len() {
                return arguments.get(parameter_index as usize);
            }
        } else {
            let invoked_expression = skip_parentheses(call_expression.expression());
            if is_access_expression(invoked_expression) {
                return skip_parentheses(invoked_expression.expression());
            }
        }
        Node::NIL
    }

    // Go: checker/flow.go:2466 getFlowTypeInConstructor
    pub fn get_flow_type_in_constructor(&mut self, symbol: SymbolId, constructor: Node) -> TypeId {
        let name = self.sym(symbol).name.clone();
        let access_name = if name.starts_with(&format!("{}#", INTERNAL_SYMBOL_NAME_PREFIX)) {
            // Go: symbol.Name[strings.Index(symbol.Name, "@")+1:]
            let start = name.find('@').map_or(0, |i| i + 1);
            self.factory.new_private_identifier(&name[start..])
        } else {
            self.factory.new_identifier(name)
        };
        let this_keyword = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
        let reference = self.factory.new_property_access_expression(
            this_keyword,
            Node::NIL,
            access_name,
            NodeFlags::NONE,
        );
        set_node_parent(reference.expression(), reference);
        set_node_parent(reference, constructor);
        set_node_flow_node(reference, constructor.return_flow_node());
        let flow_type = self.get_flow_type_of_property(reference, symbol);
        if self.no_implicit_any
            && (flow_type == self.auto_type || flow_type == self.auto_array_type)
        {
            let value_declaration = self.sym(symbol).value_declaration;
            let symbol_text = self.symbol_to_string(symbol);
            let type_text = self.type_to_string_exported(flow_type);
            self.error(
                value_declaration,
                diag::Member_0_implicitly_has_an_1_type,
                args![symbol_text, type_text],
            );
        }
        // We don't infer a type if assignments are only null or undefined.
        if self.every_type(flow_type, &mut |c: &mut Checker, t: TypeId| {
            c.is_nullable_type(t)
        }) {
            return TypeId::NIL;
        }
        self.convert_auto_to_any(flow_type)
    }

    // Go: checker/flow.go:2488 getFlowTypeInStaticBlocks
    pub fn get_flow_type_in_static_blocks(
        &mut self,
        symbol: SymbolId,
        static_blocks: &[Node],
    ) -> TypeId {
        let name = self.sym(symbol).name.clone();
        let access_name = if name.starts_with(&format!("{}#", INTERNAL_SYMBOL_NAME_PREFIX)) {
            // Go: symbol.Name[strings.Index(symbol.Name, "@")+1:]
            let start = name.find('@').map_or(0, |i| i + 1);
            self.factory.new_private_identifier(&name[start..])
        } else {
            self.factory.new_identifier(name)
        };
        for &static_block in static_blocks {
            let this_keyword = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
            let reference = self.factory.new_property_access_expression(
                this_keyword,
                Node::NIL,
                access_name,
                NodeFlags::NONE,
            );
            set_node_parent(reference.expression(), reference);
            set_node_parent(reference, static_block);
            set_node_flow_node(reference, static_block.return_flow_node());
            let flow_type = self.get_flow_type_of_property(reference, symbol);
            if self.no_implicit_any
                && (flow_type == self.auto_type || flow_type == self.auto_array_type)
            {
                let value_declaration = self.sym(symbol).value_declaration;
                let symbol_text = self.symbol_to_string(symbol);
                let type_text = self.type_to_string_exported(flow_type);
                self.error(
                    value_declaration,
                    diag::Member_0_implicitly_has_an_1_type,
                    args![symbol_text, type_text],
                );
            }
            // We don't infer a type if assignments are only null or undefined.
            if self.every_type(flow_type, &mut |c: &mut Checker, t: TypeId| {
                c.is_nullable_type(t)
            }) {
                continue;
            }
            return self.convert_auto_to_any(flow_type);
        }
        TypeId::NIL
    }

    // Go: checker/flow.go:2513 isReachableFlowNode
    pub fn is_reachable_flow_node(&mut self, flow: FlowNodeId) -> bool {
        // flowskip1 verify: an effect site (flow_skip.rs).
        self.flow_skip.effects += 1;
        let f = self.get_flow_state();
        let result = self.is_reachable_flow_node_worker(&f, flow, false /*noCacheCheck*/);
        self.put_flow_state(f);
        self.last_flow_node = flow;
        self.last_flow_node_reachable = result;
        result
    }

    // Go: checker/flow.go:2522 isReachableFlowNodeWorker
    pub fn is_reachable_flow_node_worker(
        &mut self,
        f: &Rc<RefCell<FlowState>>,
        flow: FlowNodeId,
        no_cache_check: bool,
    ) -> bool {
        let mut flow = flow;
        let mut no_cache_check = no_cache_check;
        loop {
            if flow == self.last_flow_node {
                return self.last_flow_node_reachable;
            }
            // PERF: lsshells M3b. No guard for a static file (`get_flow_in`).
            let mut flow_data_guard = None;
            let flow_data = flow.get_flow_in(&mut flow_data_guard);
            let flags = flow_data.flags;
            if flags.intersects(FlowFlags::SHARED) {
                if !no_cache_check && f.borrow().reduce_labels.is_empty() {
                    if let Some(&reachable) = self.flow_node_reachable.get(&flow) {
                        return reachable;
                    }
                    let reachable =
                        self.is_reachable_flow_node_worker(f, flow, true /*noCacheCheck*/);
                    self.flow_node_reachable.insert(flow, reachable);
                    return reachable;
                }
                no_cache_check = false;
            }
            if flags.intersects(
                FlowFlags::ASSIGNMENT | FlowFlags::CONDITION | FlowFlags::ARRAY_MUTATION,
            ) {
                flow = flow_data.antecedent;
            } else if flags.intersects(FlowFlags::CALL) {
                let signature = self.get_effects_signature(flow_data.node);
                if signature.is_some() {
                    let predicate = self.get_type_predicate_of_signature(signature);
                    if predicate.is_some()
                        && self.pred(predicate).kind == TypePredicateKind::ASSERTS_IDENTIFIER
                        && self.pred(predicate).t.is_nil()
                    {
                        let arguments = flow_data.node.arguments();
                        let parameter_index = self.pred(predicate).parameter_index;
                        if parameter_index >= 0
                            && (parameter_index as usize) < arguments.len()
                            && self.is_false_expression(arguments.get(parameter_index as usize))
                        {
                            return false;
                        }
                    }
                    let return_type = self.get_return_type_of_signature(signature);
                    if self.ty(return_type).flags.intersects(TypeFlags::NEVER) {
                        return false;
                    }
                }
                flow = flow_data.antecedent;
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                // A branching point is reachable if any branch is reachable.
                let antecedents =
                    get_branch_label_antecedents(flow, flow_data, &f.borrow().reduce_labels);
                for &antecedent in antecedents.iter() {
                    if self
                        .is_reachable_flow_node_worker(f, antecedent, false /*noCacheCheck*/)
                    {
                        return true;
                    }
                }
                return false;
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                if flow_data.antecedents.is_empty() {
                    return false;
                }
                // A loop is reachable if the control flow path that leads to the top is reachable.
                flow = flow_data.antecedents[0];
            } else if flags.intersects(FlowFlags::SWITCH_CLAUSE) {
                // The control flow path representing an unmatched value in a switch statement with
                // no default clause is unreachable if the switch statement is exhaustive.
                let data = flow_data.as_flow_switch_clause_data();
                if data.clause_start == data.clause_end
                    && self.is_exhaustive_switch_statement(data.switch_statement)
                {
                    return false;
                }
                flow = flow_data.antecedent;
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                // Cache is unreliable once we start adjusting labels
                self.last_flow_node = FlowNodeId::NIL;
                f.borrow_mut()
                    .reduce_labels
                    .push(ReduceLabel::of(flow_data));
                let result = self.is_reachable_flow_node_worker(
                    f,
                    flow_data.antecedent,
                    false, /*noCacheCheck*/
                );
                f.borrow_mut().reduce_labels.pop();
                return result;
            } else {
                return !flags.intersects(FlowFlags::UNREACHABLE);
            }
        }
    }

    // Go: checker/flow.go:2589 isFalseExpression
    pub fn is_false_expression(&mut self, expr: Node) -> bool {
        let node = skip_parentheses(expr);
        if node.kind() == SyntaxKind::FalseKeyword {
            return true;
        }
        if is_binary_expression(node) {
            let operator = node.operator_token().kind();
            return operator == SyntaxKind::AmpersandAmpersandToken
                && (self.is_false_expression(node.left())
                    || self.is_false_expression(node.right()))
                || operator == SyntaxKind::BarBarToken
                    && self.is_false_expression(node.left())
                    && self.is_false_expression(node.right());
        }
        false
    }

    // Return true if the given flow node is preceded by a 'super(...)' call in every possible code path
    // leading to the node.
    // Go: checker/flow.go:2604 isPostSuperFlowNode
    pub fn is_post_super_flow_node(&mut self, flow: FlowNodeId, no_cache_check: bool) -> bool {
        let f = self.get_flow_state();
        let result = self.is_post_super_flow_node_worker(&f, flow, no_cache_check);
        self.put_flow_state(f);
        result
    }

    // Go: checker/flow.go:2611 isPostSuperFlowNodeWorker
    pub fn is_post_super_flow_node_worker(
        &mut self,
        f: &Rc<RefCell<FlowState>>,
        flow: FlowNodeId,
        no_cache_check: bool,
    ) -> bool {
        let mut flow = flow;
        let mut no_cache_check = no_cache_check;
        loop {
            // PERF: lsshells M3b. No guard for a static file (`get_flow_in`).
            let mut flow_data_guard = None;
            let flow_data = flow.get_flow_in(&mut flow_data_guard);
            let flags = flow_data.flags;
            if flags.intersects(FlowFlags::SHARED) {
                if !no_cache_check {
                    if let Some(&post_super) = self.flow_node_post_super.get(&flow) {
                        return post_super;
                    }
                    // PORT: Go computes and caches the result here, then falls
                    // through and walks the node again; kept as is.
                    let post_super =
                        self.is_post_super_flow_node_worker(f, flow, true /*noCacheCheck*/);
                    self.flow_node_post_super.insert(flow, post_super);
                }
                no_cache_check = false;
            }
            if flags.intersects(
                FlowFlags::ASSIGNMENT
                    | FlowFlags::CONDITION
                    | FlowFlags::ARRAY_MUTATION
                    | FlowFlags::SWITCH_CLAUSE,
            ) {
                flow = flow_data.antecedent;
            } else if flags.intersects(FlowFlags::CALL) {
                if flow_data.node.expression().kind() == SyntaxKind::SuperKeyword {
                    return true;
                }
                flow = flow_data.antecedent;
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                let antecedents =
                    get_branch_label_antecedents(flow, flow_data, &f.borrow().reduce_labels);
                for &antecedent in antecedents.iter() {
                    if !self
                        .is_post_super_flow_node_worker(f, antecedent, false /*noCacheCheck*/)
                    {
                        return false;
                    }
                }
                return true;
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                // A loop is post-super if the control flow path that leads to the top is post-super.
                flow = flow_data.antecedents[0];
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                f.borrow_mut()
                    .reduce_labels
                    .push(ReduceLabel::of(flow_data));
                let result = self.is_post_super_flow_node_worker(
                    f,
                    flow_data.antecedent,
                    false, /*noCacheCheck*/
                );
                f.borrow_mut().reduce_labels.pop();
                return result;
            } else {
                // Unreachable nodes are considered post-super to silence errors
                return flags.intersects(FlowFlags::UNREACHABLE);
            }
        }
    }

    // Check if a parameter, catch variable, or mutable local variable is definitely assigned anywhere
    // Go: checker/flow.go:2655 isSymbolAssignedDefinitely
    pub fn is_symbol_assigned_definitely(&mut self, symbol: SymbolId) -> bool {
        self.ensure_assignments_marked(symbol);
        self.marked_assignment_symbol_links
            .get(symbol)
            .has_definite_assignment
    }

    // Check if a parameter, catch variable, or mutable local variable is assigned anywhere
    // Go: checker/flow.go:2661 isSymbolAssigned
    pub fn is_symbol_assigned(&mut self, symbol: SymbolId) -> bool {
        self.ensure_assignments_marked(symbol);
        self.marked_assignment_symbol_links
            .get(symbol)
            .last_assignment_pos
            != 0
    }

    // Return true if there are no assignments to the given symbol or if the given location
    // is past the last assignment to the symbol.
    // Go: checker/flow.go:2668 isPastLastAssignment
    pub fn is_past_last_assignment(&mut self, symbol: SymbolId, location: Node) -> bool {
        self.ensure_assignments_marked(symbol);
        let last_assignment_pos = self
            .marked_assignment_symbol_links
            .get(symbol)
            .last_assignment_pos;
        last_assignment_pos == 0 || location.is_some() && last_assignment_pos < location.pos()
    }

    // Go: checker/flow.go:2674 ensureAssignmentsMarked
    pub fn ensure_assignments_marked(&mut self, symbol: SymbolId) {
        let parent = find_ancestor(
            self.sym(symbol).value_declaration,
            is_function_or_source_file,
        );
        if parent.is_nil() {
            return;
        }
        let links = self.node_links.get(parent);
        if !links.flags.intersects(NodeCheckFlags::ASSIGNMENTS_MARKED) {
            links.flags |= NodeCheckFlags::ASSIGNMENTS_MARKED;
            // flowskip1 verify: an effect site (flow_skip.rs).
            self.flow_skip.effects += 1;
            if !self.has_parent_with_assignments_marked(parent) {
                let mark_node_assignments = self.mark_node_assignments.clone();
                mark_node_assignments(self, parent);
            }
        }
    }

    // Go: checker/flow.go:2688 hasParentWithAssignmentsMarked
    pub fn has_parent_with_assignments_marked(&mut self, node: Node) -> bool {
        find_ancestor(node.parent(), |node: Node| -> bool {
            is_function_or_source_file(node)
                && self
                    .node_links
                    .get(node)
                    .flags
                    .intersects(NodeCheckFlags::ASSIGNMENTS_MARKED)
        })
        .is_some()
    }

    // For all assignments within the given root node, record the last assignment source position for all
    // referenced parameters and mutable local variables. When assignments occur in nested functions  or
    // references occur in export specifiers, record math.MaxInt32 as the assignment position. When
    // assignments occur in compound statements, record the ending source position of the compound statement
    // as the assignment position (this is more conservative than full control flow analysis, but requires
    // only a single walk over the AST).
    // Go: checker/flow.go:2700 markNodeAssignmentsWorker
    // PERF: the kind is read once, and the walk calls this worker directly.
    // `c.markNodeAssignments` is always this worker (`checker_p02`), so the
    // `Rc` closure clone and call per node are not needed.
    pub fn mark_node_assignments_worker(&mut self, node: Node) -> bool {
        let kind = node.kind();
        match kind {
            SyntaxKind::Identifier => {
                let assignment_kind = get_assignment_target_kind(node);
                if assignment_kind != AssignmentKind::NONE {
                    let symbol = self.get_resolved_symbol(node);
                    if self.is_parameter_or_mutable_local_variable(symbol) {
                        let pos = self
                            .marked_assignment_symbol_links
                            .get(symbol)
                            .last_assignment_pos;
                        if pos == 0 || pos != i32::MAX {
                            let value_declaration = self.sym(symbol).value_declaration;
                            let referencing_function =
                                find_ancestor(node, is_function_or_source_file);
                            let declaring_function =
                                find_ancestor(value_declaration, is_function_or_source_file);
                            let last_assignment_pos = if referencing_function == declaring_function
                            {
                                self.extend_assignment_position(node, value_declaration)
                            } else {
                                i32::MAX
                            };
                            self.marked_assignment_symbol_links
                                .get(symbol)
                                .last_assignment_pos = last_assignment_pos;
                        }
                        if assignment_kind == AssignmentKind::DEFINITE {
                            self.marked_assignment_symbol_links
                                .get(symbol)
                                .has_definite_assignment = true;
                        }
                    }
                }
                return false;
            }
            SyntaxKind::ExportSpecifier => {
                let export_declaration = node.parent().parent();
                let name = node.property_name_or_name();
                if !node.is_type_only()
                    && !export_declaration.is_type_only()
                    && export_declaration.module_specifier().is_nil()
                    && !is_string_literal(name)
                {
                    let symbol = self.resolve_entity_name(
                        name,
                        SymbolFlags::VALUE,
                        true, /*ignoreErrors*/
                        true, /*dontResolveAlias*/
                        Node::NIL,
                    );
                    if symbol.is_some() && self.is_parameter_or_mutable_local_variable(symbol) {
                        let links = self.marked_assignment_symbol_links.get(symbol);
                        links.last_assignment_pos = i32::MAX;
                    }
                }
                return false;
            }
            SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::EnumDeclaration => {
                return false;
            }
            _ => {}
        }
        if is_type_node_kind(kind) {
            return false;
        }
        node.for_each_child(&mut |child: Node| -> bool { self.mark_node_assignments_worker(child) })
    }

    // Extend the position of the given assignment target node to the end of any intervening variable statement,
    // expression statement, compound statement, or class declaration occurring between the node and the given
    // declaration node.
    // Go: checker/flow.go:2749 extendAssignmentPosition
    pub fn extend_assignment_position(&mut self, node: Node, declaration: Node) -> i32 {
        let mut node = node;
        let mut pos = node.pos();
        while node.is_some() && node.pos() > declaration.pos() {
            match node.kind() {
                SyntaxKind::VariableStatement
                | SyntaxKind::ExpressionStatement
                | SyntaxKind::IfStatement
                | SyntaxKind::DoStatement
                | SyntaxKind::WhileStatement
                | SyntaxKind::ForStatement
                | SyntaxKind::ForInStatement
                | SyntaxKind::ForOfStatement
                | SyntaxKind::WithStatement
                | SyntaxKind::SwitchStatement
                | SyntaxKind::TryStatement
                | SyntaxKind::ClassDeclaration => {
                    pos = node.end();
                }
                _ => {}
            }
            node = node.parent();
        }
        pos
    }
}
