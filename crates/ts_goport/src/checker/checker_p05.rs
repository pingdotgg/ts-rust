//! Port of Go `checker/checker.go` lines 3874-4793 (checker-05):
//! statement checks (do/while/for/for-in/for-of/return/with/switch/labeled/
//! throw/try/catch) and class declaration checks (heritage, overrides,
//! index constraints).

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: Go `core.OrElse(node, fallback)` for node handles.
fn or_else_node(node: Node, fallback: Node) -> Node {
    if node.is_some() { node } else { fallback }
}

// PORT: Go `core.OrElse(t, fallback)` for type handles.
fn or_else_type(t: TypeId, fallback: TypeId) -> TypeId {
    if t.is_some() { t } else { fallback }
}

// PORT: the Go closure `visit` inside isSymbolUsedInBinaryExpressionChain.
fn visit_binary_expression_chain_child(
    c: &mut Checker,
    child: Node,
    tested_symbol: SymbolId,
) -> bool {
    if is_identifier(child) {
        let symbol = c.get_symbol_at_location(child, false);
        if symbol.is_some() && symbol == tested_symbol {
            return true;
        }
    }
    child.for_each_child(&mut |n: Node| visit_binary_expression_chain_child(c, n, tested_symbol))
}

// PORT: the Go closure `visit` inside isSymbolUsedInConditionBody.
fn visit_condition_body_child(
    c: &mut Checker,
    child_node: Node,
    expr: Node,
    tested_node: Node,
    tested_symbol: SymbolId,
) -> bool {
    if is_identifier(child_node) {
        let child_symbol = c.get_symbol_at_location(child_node, false);
        if child_symbol.is_some() && child_symbol == tested_symbol {
            // If the test was a simple identifier, the above check is sufficient
            if is_identifier(expr)
                || is_identifier(tested_node) && is_binary_expression(tested_node.parent())
            {
                return true;
            }
            // Otherwise we need to ensure the symbol is called on the same target
            let mut tested_expression = tested_node.parent();
            let mut child_expression = child_node.parent();
            while tested_expression.is_some() && child_expression.is_some() {
                if is_identifier(tested_expression) && is_identifier(child_expression)
                    || tested_expression.kind() == SyntaxKind::ThisKeyword
                        && child_expression.kind() == SyntaxKind::ThisKeyword
                {
                    let a = c.get_symbol_at_location(tested_expression, false);
                    let b = c.get_symbol_at_location(child_expression, false);
                    return a == b;
                } else if is_property_access_expression(tested_expression)
                    && is_property_access_expression(child_expression)
                {
                    let a = c.get_symbol_at_location(tested_expression.name(), false);
                    let b = c.get_symbol_at_location(child_expression.name(), false);
                    if a != b {
                        return false;
                    }
                    child_expression = child_expression.expression();
                    tested_expression = tested_expression.expression();
                } else if is_call_expression(tested_expression)
                    && is_call_expression(child_expression)
                {
                    child_expression = child_expression.expression();
                    tested_expression = tested_expression.expression();
                } else {
                    return false;
                }
            }
        }
    }
    child_node.for_each_child(&mut |n: Node| {
        visit_condition_body_child(c, n, expr, tested_node, tested_symbol)
    })
}

impl Checker {
    // Go: checker/checker.go:3918 isSymbolUsedInBinaryExpressionChain
    pub fn is_symbol_used_in_binary_expression_chain(
        &mut self,
        node: Node,
        tested_symbol: SymbolId,
    ) -> bool {
        let mut node = node;
        while is_binary_expression(node)
            && node.operator_token().kind() == SyntaxKind::AmpersandAmpersandToken
        {
            let right = node.right();
            let is_used = right.for_each_child(&mut |child: Node| {
                visit_binary_expression_chain_child(self, child, tested_symbol)
            });
            if is_used {
                return true;
            }
            node = node.parent();
        }
        false
    }

    // Go: checker/checker.go:3939 isSymbolUsedInConditionBody
    pub fn is_symbol_used_in_condition_body(
        &mut self,
        expr: Node,
        body: Node,
        tested_node: Node,
        tested_symbol: SymbolId,
    ) -> bool {
        body.for_each_child(&mut |child: Node| {
            visit_condition_body_child(self, child, expr, tested_node, tested_symbol)
        })
    }

    // Go: checker/checker.go:3975 checkDoStatement
    pub fn check_do_statement(&mut self, node: Node) {
        self.check_grammar_statement_in_ambient_context(node);
        self.check_source_element(node.statement());
        self.check_truthiness_expression(node.expression(), CheckMode::NORMAL);
    }

    // Go: checker/checker.go:3981 checkWhileStatement
    pub fn check_while_statement(&mut self, node: Node) {
        self.check_grammar_statement_in_ambient_context(node);
        self.check_truthiness_expression(node.expression(), CheckMode::NORMAL);
        self.check_source_element(node.statement());
    }

    // Go: checker/checker.go:3987 checkForStatement
    pub fn check_for_statement(&mut self, node: Node) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            let init = node.initializer();
            if init.is_some() && init.kind() == SyntaxKind::VariableDeclarationList {
                // PORT: Go passes `init.AsVariableDeclarationList()`; the Rust callee takes the node.
                self.check_grammar_variable_declaration_list(init);
            }
        }
        let initializer = node.initializer();
        if initializer.is_some() {
            if is_variable_declaration_list(initializer) {
                self.check_variable_declaration_list(initializer);
            } else {
                self.check_expression(initializer);
            }
        }
        let condition = node.condition();
        if condition.is_some() {
            self.check_truthiness_expression(condition, CheckMode::NORMAL);
        }
        let incrementor = node.incrementor();
        if incrementor.is_some() {
            self.check_expression(incrementor);
        }
        self.check_source_element(node.statement());
        if node.locals().is_some() {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:4013 checkForInStatement
    pub fn check_for_in_statement(&mut self, node: Node) {
        // PORT: Go passes `node.AsForInOrOfStatement()`; the Rust callee takes the node.
        self.check_grammar_for_in_or_for_of_statement(node);
        let expression = node.expression();
        let checked = self.check_expression(expression);
        let right_type = self.get_non_nullable_type_if_needed(checked);
        // TypeScript 1.0 spec (April 2014): 5.4
        // In a 'for-in' statement of the form
        // for (let VarDecl in Expr) Statement
        //   VarDecl must be a variable declaration without a type annotation that declares a variable of type Any,
        //   and Expr must be an expression of type Any, an object type, or a type parameter type.
        let initializer = node.initializer();
        if is_variable_declaration_list(initializer) {
            let declarations = initializer.declarations().nodes().to_vec();
            if !declarations.is_empty() && is_binding_pattern(declarations[0].name()) {
                self.error(
                    declarations[0].name(),
                    diag::The_left_hand_side_of_a_for_in_statement_cannot_be_a_destructuring_pattern,
                    args![],
                );
            }
            self.check_variable_declaration_list(initializer);
        } else {
            // In a 'for-in' statement of the form
            // for (Var in Expr) Statement
            //   Var must be an expression classified as a reference of type Any or the String primitive type,
            //   and Expr must be an expression of type Any, an object type, or a type parameter type.
            let var_expr = initializer;
            let left_type = self.check_expression(var_expr);
            if is_array_literal_expression(var_expr) || is_object_literal_expression(var_expr) {
                self.error(
                    var_expr,
                    diag::The_left_hand_side_of_a_for_in_statement_cannot_be_a_destructuring_pattern,
                    args![],
                );
            } else {
                let index_type = self.get_index_type_or_string(right_type);
                if !self.is_type_assignable_to(index_type, left_type) {
                    self.error(
                        var_expr,
                        diag::The_left_hand_side_of_a_for_in_statement_must_be_of_type_string_or_any,
                        args![],
                    );
                } else {
                    // run check only former check succeeded to avoid cascading errors
                    self.check_reference_expression(
                        var_expr,
                        diag::The_left_hand_side_of_a_for_in_statement_must_be_a_variable_or_a_property_access,
                        diag::The_left_hand_side_of_a_for_in_statement_may_not_be_an_optional_property_access,
                    );
                }
            }
        }
        // unknownType is returned i.e. if node.expression is identifier whose name cannot be resolved
        // in this case error about missing name is already reported - do not report extra one
        if right_type == self.never_type
            || !self.is_type_assignable_to_kind(
                right_type,
                TypeFlags::NON_PRIMITIVE | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
            )
        {
            let type_string = self.type_to_string_exported(right_type);
            self.error(
                expression,
                diag::The_right_hand_side_of_a_for_in_statement_must_be_of_type_any_an_object_type_or_a_type_parameter_but_here_has_type_0,
                args![type_string],
            );
        }
        self.check_source_element(node.statement());
        if node.locals().is_some() {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:4055 getIndexTypeOrString
    pub fn get_index_type_or_string(&mut self, t: TypeId) -> TypeId {
        let index = self.get_index_type(t);
        let index_type = self.get_extract_string_type(index);
        if self.ty(index_type).flags.intersects(TypeFlags::NEVER) {
            self.string_type
        } else {
            index_type
        }
    }

    // Go: checker/checker.go:4060 checkForOfStatement
    pub fn check_for_of_statement(&mut self, node: Node) {
        // PORT: Go passes `node.AsForInOrOfStatement()`; the Rust callee takes the node.
        self.check_grammar_for_in_or_for_of_statement(node);
        let container = get_containing_function_or_class_static_block(node);
        let await_modifier = node.await_modifier();
        if await_modifier.is_some() {
            if container.is_some() && is_class_static_block_declaration(container) {
                self.grammar_error_on_node(
                    await_modifier,
                    diag::X_for_await_loops_cannot_be_used_inside_a_class_static_block,
                    args![],
                );
            } else {
                let function_flags = get_function_flags(container);
                if (function_flags & (FunctionFlags::INVALID | FunctionFlags::ASYNC))
                    == FunctionFlags::ASYNC
                    && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.for_await_of
                {
                    // for..await..of in an async function or async generator function prior to ESNext requires the __asyncValues helper
                    self.check_external_emit_helpers(
                        node,
                        ExternalEmitHelpers::FOR_AWAIT_OF_INCLUDES,
                    );
                }
            }
        } // Check the LHS and RHS
        // If the LHS is a declaration, just check it as a variable declaration, which will in turn check the RHS
        // via checkRightHandSideOfForOf.
        // If the LHS is an expression, check the LHS, as a destructuring assignment or as a reference.
        // Then check that the RHS is assignable to it.
        let initializer = node.initializer();
        if is_variable_declaration_list(initializer) {
            self.check_variable_declaration_list(initializer);
        } else {
            let var_expr = initializer;
            let iterated_type = self.check_right_hand_side_of_for_of(node);
            // There may be a destructuring assignment on the left side
            if is_array_literal_expression(var_expr) || is_object_literal_expression(var_expr) {
                // iteratedType may be undefined. In this case, we still want to check the structure of
                // varExpr, in particular making sure it's a valid LeftHandSideExpression. But we'd like
                // to short circuit the type relation checking as much as possible, so we pass the unknownType.
                let source_type = or_else_type(iterated_type, self.error_type);
                self.check_destructuring_assignment(
                    var_expr,
                    source_type,
                    CheckMode::NORMAL,
                    false,
                );
            } else {
                let left_type = self.check_expression(var_expr);
                self.check_reference_expression(
                    var_expr,
                    diag::The_left_hand_side_of_a_for_of_statement_must_be_a_variable_or_a_property_access,
                    diag::The_left_hand_side_of_a_for_of_statement_may_not_be_an_optional_property_access,
                );
                // iteratedType will be undefined if the rightType was missing properties/signatures
                // required to get its iteratedType (like [Symbol.iterator] or next). This may be
                // because we accessed properties from anyType, or it may have led to an error inside
                // getElementTypeOfIterable.
                if iterated_type.is_some() {
                    self.check_type_assignable_to_and_optionally_elaborate(
                        iterated_type,
                        left_type,
                        var_expr,
                        node.expression(),
                        None,
                        None,
                    );
                }
            }
        }
        self.check_source_element(node.statement());
        if node.locals().is_some() {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:4108 checkBreakOrContinueStatement
    pub fn check_break_or_continue_statement(&mut self, node: Node) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            self.check_grammar_break_or_continue_statement(node);
        }
    }

    // Go: checker/checker.go:4114 checkReturnStatement
    pub fn check_return_statement(&mut self, node: Node) {
        // Always check the return expression so its identifiers are resolved even when the
        // return statement is misplaced (grammar error), keeping diagnostics stable
        // regardless of traversal order.
        let expr_node = node.expression();
        let mut expr_type = self.undefined_type;
        if expr_node.is_some() {
            expr_type = self.check_expression_cached(expr_node);
        }
        if self.check_grammar_statement_in_ambient_context(node) {
            return;
        }
        let container = get_containing_function_or_class_static_block(node);
        if container.is_some() && is_class_static_block_declaration(container) {
            self.grammar_error_on_first_token(
                node,
                diag::A_return_statement_cannot_be_used_inside_a_class_static_block,
                args![],
            );
            return;
        }
        if container.is_nil() {
            self.grammar_error_on_first_token(
                node,
                diag::A_return_statement_can_only_be_used_within_a_function_body,
                args![],
            );
            return;
        }
        let signature = self.get_signature_from_declaration(container);
        let return_type = self.get_return_type_of_signature(signature);
        let function_flags = get_function_flags(container);
        if self.strict_null_checks
            || expr_node.is_some()
            || self.ty(return_type).flags.intersects(TypeFlags::NEVER)
        {
            if is_set_accessor_declaration(container) {
                if expr_node.is_some() {
                    self.error(node, diag::Setters_cannot_return_a_value, args![]);
                }
            } else if is_constructor_declaration(container) {
                if expr_node.is_some()
                    && !self.check_type_assignable_to_and_optionally_elaborate(
                        expr_type,
                        return_type,
                        node,
                        expr_node,
                        None,
                        None,
                    )
                {
                    self.error(
                        node,
                        diag::Return_type_of_constructor_signature_must_be_assignable_to_the_instance_type_of_the_class,
                        args![],
                    );
                }
            } else if self.get_return_type_from_annotation(container).is_some() {
                let unwrapped = self.unwrap_return_type(return_type, function_flags);
                let unwrapped_return_type = or_else_type(unwrapped, return_type);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    node.expression(),
                    expr_type,
                    false,
                );
            }
        } else if !is_constructor_declaration(container)
            && self.compiler_options.no_implicit_returns.is_true()
            && !self.is_unwrapped_return_type_undefined_void_or_any(container, return_type)
        {
            // The function has a return type, but the return statement doesn't have an expression.
            self.error(node, diag::Not_all_code_paths_return_a_value, args![]);
        }
    }

    // When checking an arrow expression such as `(x) => exp`, then `node` is the expression `exp`.
    // Otherwise, `node` is a return statement.
    // Go: checker/checker.go:4159 checkReturnExpression
    pub fn check_return_expression(
        &mut self,
        container: Node,
        unwrapped_return_type: TypeId,
        node: Node,
        expr: Node,
        expr_type: TypeId,
        in_conditional_expression: bool,
    ) {
        let mut unwrapped_expr_type = expr_type;
        let function_flags = get_function_flags(container);
        if expr.is_some() {
            let unwrapped_expr = skip_parentheses(expr);
            if is_conditional_expression(unwrapped_expr) {
                let when_true = unwrapped_expr.when_true();
                let when_false = unwrapped_expr.when_false();
                let when_true_type = self.check_expression(when_true);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    when_true,
                    when_true_type,
                    true, /*inConditionalExpression*/
                );
                let when_false_type = self.check_expression(when_false);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    when_false,
                    when_false_type,
                    true, /*inConditionalExpression*/
                );
                return;
            }
        }
        let in_return_statement = node.kind() == SyntaxKind::ReturnStatement;
        if function_flags.intersects(FunctionFlags::ASYNC) {
            unwrapped_expr_type = self.check_awaited_type(
                expr_type,
                false, /*withAlias*/
                node,
                diag::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member,
            );
        }
        let mut effective_expr = expr; // The effective expression for diagnostics purposes.
        if expr.is_some() {
            effective_expr = self.get_effective_check_node(expr);
        }
        let error_node = if in_return_statement && !in_conditional_expression {
            node
        } else {
            effective_expr
        };
        self.check_type_assignable_to_and_optionally_elaborate(
            unwrapped_expr_type,
            unwrapped_return_type,
            error_node,
            effective_expr,
            None,
            None,
        );
    }

    // Go: checker/checker.go:4184 checkWithStatement
    pub fn check_with_statement(&mut self, node: Node) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            if node.flags().intersects(NodeFlags::AWAIT_CONTEXT) {
                self.grammar_error_on_first_token(
                    node,
                    diag::X_with_statements_are_not_allowed_in_an_async_function_block,
                    args![],
                );
            }
        }
        self.check_expression(node.expression());
        let source_file = get_source_file_of_node(node);
        if !self.has_parse_diagnostics(source_file) {
            let start = skip_trivia(&source_file_text(source_file), node.pos());
            let end = node.statement().pos();
            self.grammar_error_at_pos(
                source_file,
                start,
                end - start,
                diag::The_with_statement_is_not_supported_All_symbols_in_a_with_block_will_have_type_any,
                args![],
            );
        }
    }

    // Go: checker/checker.go:4199 checkSwitchStatement
    pub fn check_switch_statement(&mut self, node: Node) {
        // Grammar checking
        self.check_grammar_statement_in_ambient_context(node);
        let mut first_default_clause = Node::NIL;
        let mut has_duplicate_default_clause = false;
        let expression_type = self.check_expression(node.expression());
        let case_block = node.case_block();
        for clause in case_block.clauses().nodes() {
            // Grammar check for duplicate default clauses, skip if we already report duplicate default clause
            if is_default_clause(clause) && !has_duplicate_default_clause {
                if first_default_clause.is_nil() {
                    first_default_clause = clause;
                } else {
                    self.grammar_error_on_node(
                        clause,
                        diag::A_default_clause_cannot_appear_more_than_once_in_a_switch_statement,
                        args![],
                    );
                    has_duplicate_default_clause = true;
                }
            }
            if is_case_clause(clause) {
                let case_type = self.check_expression(clause.expression());
                if !self.is_type_equality_comparable_to(expression_type, case_type) {
                    // expressionType is not comparable to caseType, try the reversed check and report errors if it fails
                    self.check_type_comparable_to(
                        case_type,
                        expression_type,
                        clause.expression(),
                        None, /*headMessage*/
                    );
                }
            }
            self.check_source_elements(clause.statements());
            if self
                .compiler_options
                .no_fallthrough_cases_in_switch
                .is_true()
            {
                let flow_node = clause.fallthrough_flow_node();
                if flow_node.is_some() && self.is_reachable_flow_node(flow_node) {
                    self.error(clause, diag::Fallthrough_case_in_switch, args![]);
                }
            }
        }
        if case_block.locals().is_some() {
            self.register_for_unused_identifiers_check(case_block);
        }
    }

    // Go: checker/checker.go:4235 checkLabeledStatement
    pub fn check_labeled_statement(&mut self, node: Node) {
        let label_node = node.label();
        let label_text = label_node.text();
        if !self.check_grammar_statement_in_ambient_context(node) {
            let mut current = node.parent();
            while current.is_some() && !is_function_like(current) {
                if is_labeled_statement(current) && current.label().text() == label_text {
                    self.grammar_error_on_node(
                        label_node,
                        diag::Duplicate_label_0,
                        args![label_text],
                    );
                    break;
                }
                current = current.parent();
            }
        }
        if label_node.flags().intersects(NodeFlags::UNREACHABLE)
            && self.compiler_options.allow_unused_labels != Tristate::True
        {
            let is_error = self.compiler_options.allow_unused_labels == Tristate::False;
            self.error_or_suggestion(is_error, label_node, diag::Unused_label, args![]);
        }
        self.check_source_element(node.statement());
    }

    // Go: checker/checker.go:4253 checkThrowStatement
    pub fn check_throw_statement(&mut self, node: Node) {
        let throw_expr = node.expression();
        if !self.check_grammar_statement_in_ambient_context(node) {
            if is_identifier(throw_expr) && throw_expr.text().is_empty() {
                self.grammar_error_at_pos(
                    node,
                    throw_expr.pos(),
                    0, /*length*/
                    diag::Line_break_not_permitted_here,
                    args![],
                );
            }
        }
        self.check_expression(throw_expr);
    }

    // Go: checker/checker.go:4263 checkTryStatement
    pub fn check_try_statement(&mut self, node: Node) {
        self.check_grammar_statement_in_ambient_context(node);
        self.check_block(node.try_block());
        let catch_clause = node.catch_clause();
        if catch_clause.is_some() {
            self.check_catch_clause(catch_clause);
        }
        let finally_block = node.finally_block();
        if finally_block.is_some() {
            self.check_block(finally_block);
        }
    }

    // Go: checker/checker.go:4275 checkCatchClause
    pub fn check_catch_clause(&mut self, node: Node) {
        let declaration = node.variable_declaration();
        if declaration.is_some() {
            self.check_variable_like_declaration(declaration);
            let type_node = declaration.type_();
            if type_node.is_some() {
                let t = self.get_type_from_type_node(type_node);
                if t.is_some() && !self.ty(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                    self.grammar_error_on_first_token(
                        type_node,
                        diag::Catch_clause_variable_type_annotation_must_be_any_or_unknown_if_specified,
                        args![],
                    );
                }
            } else if declaration.initializer().is_some() {
                self.grammar_error_on_first_token(
                    declaration.initializer(),
                    diag::Catch_clause_variable_cannot_have_an_initializer,
                    args![],
                );
            } else {
                let block_locals = node.block().locals();
                if block_locals.is_some() {
                    // PORT: Go ranges over a map (random order); we use insertion order.
                    for (caught_name, _) in self.symbols.entries(node.locals()) {
                        let block_local = self.symbols.get(block_locals, &caught_name);
                        if block_local.is_some() {
                            let value_declaration = self.sym(block_local).value_declaration;
                            let flags = self.sym(block_local).flags;
                            if value_declaration.is_some()
                                && flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
                            {
                                self.grammar_error_on_node(
                                    value_declaration,
                                    diag::Cannot_redeclare_identifier_0_in_catch_clause,
                                    args![caught_name],
                                );
                            }
                        }
                    }
                }
            }
        }
        self.check_block(node.block());
    }

    // Go: checker/checker.go:4301 checkBindingElement
    pub fn check_binding_element(&mut self, node: Node) {
        // PORT: Go passes `node.AsBindingElement()`; the Rust callee takes the node.
        self.check_grammar_binding_element(node);
        self.check_variable_like_declaration(node);
    }

    // Go: checker/checker.go:4306 checkClassDeclaration
    pub fn check_class_declaration(&mut self, node: Node) {
        let first_decorator = node
            .modifier_nodes()
            .iter()
            .find(|&n| is_decorator(n))
            .unwrap_or(Node::NIL);
        if self.legacy_decorators
            && first_decorator.is_some()
            && node.members().iter().any(|p| {
                has_static_modifier(p) && is_private_identifier_class_element_declaration(p)
            })
        {
            self.grammar_error_on_node(
                first_decorator,
                diag::Class_decorators_can_t_be_used_with_static_private_identifier_Consider_removing_the_experimental_decorator,
                args![],
            );
        }
        if node.name().is_nil() && !has_syntactic_modifier(node, ModifierFlags::DEFAULT) {
            self.grammar_error_on_first_token(
                node,
                diag::A_class_declaration_without_the_default_modifier_must_have_a_name,
                args![],
            );
        }
        self.check_class_like_declaration(node);
        self.check_source_elements(node.members());
        // ts#64646, Go N' checker.go:4321
        self.check_constructor_declared_properties(node);
        self.register_for_unused_identifiers_check(node);
    }

    // Go: checker/checker.go:4325 checkConstructorDeclaredProperties (ts#64646, Go N')
    // Gets the type of each JS property that the class constructor declares,
    // so the diagnostics of its type do not depend on what is checked first.
    pub fn check_constructor_declared_properties(&mut self, node: Node) {
        if !is_in_js_file(node) {
            return;
        }
        let symbol = self.get_symbol_of_declaration(node);
        let class_type = self.get_declared_type_of_symbol(symbol);
        let properties = self.get_properties_of_type(class_type);
        for property in properties.iter().copied() {
            let (kind, constructor) = self.is_constructor_declared_this_property(property);
            if kind == ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_CONSTRUCTOR
                && constructor.parent() == node
            {
                self.get_type_of_symbol(property);
            }
        }
    }

    // Go: checker/checker.go:4321 checkClassLikeDeclaration
    pub fn check_class_like_declaration(&mut self, node: Node) {
        self.check_grammar_class_like_declaration(node);
        self.check_decorators(node);
        self.check_collisions_for_declaration_name(node, node.name());
        self.check_type_parameters(node.type_parameters());
        self.check_exports_on_merged_declarations(node);
        let symbol = self.get_symbol_of_declaration(node);
        let class_type = self.get_declared_type_of_symbol(symbol);
        let type_with_this = self.get_type_with_this_argument(class_type, TypeId::NIL, false);
        let static_type = self.get_type_of_symbol(symbol);
        self.check_type_parameter_lists_identical(symbol);
        self.check_function_or_constructor_symbol(symbol);
        self.check_object_type_for_duplicate_declarations(node, true /*checkPrivateNames*/);

        // Only check for reserved static identifiers on non-ambient context.
        let node_in_ambient_context = node.flags().intersects(NodeFlags::AMBIENT);
        if !node_in_ambient_context {
            self.check_class_for_static_property_name_conflicts(node);
        }

        let base_type_node = get_class_extends_heritage_element(node);
        if base_type_node.is_some() {
            self.check_source_elements(base_type_node.type_arguments());
            let base_types = self.get_base_types(class_type);
            if !base_types.is_empty() {
                let base_type = base_types[0];
                self.check_js_doc_augments_tag_matches_extends(node, base_type_node, base_type);
                let base_constructor_type = self.get_base_constructor_type_of_class(class_type);
                let static_base_type = self.get_apparent_type(base_constructor_type);
                self.check_base_type_accessibility(static_base_type, base_type_node);
                self.check_source_element(base_type_node.expression());
                let type_argument_nodes = base_type_node.type_arguments().to_vec();
                if !type_argument_nodes.is_empty() {
                    self.check_source_elements(type_argument_nodes.iter().copied());
                    for constructor in self.get_constructors_for_type_arguments(
                        static_base_type,
                        &type_argument_nodes,
                        base_type_node,
                    ) {
                        let type_parameters = self.sig(constructor).type_parameters.clone();
                        if !self.check_type_argument_constraints(base_type_node, &type_parameters) {
                            break;
                        }
                    }
                }
                let class_this_type = self.ty(class_type).as_interface_type().this_type;
                let base_with_this =
                    self.get_type_with_this_argument(base_type, class_this_type, false);
                if !self.check_type_assignable_to(type_with_this, base_with_this, Node::NIL, None) {
                    self.issue_member_specific_error(
                        node,
                        type_with_this,
                        base_with_this,
                        diag::Class_0_incorrectly_extends_base_class_1,
                    );
                } else {
                    // Report static side error only when instance type is assignable
                    let without_signatures = self.get_type_without_signatures(static_base_type);
                    self.check_type_assignable_to(
                        static_type,
                        without_signatures,
                        or_else_node(node.name(), node),
                        Some(
                            diag::Class_static_side_0_incorrectly_extends_base_class_static_side_1,
                        ),
                    );
                }
                if self
                    .ty(base_constructor_type)
                    .flags
                    .intersects(TypeFlags::TYPE_VARIABLE)
                {
                    if !self.is_mixin_constructor_type(static_type) {
                        self.error(
                            or_else_node(node.name(), node),
                            diag::A_mixin_class_must_have_a_constructor_with_a_single_rest_parameter_of_type_any,
                            args![],
                        );
                    } else {
                        let construct_signatures = self.get_signatures_of_type(
                            base_constructor_type,
                            SignatureKind::CONSTRUCT,
                        );
                        if construct_signatures.iter().any(|&signature| {
                            self.sig(signature)
                                .flags
                                .intersects(SignatureFlags::ABSTRACT)
                        }) && !has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
                        {
                            self.error(
                                or_else_node(node.name(), node),
                                diag::A_mixin_class_that_extends_from_a_type_variable_containing_an_abstract_construct_signature_must_also_be_declared_abstract,
                                args![],
                            );
                        }
                    }
                }
                let static_base_symbol = self.ty(static_base_type).symbol;
                if !(static_base_symbol.is_some()
                    && self
                        .sym(static_base_symbol)
                        .flags
                        .intersects(SymbolFlags::CLASS))
                    && !self
                        .ty(base_constructor_type)
                        .flags
                        .intersects(TypeFlags::TYPE_VARIABLE)
                {
                    // When the static base type is a "class-like" constructor function (but not actually a class), we verify
                    // that all instantiated base constructor signatures return the same type.
                    let constructors = self.get_instantiated_constructors_for_type_arguments(
                        static_base_type,
                        &base_type_node.type_arguments().to_vec(),
                        base_type_node,
                    );
                    let mut all_same = true;
                    for sig in constructors {
                        let return_type = self.get_return_type_of_signature(sig);
                        if !self.is_type_identical_to(return_type, base_type) {
                            all_same = false;
                            break;
                        }
                    }
                    if !all_same {
                        self.error(
                            base_type_node.expression(),
                            diag::Base_constructors_must_all_have_the_same_return_type,
                            args![],
                        );
                    }
                }
                self.check_kinds_of_property_member_overrides(class_type, base_type);
            }
        }
        self.check_members_for_override_modifier(node, class_type, type_with_this, static_type);
        let implemented_type_nodes = get_implements_heritage_clause_elements(node);
        for type_ref_node in implemented_type_nodes {
            if is_expression_with_type_arguments(type_ref_node) {
                let expr = type_ref_node.expression();
                if !is_entity_name_expression(expr) || is_optional_chain(expr) {
                    self.error(
                        expr,
                        diag::A_class_can_only_implement_an_identifier_Slashqualified_name_with_optional_type_arguments,
                        args![],
                    );
                }
            }
            self.check_type_reference_node(type_ref_node);
            let from_node = self.get_type_from_type_node(type_ref_node);
            let t = self.get_reduced_type(from_node);
            if !self.is_error_type(t) {
                if self.is_valid_base_type(t) {
                    let t_symbol = self.ty(t).symbol;
                    let generic_diag = if t_symbol.is_some()
                        && self.sym(t_symbol).flags.intersects(SymbolFlags::CLASS)
                    {
                        diag::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass
                    } else {
                        diag::Class_0_incorrectly_implements_interface_1
                    };
                    let class_this_type = self.ty(class_type).as_interface_type().this_type;
                    let base_with_this =
                        self.get_type_with_this_argument(t, class_this_type, false);
                    if !self.check_type_assignable_to(
                        type_with_this,
                        base_with_this,
                        Node::NIL,
                        None,
                    ) {
                        self.issue_member_specific_error(
                            node,
                            type_with_this,
                            base_with_this,
                            generic_diag,
                        );
                    }
                } else {
                    self.error(
                        type_ref_node,
                        diag::A_class_can_only_implement_an_object_type_or_intersection_of_object_types_with_statically_known_members,
                        args![],
                    );
                }
            }
        }
        self.check_index_constraints(class_type, symbol, false /*isStaticIndex*/);
        self.check_index_constraints(static_type, symbol, true /*isStaticIndex*/);
        self.check_class_or_interface_for_duplicate_index_signatures(node);
        self.check_property_initialization(node);
    }

    // Go: checker/checker.go:4424 checkJSDocAugmentsTagMatchesExtends
    pub fn check_js_doc_augments_tag_matches_extends(
        &mut self,
        node: Node,
        base_type_node: Node,
        base_type: TypeId,
    ) {
        if !is_in_js_file(node) {
            return;
        }
        let file = get_source_file_of_node(node);
        for j in node.eager_js_doc(file).to_vec() {
            let tags = j.tags();
            if tags.is_nil() {
                continue;
            }
            for tag in tags.nodes().to_vec() {
                if tag.kind() != SyntaxKind::JsDocAugmentsTag {
                    continue;
                }
                let source_type_node = tag.class_name();
                let source_type = self.get_type_from_type_node(source_type_node);
                if self.is_type_identical_to(source_type, base_type) {
                    continue;
                }
                let target_name =
                    get_identifier_from_entity_name_expression(base_type_node.expression());
                let source_name =
                    get_identifier_from_entity_name_expression(source_type_node.expression());
                if target_name.is_some() && source_name.is_some() {
                    self.error(
                        source_name,
                        diag::JSDoc_0_1_does_not_match_the_extends_2_clause,
                        args![
                            tag.tag_name().text(),
                            source_name.text(),
                            target_name.text()
                        ],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:4450 checkClassForStaticPropertyNameConflicts
    pub fn check_class_for_static_property_name_conflicts(&mut self, node: Node) {
        if self.compiler_options.get_use_define_for_class_fields() {
            return;
        }
        for member in node.members() {
            let member_name_node = member.name();
            let is_static_member = is_static(member);
            if is_static_member && member_name_node.is_some() {
                let (member_name, _) =
                    self.get_effective_property_name_for_property_name_node(member_name_node);
                match &*member_name {
                    "name" | "length" | "caller" | "arguments" => {
                        let class_symbol = self.get_symbol_of_declaration(node);
                        let class_name = self.symbol_to_string(class_symbol);
                        self.error(
                            member_name_node,
                            diag::Static_property_0_conflicts_with_built_in_property_Function_0_of_constructor_function_1,
                            args![member_name, class_name],
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    // Check that type parameter lists are identical across multiple declarations
    // Go: checker/checker.go:4473 checkTypeParameterListsIdentical
    pub fn check_type_parameter_lists_identical(&mut self, symbol: SymbolId) {
        if self.sym(symbol).declarations.len() == 1 {
            return;
        }
        let links = self.declared_type_links.get(symbol);
        if !links.type_parameters_checked {
            links.type_parameters_checked = true;
            let declarations = self.get_class_or_interface_declarations_of_symbol(symbol);
            if declarations.len() <= 1 {
                return;
            }
            let t = self.get_declared_type_of_symbol(symbol);
            let local_type_parameters = self
                .ty(t)
                .as_interface_type()
                .local_type_parameters()
                .to_vec();
            if !self.are_type_parameters_identical(
                &declarations,
                &local_type_parameters,
                &|n: Node| n.type_parameters().to_vec(),
            ) {
                // Report an error on every conflicting declaration.
                let name = self.symbol_to_string(symbol);
                for declaration in declarations {
                    self.error(
                        declaration.name(),
                        diag::All_declarations_of_0_must_have_identical_type_parameters,
                        args![name],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:4495 getClassOrInterfaceDeclarationsOfSymbol
    pub fn get_class_or_interface_declarations_of_symbol(&self, symbol: SymbolId) -> Vec<Node> {
        self.sym(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|&d| is_class_declaration(d) || is_interface_declaration(d))
            .collect()
    }

    // Go: checker/checker.go:4501 areTypeParametersIdentical
    // PORT: Go `getTypeParameterDeclarations func(node *ast.Node) []*ast.Node`
    // is `&dyn Fn(Node) -> Vec<Node>`.
    pub fn are_type_parameters_identical(
        &mut self,
        declarations: &[Node],
        target_parameters: &[TypeId],
        get_type_parameter_declarations: &dyn Fn(Node) -> Vec<Node>,
    ) -> bool {
        let max_type_argument_count = target_parameters.len();
        let min_type_argument_count = self.get_min_type_argument_count(target_parameters);
        for &declaration in declarations {
            // If this declaration has too few or too many type parameters, we report an error
            let source_parameters = get_type_parameter_declarations(declaration);
            if (source_parameters.len() as i32) < min_type_argument_count
                || source_parameters.len() > max_type_argument_count
            {
                return false;
            }
            for (i, &source) in source_parameters.iter().enumerate() {
                let target = target_parameters[i];
                // If the type parameter node does not have the same name as the resolved type
                // parameter at this position, we report an error.
                let target_symbol = self.ty(target).symbol;
                if source.name().text() != self.sym(target_symbol).name {
                    return false;
                }
                // If the type parameter node does not have an identical constraintNode as the resolved
                // type parameter at this position, we report an error.
                let constraint_node = source.constraint();
                let target_constraint = self.get_constraint_of_type_parameter(target);
                // relax check if later interface augmentation has no constraint, it's more broad and is OK to merge with
                // a more constrained interface (this could be generalized to a full hierarchy check, but that's maybe overkill)
                if constraint_node.is_some() && target_constraint.is_some() {
                    let constraint_type = self.get_type_from_type_node(constraint_node);
                    if !self.is_type_identical_to(constraint_type, target_constraint) {
                        return false;
                    }
                }
                // If the type parameter node has a default and it is not identical to the default
                // for the type parameter at this position, we report an error.
                let default_node = source.default_type();
                let target_default = self.get_default_from_type_parameter(target);
                if default_node.is_some() && target_default.is_some() {
                    let default_type = self.get_type_from_type_node(default_node);
                    if !self.is_type_identical_to(default_type, target_default) {
                        return false;
                    }
                }
            }
        }
        true
    }

    // Go: checker/checker.go:4538 checkBaseTypeAccessibility
    pub fn check_base_type_accessibility(&mut self, t: TypeId, node: Node) {
        let signatures = self.get_signatures_of_type(t, SignatureKind::CONSTRUCT);
        let accessibility_error =
            self.get_constructor_accessibility_error(node, &signatures, ModifierFlags::PRIVATE);
        if let Some(accessibility_error) = accessibility_error {
            let declaring_class_symbol = self.ty(accessibility_error.declaring_class).symbol;
            let name = self.get_fully_qualified_name(declaring_class_symbol, Node::NIL);
            self.error(
                node,
                diag::Cannot_extend_a_class_0_Class_constructor_is_marked_as_private,
                args![name],
            );
        }
    }

    // Go: checker/checker.go:4546 issueMemberSpecificError
    pub fn issue_member_specific_error(
        &mut self,
        node: Node,
        type_with_this: TypeId,
        base_with_this: TypeId,
        broad_diag: &'static Message,
    ) {
        // iterate over all implemented properties and issue errors on each one which isn't compatible, rather than the class as a whole, if possible
        let mut issued_member_error = false;
        for member in node.members() {
            if is_static(member) {
                continue;
            }
            let declared_prop = self.get_symbol_of_declaration(member);
            if declared_prop.is_some()
                && self.sym(declared_prop).name != INTERNAL_SYMBOL_NAME_COMPUTED
            {
                let declared_name = self.sym(declared_prop).name.clone();
                let prop = self.get_property_of_type(type_with_this, &declared_name);
                let base_prop = self.get_property_of_type(base_with_this, &declared_name);
                if prop.is_some() && base_prop.is_some() {
                    let mut diags: Vec<Diagnostic> = Vec::new();
                    let prop_type = self.get_type_of_symbol(prop);
                    let base_prop_type = self.get_type_of_symbol(base_prop);
                    if !self.check_type_assignable_to_ex(
                        prop_type,
                        base_prop_type,
                        or_else_node(member.name(), member),
                        None, /*headMessage*/
                        Some(&mut diags),
                    ) {
                        let prop_name = self.symbol_to_string(declared_prop);
                        let type_name = self.type_to_string_exported(type_with_this);
                        let base_name = self.type_to_string_exported(base_with_this);
                        let diagnostic = new_diagnostic_chain(
                            Some(diags[0].clone()),
                            diag::Property_0_in_type_1_is_not_assignable_to_the_same_property_in_base_type_2,
                            args![prop_name, type_name, base_name],
                        );
                        self.add_diagnostic(diagnostic);
                        issued_member_error = true;
                    }
                }
            }
        }
        if !issued_member_error {
            // check again with diagnostics to generate a less-specific error
            self.check_type_assignable_to(
                type_with_this,
                base_with_this,
                or_else_node(node.name(), node),
                Some(broad_diag),
            );
        }
    }

    // Go: checker/checker.go:4571 getTypeWithoutSignatures
    pub fn get_type_without_signatures(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            // PORT: Go `resolveStructuredTypeMembers` returns `t.AsStructuredType()`,
            // so we read the resolved members back from `t`.
            let _ = self.resolve_structured_type_members(t);
            let (has_signatures, members, properties) = {
                let resolved = self.ty(t).as_structured_type();
                (
                    !resolved.signatures().is_empty(),
                    resolved.members,
                    resolved.properties.clone(),
                )
            };
            if has_signatures {
                let symbol = self.ty(t).symbol;
                let result = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
                self.ty_mut(result).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
                let structured = self.ty_mut(result).as_structured_type_mut();
                structured.members = members;
                structured.properties = properties;
                return result;
            }
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(t).types_list();
            let mut mapped = Vec::with_capacity(types.len());
            for member in types {
                mapped.push(self.get_type_without_signatures(member));
            }
            return self.get_intersection_type(&mapped);
        }
        t
    }

    // Go: checker/checker.go:4588 checkKindsOfPropertyMemberOverrides
    pub fn check_kinds_of_property_member_overrides(&mut self, t: TypeId, base_type: TypeId) {
        // TypeScript 1.0 spec (April 2014): 8.2.3
        // A derived class inherits all members from its base class it doesn't override.
        // Inheritance means that a derived class implicitly contains all non - overridden members of the base class.
        // Both public and private property members are inherited, but only public property members can be overridden.
        // A property member in a derived class is said to override a property member in a base class
        // when the derived class property member has the same name and kind(instance or static)
        // as the base class property member.
        // The type of an overriding property member must be assignable(section 3.8.4)
        // to the type of the overridden property member, or otherwise a compile - time error occurs.
        // Base class instance member functions can be overridden by derived class instance member functions,
        // but not by other kinds of members.
        // Base class instance member variables and accessors can be overridden by
        // derived class instance member variables and accessors, but not by other kinds of members.
        // NOTE: assignability is checked in checkClassDeclaration
        #[derive(Clone, Default)]
        struct MemberInfo {
            missed_properties: Vec<String>,
            base_type_name: String,
            type_name: String,
        }
        // PORT: Go iterates a map (random order); IndexMap keeps insertion order.
        let mut not_implemented_info: IndexMap<Node, MemberInfo> = IndexMap::new();
        'base_property_check: for base_property in self.get_properties_of_type(base_type) {
            let base = self.get_target_symbol(base_property);
            if self.sym(base).flags.intersects(SymbolFlags::PROTOTYPE) {
                continue;
            }
            let base_name = self.sym(base).name.clone();
            let base_symbol = self.get_property_of_object_type(t, &base_name);
            if base_symbol.is_nil() {
                continue;
            }
            let derived = self.get_target_symbol(base_symbol);
            let base_declaration_flags = self.get_declaration_modifier_flags_from_symbol(base);
            // In order to resolve whether the inherited method was overridden in the base class or not,
            // we compare the Symbols obtained. Since getTargetSymbol returns the symbol on the *uninstantiated*
            // type declaration, derived and base resolve to the same symbol even in the case of generic classes.
            if derived == base {
                // derived class inherits base without override/redeclaration.
                if base_declaration_flags.intersects(ModifierFlags::ABSTRACT) {
                    // It is an error to inherit an abstract member without implementing it or being declared abstract.
                    // If there is no declaration for the derived class (as in the case of class expressions),
                    // then the class cannot be declared abstract.
                    let t_symbol = self.ty(t).symbol;
                    let derived_class_decl =
                        get_class_like_declaration_of_symbol(&self.symbols, t_symbol);
                    if derived_class_decl.is_nil()
                        || !has_syntactic_modifier(derived_class_decl, ModifierFlags::ABSTRACT)
                    {
                        // Searches other base types for a declaration that would satisfy the inherited abstract member.
                        // (The class may have more than one base type via declaration merging with an interface with the
                        // same name.)
                        for other_base_type in self.get_base_types(t) {
                            if other_base_type == base_type {
                                continue;
                            }
                            let base_symbol =
                                self.get_property_of_object_type(other_base_type, &base_name);
                            if base_symbol.is_some() && base != self.get_target_symbol(base_symbol)
                            {
                                // Derived property exists elsewhere.
                                continue 'base_property_check;
                            }
                        }
                        let base_type_name = self.type_to_string_exported(base_type);
                        let type_name = self.type_to_string_exported(t);
                        let mut missed_properties = not_implemented_info
                            .get(&derived_class_decl)
                            .map(|info| info.missed_properties.clone())
                            .unwrap_or_default();
                        missed_properties.push(self.symbol_to_string(base_property));
                        not_implemented_info.insert(
                            derived_class_decl,
                            MemberInfo {
                                base_type_name,
                                type_name,
                                missed_properties,
                            },
                        );
                    }
                }
            } else {
                // derived overrides base.
                let derived_declaration_flags =
                    self.get_declaration_modifier_flags_from_symbol(derived);
                if base_declaration_flags.intersects(ModifierFlags::PRIVATE)
                    || derived_declaration_flags.intersects(ModifierFlags::PRIVATE)
                {
                    // either base or derived property is private - not override, skip it
                    continue;
                }
                let error_message: &'static Message;
                let base_flags = self.sym(base).flags;
                let derived_flags = self.sym(derived).flags;
                let base_property_flags = base_flags & SymbolFlags::PROPERTY_OR_ACCESSOR;
                let derived_property_flags = derived_flags & SymbolFlags::PROPERTY_OR_ACCESSOR;
                let derived_value_declaration = self.sym(derived).value_declaration;
                if !base_property_flags.is_empty() && !derived_property_flags.is_empty() {
                    // property/accessor is overridden with property/accessor
                    if self.sym(base).check_flags.intersects(CheckFlags::MAPPED)
                        || derived_value_declaration.is_some()
                            && is_binary_expression(derived_value_declaration)
                        || self.are_properties_abstract_or_interface(base, base_declaration_flags)
                    {
                        // when the base property is abstract or from an interface, base/derived flags don't need to match
                        // for intersection properties, this must be true of *any* of the declarations, for others it must be true of *all*
                        // same when the derived property is from an assignment
                        continue;
                    }
                    let overridden_instance_property = base_property_flags != SymbolFlags::PROPERTY
                        && derived_property_flags == SymbolFlags::PROPERTY;
                    let overridden_instance_accessor = base_property_flags == SymbolFlags::PROPERTY
                        && derived_property_flags != SymbolFlags::PROPERTY;
                    if overridden_instance_property || overridden_instance_accessor {
                        let error_message = if overridden_instance_property {
                            diag::X_0_is_defined_as_an_accessor_in_class_1_but_is_overridden_here_in_2_as_an_instance_property
                        } else {
                            diag::X_0_is_defined_as_a_property_in_class_1_but_is_overridden_here_in_2_as_an_accessor
                        };
                        let error_node = or_else_node(
                            get_name_of_declaration(derived_value_declaration),
                            derived_value_declaration,
                        );
                        let base_string = self.symbol_to_string(base);
                        let base_type_string = self.type_to_string_exported(base_type);
                        let type_string = self.type_to_string_exported(t);
                        self.error(
                            error_node,
                            error_message,
                            args![base_string, base_type_string, type_string],
                        );
                    } else if self.compiler_options.get_use_define_for_class_fields() {
                        let derived_declarations = self.sym(derived).declarations.clone();
                        let uninitialized = derived_declarations
                            .iter()
                            .copied()
                            .find(|&d| is_property_declaration(d) && d.initializer().is_nil())
                            .unwrap_or(Node::NIL);
                        if uninitialized.is_some()
                            && !derived_flags.intersects(SymbolFlags::TRANSIENT)
                            && !base_declaration_flags.intersects(ModifierFlags::ABSTRACT)
                            && !derived_declaration_flags.intersects(ModifierFlags::ABSTRACT)
                            && !derived_declarations
                                .iter()
                                .any(|&d| d.flags().intersects(NodeFlags::AMBIENT))
                        {
                            let t_symbol = self.ty(t).symbol;
                            let constructor = find_constructor_declaration(
                                get_class_like_declaration_of_symbol(&self.symbols, t_symbol),
                            );
                            let prop_name = uninitialized.name();
                            if is_exclamation_token(uninitialized.postfix_token())
                                || constructor.is_nil()
                                || !is_identifier(prop_name)
                                || !self.strict_null_checks
                                || !self.is_property_initialized_in_constructor(
                                    prop_name,
                                    t,
                                    constructor,
                                )
                            {
                                let error_message = diag::Property_0_will_overwrite_the_base_property_in_1_If_this_is_intentional_add_an_initializer_Otherwise_add_a_declare_modifier_or_remove_the_redundant_declaration;
                                let error_node = or_else_node(
                                    get_name_of_declaration(derived_value_declaration),
                                    derived_value_declaration,
                                );
                                let base_string = self.symbol_to_string(base);
                                let base_type_string = self.type_to_string_exported(base_type);
                                self.error(
                                    error_node,
                                    error_message,
                                    args![base_string, base_type_string],
                                );
                            }
                        }
                    }
                    // correct case
                    continue;
                } else if self.is_prototype_property(base) {
                    if self.is_prototype_property(derived)
                        || derived_flags.intersects(SymbolFlags::PROPERTY)
                    {
                        // method is overridden with method or property -- correct case
                        continue;
                    } else {
                        error_message = diag::Class_0_defines_instance_member_function_1_but_extended_class_2_defines_it_as_instance_member_accessor;
                    }
                } else if base_flags.intersects(SymbolFlags::ACCESSOR) {
                    error_message = diag::Class_0_defines_instance_member_accessor_1_but_extended_class_2_defines_it_as_instance_member_function;
                } else {
                    error_message = diag::Class_0_defines_instance_member_property_1_but_extended_class_2_defines_it_as_instance_member_function;
                }
                let error_node = or_else_node(
                    get_name_of_declaration(derived_value_declaration),
                    derived_value_declaration,
                );
                let base_type_string = self.type_to_string_exported(base_type);
                let base_string = self.symbol_to_string(base);
                let type_string = self.type_to_string_exported(t);
                self.error(
                    error_node,
                    error_message,
                    args![base_type_string, base_string, type_string],
                );
            }
        }
        for (error_node, member_info) in not_implemented_info {
            let count = member_info.missed_properties.len();
            if count == 1 {
                let missed_property = member_info.missed_properties[0].clone();
                if is_class_expression(error_node) {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_expression_does_not_implement_inherited_abstract_member_0_from_class_1,
                        args![missed_property, member_info.base_type_name],
                    );
                } else {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_0_does_not_implement_inherited_abstract_member_1_from_class_2,
                        args![member_info.type_name, missed_property, member_info.base_type_name],
                    );
                }
            } else if count > 5 {
                let missed_properties =
                    quoted_and_comma_separated(&member_info.missed_properties[..4]);
                let remaining_missed_properties = count as i32 - 4;
                if is_class_expression(error_node) {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_expression_is_missing_implementations_for_the_following_members_of_0_Colon_1_and_2_more,
                        args![member_info.base_type_name, missed_properties, remaining_missed_properties],
                    );
                } else {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_0_is_missing_implementations_for_the_following_members_of_1_Colon_2_and_3_more,
                        args![member_info.type_name, member_info.base_type_name, missed_properties, remaining_missed_properties],
                    );
                }
            } else {
                let missed_properties = quoted_and_comma_separated(&member_info.missed_properties);
                if is_class_expression(error_node) {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_expression_is_missing_implementations_for_the_following_members_of_0_Colon_1,
                        args![member_info.base_type_name, missed_properties],
                    );
                } else {
                    self.error(
                        error_node,
                        diag::Non_abstract_class_0_is_missing_implementations_for_the_following_members_of_1_Colon_2,
                        args![member_info.type_name, member_info.base_type_name, missed_properties],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:4744 arePropertiesAbstractOrInterface
    pub fn are_properties_abstract_or_interface(
        &self,
        base: SymbolId,
        base_declaration_flags: ModifierFlags,
    ) -> bool {
        let symbol = self.sym(base);
        if symbol.check_flags.intersects(CheckFlags::SYNTHETIC) {
            return symbol
                .declarations
                .iter()
                .any(|&d| self.is_property_abstract_or_interface(d, base_declaration_flags));
        }
        symbol
            .declarations
            .iter()
            .all(|&d| self.is_property_abstract_or_interface(d, base_declaration_flags))
    }

    // Go: checker/checker.go:4751 isPropertyAbstractOrInterface
    pub fn is_property_abstract_or_interface(
        &self,
        declaration: Node,
        base_declaration_flags: ModifierFlags,
    ) -> bool {
        is_interface_declaration(declaration.parent())
            || base_declaration_flags.intersects(ModifierFlags::ABSTRACT)
                && (!is_property_declaration(declaration) || declaration.initializer().is_nil())
    }

    // Go: checker/checker.go:4756 checkMembersForOverrideModifier
    pub fn check_members_for_override_modifier(
        &mut self,
        node: Node,
        t: TypeId,
        type_with_this: TypeId,
        static_type: TypeId,
    ) {
        let mut base_with_this = TypeId::NIL;
        let base_type_node = get_class_extends_heritage_element(node);
        if base_type_node.is_some() {
            let base_types = self.get_base_types(t);
            if !base_types.is_empty() {
                let this_type = self.ty(t).as_interface_type().this_type;
                base_with_this = self.get_type_with_this_argument(base_types[0], this_type, false);
            }
        }
        let base_static_type = self.get_base_constructor_type_of_class(t);
        for member in node.members() {
            if !has_ambient_modifier(member) {
                if is_constructor_declaration(member) {
                    for param in member.parameters() {
                        if is_parameter_property_declaration(param, member) {
                            self.check_member_for_override_modifier(
                                node,
                                static_type,
                                base_static_type,
                                base_with_this,
                                t,
                                type_with_this,
                                param,
                            );
                        }
                    }
                } else {
                    self.check_member_for_override_modifier(
                        node,
                        static_type,
                        base_static_type,
                        base_with_this,
                        t,
                        type_with_this,
                        member,
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:4781 checkMemberForOverrideModifier
    pub fn check_member_for_override_modifier(
        &mut self,
        node: Node,
        static_type: TypeId,
        base_static_type: TypeId,
        base_with_this: TypeId,
        t: TypeId,
        type_with_this: TypeId,
        member: Node,
    ) {
        let symbol = self.get_symbol_of_declaration(member);
        if symbol.is_nil() {
            return;
        }

        self.check_member_for_override_modifier_worker(
            node,
            static_type,
            base_static_type,
            base_with_this,
            t,
            type_with_this,
            has_override_modifier(member),
            has_abstract_modifier(member),
            is_static(member),
            is_parameter_declaration(member),
            symbol,
            member,
        );
    }

    // Go: checker/checker.go:4790 getMemberOverrideModifierStatus
    pub fn get_member_override_modifier_status(
        &mut self,
        node: Node,
        member: Node,
        member_symbol: SymbolId,
    ) -> MemberOverrideStatus {
        if member.name().is_nil() || member_symbol.is_nil() {
            return MemberOverrideStatus::NONE;
        }

        let class_symbol = self.get_symbol_of_declaration(node);
        if class_symbol.is_nil() {
            return MemberOverrideStatus::NONE;
        }

        let t = self.get_declared_type_of_symbol(class_symbol);
        let type_with_this = self.get_type_with_this_argument(t, TypeId::NIL, false);
        let static_type = self.get_type_of_symbol(class_symbol);

        let mut base_with_this = TypeId::NIL;
        if get_class_extends_heritage_element(node).is_some() {
            let base_types = self.get_base_types(t);
            if !base_types.is_empty() {
                let this_type = self.ty(t).as_interface_type().this_type;
                base_with_this = self.get_type_with_this_argument(base_types[0], this_type, false);
            }
        }

        let base_static_type = self.get_base_constructor_type_of_class(t);
        self.check_member_for_override_modifier_worker(
            node,
            static_type,
            base_static_type,
            base_with_this,
            t,
            type_with_this,
            has_syntactic_modifier(member, ModifierFlags::OVERRIDE),
            has_abstract_modifier(member),
            is_static(member),
            false, /*memberIsParameterProperty*/
            member_symbol,
            Node::NIL, /*errorNode*/
        )
    }

    // Go: checker/checker.go:4815 checkMemberForOverrideModifierWorker
    #[allow(clippy::too_many_arguments)]
    pub fn check_member_for_override_modifier_worker(
        &mut self,
        node: Node,
        static_type: TypeId,
        base_static_type: TypeId,
        base_with_this: TypeId,
        t: TypeId,
        type_with_this: TypeId,
        member_has_override_modifier: bool,
        member_has_abstract_modifier: bool,
        member_is_static: bool,
        member_is_parameter_property: bool,
        member: SymbolId,
        error_node: Node,
    ) -> MemberOverrideStatus {
        let is_js = is_in_js_file(node);
        let value_declaration = self.sym(member).value_declaration;
        if member_has_override_modifier
            && value_declaration.is_some()
            && is_class_element(value_declaration)
            && value_declaration.name().is_some()
            && self.is_non_bindable_dynamic_name(value_declaration.name())
        {
            if error_node.is_some() {
                let message = if is_js {
                    diag::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_its_name_is_dynamic
                } else {
                    diag::This_member_cannot_have_an_override_modifier_because_its_name_is_dynamic
                };
                self.error(error_node, message, args![]);
            }
            return MemberOverrideStatus::HAS_INVALID_OVERRIDE;
        }

        if base_with_this.is_some()
            && (member_has_override_modifier
                || self.compiler_options.no_implicit_override.is_true())
        {
            let this_type = if member_is_static {
                static_type
            } else {
                type_with_this
            };
            let base_type = if member_is_static {
                base_static_type
            } else {
                base_with_this
            };
            let member_name = self.sym(member).name.clone();
            let prop = self.get_property_of_type(this_type, &member_name);
            let base_prop = self.get_property_of_type(base_type, &member_name);

            if prop.is_some() && base_prop.is_nil() && member_has_override_modifier {
                if error_node.is_some() {
                    let name = symbol_name(&self.symbols, member);
                    let suggestion =
                        self.get_suggested_symbol_for_nonexistent_class_member(&name, base_type);
                    if suggestion.is_some() {
                        let message = if is_js {
                            diag::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_it_is_not_declared_in_the_base_class_0_Did_you_mean_1
                        } else {
                            diag::This_member_cannot_have_an_override_modifier_because_it_is_not_declared_in_the_base_class_0_Did_you_mean_1
                        };
                        let base_string = self.type_to_string_exported(base_with_this);
                        let suggestion_string = self.symbol_to_string(suggestion);
                        self.error(error_node, message, args![base_string, suggestion_string]);
                    } else {
                        let message = if is_js {
                            diag::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_it_is_not_declared_in_the_base_class_0
                        } else {
                            diag::This_member_cannot_have_an_override_modifier_because_it_is_not_declared_in_the_base_class_0
                        };
                        let base_string = self.type_to_string_exported(base_with_this);
                        self.error(error_node, message, args![base_string]);
                    }
                }
                return MemberOverrideStatus::HAS_INVALID_OVERRIDE;
            }

            if prop.is_some()
                && base_prop.is_some()
                && !self.sym(base_prop).declarations.is_empty()
                && self.compiler_options.no_implicit_override.is_true()
                && !node.flags().intersects(NodeFlags::AMBIENT)
            {
                let base_has_abstract = self
                    .sym(base_prop)
                    .declarations
                    .iter()
                    .any(|&d| has_abstract_modifier(d));
                if member_has_override_modifier {
                    return MemberOverrideStatus::NONE;
                }
                if !base_has_abstract {
                    if error_node.is_some() {
                        let message = if member_is_parameter_property {
                            if is_js {
                                diag::This_parameter_property_must_have_a_JSDoc_comment_with_an_override_tag_because_it_overrides_a_member_in_the_base_class_0
                            } else {
                                diag::This_parameter_property_must_have_an_override_modifier_because_it_overrides_a_member_in_base_class_0
                            }
                        } else if is_js {
                            diag::This_member_must_have_a_JSDoc_comment_with_an_override_tag_because_it_overrides_a_member_in_the_base_class_0
                        } else {
                            diag::This_member_must_have_an_override_modifier_because_it_overrides_a_member_in_the_base_class_0
                        };
                        let base_string = self.type_to_string_exported(base_with_this);
                        self.error(error_node, message, args![base_string]);
                    }
                    return MemberOverrideStatus::NEEDS_OVERRIDE;
                }
                if member_has_abstract_modifier {
                    if error_node.is_some() {
                        let base_string = self.type_to_string_exported(base_with_this);
                        self.error(
                            error_node,
                            diag::This_member_must_have_an_override_modifier_because_it_overrides_an_abstract_method_that_is_declared_in_the_base_class_0,
                            args![base_string],
                        );
                    }
                    return MemberOverrideStatus::NEEDS_OVERRIDE;
                }
            }
        } else if member_has_override_modifier {
            if error_node.is_some() {
                let message = if is_js {
                    diag::This_member_cannot_have_a_JSDoc_comment_with_an_override_tag_because_its_containing_class_0_does_not_extend_another_class
                } else {
                    diag::This_member_cannot_have_an_override_modifier_because_its_containing_class_0_does_not_extend_another_class
                };
                let type_string = self.type_to_string_exported(t);
                self.error(error_node, message, args![type_string]);
            }
            return MemberOverrideStatus::HAS_INVALID_OVERRIDE;
        }

        MemberOverrideStatus::NONE
    }

    // Go: checker/checker.go:4873 getSuggestedSymbolForNonexistentClassMember
    pub fn get_suggested_symbol_for_nonexistent_class_member(
        &mut self,
        name: &str,
        base_type: TypeId,
    ) -> SymbolId {
        // PORT: Go passes `slices.Values(props)` (an `iter.Seq`); the Rust callee takes a slice.
        let properties = self.get_properties_of_type(base_type);
        self.get_spelling_suggestion_for_name(name, &properties, SymbolFlags::CLASS_MEMBER)
    }

    // Go: checker/checker.go:4877 checkIndexConstraints
    pub fn check_index_constraints(&mut self, t: TypeId, symbol: SymbolId, is_static_index: bool) {
        let index_infos = self.get_index_infos_of_type(t);
        if index_infos.is_empty() {
            return;
        }
        for prop in self.get_properties_of_object_type(t) {
            if !(is_static_index && self.sym(prop).flags.intersects(SymbolFlags::PROTOTYPE)) {
                let prop_name_type = self.get_literal_type_from_property(
                    prop,
                    TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                    true, /*includeNonPublic*/
                );
                let prop_type = self.get_non_missing_type_of_symbol(prop);
                self.check_index_constraint_for_property(t, prop, prop_name_type, prop_type);
            }
        }
        let type_declaration = self.sym(symbol).value_declaration;
        if type_declaration.is_some() && is_class_like(type_declaration) {
            for member in type_declaration.members() {
                // Only process instance properties against instance index signatures and static properties against static index signatures
                if is_static(member) == is_static_index && !self.has_bindable_name(member) {
                    let symbol = self.get_symbol_of_declaration(member);
                    let name_type = self.get_type_of_expression(member.name().expression());
                    let prop_type = self.get_non_missing_type_of_symbol(symbol);
                    self.check_index_constraint_for_property(t, symbol, name_type, prop_type);
                }
            }
        }
        if index_infos.len() > 1 {
            for info in index_infos {
                self.check_index_constraint_for_index_signature(t, info);
            }
        }
    }
}
