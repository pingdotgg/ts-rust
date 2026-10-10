//! Port of Go `transformers/moduletransforms/commonjsmodule.go` lines 978 to
//! 2160: `visitTopLevelVariableStatement` through `getExports`.

use super::commonjs_module::{CommonJSModuleTransformer, VisitorKind};
use super::utilities::{
    get_external_module_name_literal, is_declaration_name_of_enum_or_namespace,
    is_file_level_reserved_generated_identifier, is_simple_inlineable_expression,
    rewrite_module_specifier,
};
use crate::prelude::*;
use crate::transformers::destructuring::{FlattenLevel, flatten_destructuring_assignment};
use crate::transformers::modifier_visitor::extract_modifiers;
use crate::transformers::utilities::{
    convert_variable_declaration_to_assignment_expression, is_export_name, is_generated_identifier,
    is_helper_name, is_identifier_reference, is_local_name, single_or_many,
};

impl CommonJSModuleTransformer {
    // Go: transformers/moduletransforms/commonjsmodule.go:976 CommonJSModuleTransformer.visitTopLevelVariableStatement
    pub(super) fn visit_top_level_variable_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut statements: Vec<Node> = Vec::new();
        if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
            // export var a = b;
            let mut variables: Vec<Node> = Vec::new();
            let mut expressions: Vec<Node> = Vec::new();
            let mut modifiers = ModifierList::NIL;

            // PORT: Go closures `commitPendingVariables`, `commitPendingExpressions`,
            // `pushVariable` and `pushExpression` share the locals above; they are
            // local fns over those locals here.
            let commit_pending_variables =
                |statements: &mut Vec<Node>, variables: &mut Vec<Node>, modifiers: ModifierList| {
                    if !variables.is_empty() {
                        let variable_list = f.new_node_list(variables);
                        let statement = f.update_variable_statement(
                            node,
                            modifiers,
                            f.update_variable_declaration_list(
                                node.declaration_list(),
                                variable_list,
                                node.declaration_list().flags(),
                            ),
                        );
                        if !statements.is_empty() {
                            ec.add_emit_flags(statement, EmitFlags::NO_COMMENTS);
                        }
                        statements.push(statement);
                        variables.clear();
                    }
                };

            let commit_pending_expressions =
                |statements: &mut Vec<Node>, expressions: &mut Vec<Node>| {
                    if !expressions.is_empty() {
                        let statement =
                            f.new_expression_statement(f.inline_expressions(expressions));
                        ec.assign_comment_and_source_map_ranges(statement, node);
                        if !statements.is_empty() {
                            ec.add_emit_flags(statement, EmitFlags::NO_COMMENTS);
                        }
                        statements.push(statement);
                        expressions.clear();
                    }
                };

            // If we're exporting these variables, then these just become assignments to 'exports.x'.
            for mut variable in node.declaration_list().declarations().nodes().to_vec() {
                let v = variable;

                if is_identifier(v.name()) && is_local_name(&ec, v.name()) {
                    // A "local name" generally means a variable declaration that *shouldn't* be
                    // converted to `exports.x = ...`, even if the declaration is exported. This
                    // usually indicates a class or function declaration that was converted into
                    // a variable declaration, as most references to the declaration will remain
                    // untransformed (i.e., `new C` rather than `new exports.C`). In these cases,
                    // an `export { x }` declaration will follow.

                    if modifiers.is_nil() {
                        modifiers = extract_modifiers(
                            &ec,
                            node.modifiers(),
                            !ModifierFlags::EXPORT_DEFAULT,
                        );
                    }

                    if v.initializer().is_some() {
                        let initializer = self.visit_node_with(VisitorKind::Root, v.initializer());
                        variable = f.update_variable_declaration(
                            v,
                            v.name(),
                            Node::NIL, /*exclamationToken*/
                            Node::NIL, /*type*/
                            self.create_export_expression(
                                v.name(),
                                initializer,
                                None,
                                false, /*liveBinding*/
                            ),
                        );
                    }

                    // pushVariable
                    commit_pending_expressions(&mut statements, &mut expressions);
                    variables.push(variable);
                } else if v.initializer().is_some()
                    && !is_binding_pattern(v.name())
                    && (is_arrow_function(v.initializer())
                        || is_function_expression(v.initializer())
                        || is_class_expression(v.initializer()))
                {
                    // preserve variable declarations for functions and classes to assign names

                    let initializer = self.visit_node_with(VisitorKind::Root, v.initializer());
                    // pushVariable
                    commit_pending_expressions(&mut statements, &mut expressions);
                    variables.push(f.new_variable_declaration(
                        v.name(),
                        v.exclamation_token(),
                        v.type_(),
                        initializer,
                    ));

                    let property_access = f.new_property_access_expression(
                        f.new_identifier("exports"),
                        Node::NIL, /*questionDotToken*/
                        v.name(),
                        NodeFlags::NONE,
                    );
                    ec.assign_comment_and_source_map_ranges(property_access, v.name());

                    // pushExpression
                    commit_pending_variables(&mut statements, &mut variables, modifiers);
                    expressions
                        .push(f.new_assignment_expression(property_access, f.clone_node(v.name())));
                } else if is_identifier(v.name()) {
                    let expression = self.transform_initialized_variable(v);
                    if expression.is_some() {
                        let expression = self.visit_node_with(VisitorKind::Root, expression);
                        // pushExpression
                        commit_pending_variables(&mut statements, &mut variables, modifiers);
                        expressions.push(expression);
                    }
                } else if is_binding_pattern(v.name()) {
                    // For binding patterns with export modifier, use flattenDestructuringAssignment
                    // to decompose into individual export assignments
                    let expression = self.transform_initialized_variable(v);
                    if expression.is_some() {
                        // pushExpression
                        commit_pending_variables(&mut statements, &mut variables, modifiers);
                        expressions.push(expression);
                    }
                } else {
                    // For binding patterns, we can't do exports.{pattern} = value
                    // Just emit the assignment and let appendExportsOfVariableStatement handle the exports
                    let expression = convert_variable_declaration_to_assignment_expression(&ec, v);
                    if expression.is_some() {
                        let expression = self.visit_node_with(VisitorKind::Root, expression);
                        // pushExpression
                        commit_pending_variables(&mut statements, &mut variables, modifiers);
                        expressions.push(expression);
                    }
                }
            }

            commit_pending_variables(&mut statements, &mut variables, modifiers);
            commit_pending_expressions(&mut statements, &mut expressions);
            statements = self.append_exports_of_variable_statement(statements, node);
            // PORT: Go `statements` is a nil slice when nothing was appended
            // (`export const x: T;`), so SingleOrMany returns nil, not an empty
            // SyntaxList. Callers then emit `{ }` or `;` in its place.
            return single_or_many((!statements.is_empty()).then_some(&statements[..]), f);
        }
        self.visit_top_level_nested_variable_statement(node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1110 CommonJSModuleTransformer.transformInitializedVariable
    fn transform_initialized_variable(&mut self, node: Node) -> Node {
        if node.initializer().is_nil() {
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let name = node.name();
        if is_binding_pattern(name) {
            // Convert the binding pattern into an equivalent assignment expression and visit it
            // as a destructuring assignment. This preserves native destructuring (and therefore
            // iterator semantics for array patterns) whenever each leaf identifier can be
            // substituted to an export reference. Only when the destructuring would assign to
            // re-aliased or multi-exported names (where native destructuring cannot update all
            // targets) does `visitDestructuringAssignment` fall back to flattening.
            let assignment = convert_variable_declaration_to_assignment_expression(&ec, node);
            let grandparent_node = self.push_node(assignment);
            let result =
                self.visit_destructuring_assignment(assignment, true /*valueIsDiscarded*/);
            self.pop_node(grandparent_node);
            return result;
        }
        let property_access = f.new_property_access_expression(
            f.new_identifier("exports"),
            Node::NIL, /*questionDotToken*/
            name,
            NodeFlags::NONE,
        );
        ec.assign_comment_and_source_map_ranges(property_access, name);
        f.new_assignment_expression(property_access, node.initializer())
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1139 CommonJSModuleTransformer.visitTopLevelNestedVariableStatement
    /// Visits a top-level nested variable statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    fn visit_top_level_nested_variable_statement(&mut self, node: Node) -> Node {
        let mut statements = vec![self.visit_each_child_with(VisitorKind::Root, node)];
        statements = self.append_exports_of_variable_statement(statements, node);
        single_or_many(Some(&statements), self.emit_context.factory())
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1148 CommonJSModuleTransformer.visitTopLevelNestedForStatement
    /// Visits a top-level nested `for` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_for_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let initializer = node.initializer();
        if initializer.is_some()
            && is_variable_declaration_list(initializer)
            && !initializer.flags().intersects(NodeFlags::BLOCK_SCOPED)
        {
            let export_statements = self.append_exports_of_variable_declaration_list(
                Vec::new(), /*statements*/
                initializer,
                false, /*isForInOrOfInitializer*/
            );
            if !export_statements.is_empty() {
                // given:
                //   export { x }
                //   for (var x = 0; ;) { }
                // emits:
                //   var x = 0;
                //   exports.x = x;
                //   for (; ;) { }

                let mut statements: Vec<Node> = Vec::new();
                let var_decl_list = self.visit_node_with(VisitorKind::DiscardedValue, initializer);
                let var_statement =
                    f.new_variable_statement(ModifierList::NIL /*modifiers*/, var_decl_list);
                statements.push(var_statement);
                statements.extend(export_statements);

                let condition = self.visit_node_with(VisitorKind::Root, node.condition());
                let incrementor =
                    self.visit_node_with(VisitorKind::DiscardedValue, node.incrementor());
                let body =
                    self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
                statements.push(f.update_for_statement(
                    node,
                    Node::NIL, /*initializer*/
                    condition,
                    incrementor,
                    body,
                ));
                return single_or_many(Some(&statements), f);
            }
        }
        let initializer = self.visit_node_with(VisitorKind::DiscardedValue, initializer);
        let condition = self.visit_node_with(VisitorKind::Root, node.condition());
        let incrementor = self.visit_node_with(VisitorKind::DiscardedValue, node.incrementor());
        let body = self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
        f.update_for_statement(node, initializer, condition, incrementor, body)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1190 CommonJSModuleTransformer.visitTopLevelNestedForInOrOfStatement
    /// Visits a top-level nested `for..in` or `for..of` statement as it may contain `var` declarations that are hoisted and
    /// may still be exported with `export {}`.
    pub(super) fn visit_top_level_nested_for_in_or_of_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let initializer = node.initializer();
        if is_variable_declaration_list(initializer)
            && !initializer.flags().intersects(NodeFlags::BLOCK_SCOPED)
        {
            let export_statements = self.append_exports_of_variable_declaration_list(
                Vec::new(), /*statements*/
                initializer,
                true, /*isForInOrOfInitializer*/
            );
            if !export_statements.is_empty() {
                // given:
                //   export { x }
                //   for (var x in y) {
                //     ...
                //   }
                // emits:
                //   for (var x in y) {
                //     exports.x = x;
                //     ...
                //   }

                let initializer = self.visit_node_with(VisitorKind::DiscardedValue, initializer);
                let expression = self.visit_node_with(VisitorKind::Root, node.expression());
                let mut body =
                    self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
                if is_block(body) {
                    let block = body;
                    let mut body_statements = export_statements;
                    body_statements.extend(block.statements().iter());
                    let body_statement_list =
                        f.new_node_list_with_loc(&body_statements, block.statement_list().loc());
                    body = f.update_block(block, body_statement_list, block.multi_line());
                } else {
                    let mut body_statements = export_statements;
                    body_statements.push(body);
                    body = f.new_block(f.new_node_list(&body_statements), true /*multiLine*/);
                }
                return f.update_for_in_or_of_statement(
                    node,
                    node.await_modifier(),
                    initializer,
                    expression,
                    body,
                );
            }
        }
        let initializer = self.visit_node_with(VisitorKind::DiscardedValue, initializer);
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let body = self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
        f.update_for_in_or_of_statement(node, node.await_modifier(), initializer, expression, body)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1232 CommonJSModuleTransformer.visitTopLevelNestedDoStatement
    /// Visits a top-level nested `do` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_do_statement(&mut self, node: Node) -> Node {
        let statement =
            self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        self.emit_context
            .factory()
            .update_do_statement(node, statement, expression)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1242 CommonJSModuleTransformer.visitTopLevelNestedWhileStatement
    /// Visits a top-level nested `while` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_while_statement(&mut self, node: Node) -> Node {
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let statement =
            self.visit_iteration_body_with(VisitorKind::TopLevelNested, node.statement());
        self.emit_context
            .factory()
            .update_while_statement(node, expression, statement)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1252 CommonJSModuleTransformer.visitTopLevelNestedLabeledStatement
    /// Visits a top-level nested labeled statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_labeled_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut statement =
            self.visit_embedded_statement_with(VisitorKind::TopLevelNested, node.statement());
        if statement.is_nil() {
            statement = f.new_empty_statement();
        }
        f.update_labeled_statement(node, node.label(), statement)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1262 CommonJSModuleTransformer.visitTopLevelNestedWithStatement
    /// Visits a top-level nested `with` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_with_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        // Go visits the statement first, then the expression (an argument of the update call).
        let mut statement =
            self.visit_embedded_statement_with(VisitorKind::TopLevelNested, node.statement());
        if statement.is_nil() {
            statement = f.new_empty_statement();
        }
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        f.update_with_statement(node, expression, statement)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1272 CommonJSModuleTransformer.visitTopLevelNestedIfStatement
    /// Visits a top-level nested `if` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_if_statement(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let mut then_statement =
            self.visit_embedded_statement_with(VisitorKind::TopLevelNested, node.then_statement());
        if then_statement.is_nil() {
            then_statement = f.new_block(f.new_node_list(&[]), false /*multiLine*/);
        }
        let else_statement =
            self.visit_embedded_statement_with(VisitorKind::TopLevelNested, node.else_statement());
        f.update_if_statement(node, expression, then_statement, else_statement)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1284 CommonJSModuleTransformer.visitTopLevelNestedSwitchStatement
    /// Visits a top-level nested `switch` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_switch_statement(&mut self, node: Node) -> Node {
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let case_block = self.visit_node_with(VisitorKind::TopLevelNested, node.case_block());
        self.emit_context
            .factory()
            .update_switch_statement(node, expression, case_block)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1294 CommonJSModuleTransformer.visitTopLevelNestedCaseBlock
    /// Visits a top-level nested case block as it may contain `var` declarations that are hoisted and may still be exported
    /// with `export {}`.
    pub(super) fn visit_top_level_nested_case_block(&mut self, node: Node) -> Node {
        self.visit_each_child_with(VisitorKind::TopLevelNested, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1300 CommonJSModuleTransformer.visitTopLevelNestedCaseOrDefaultClause
    /// Visits a top-level nested `case` or `default` clause as it may contain `var` declarations that are hoisted and may
    /// still be exported with `export {}`.
    pub(super) fn visit_top_level_nested_case_or_default_clause(&mut self, node: Node) -> Node {
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let statements = self.visit_nodes_with(VisitorKind::TopLevelNested, node.statement_list());
        self.emit_context
            .factory()
            .update_case_or_default_clause(node, expression, statements)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1310 CommonJSModuleTransformer.visitTopLevelNestedTryStatement
    /// Visits a top-level nested `try` statement as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_try_statement(&mut self, node: Node) -> Node {
        self.visit_each_child_with(VisitorKind::TopLevelNested, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1316 CommonJSModuleTransformer.visitTopLevelNestedCatchClause
    /// Visits a top-level nested `catch` clause as it may contain `var` declarations that are hoisted and may still be
    /// exported with `export {}`.
    pub(super) fn visit_top_level_nested_catch_clause(&mut self, node: Node) -> Node {
        let block = self.visit_node_with(VisitorKind::TopLevelNested, node.block());
        self.emit_context
            .factory()
            .update_catch_clause(node, node.variable_declaration(), block)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1326 CommonJSModuleTransformer.visitTopLevelNestedBlock
    /// Visits a top-level nested block as it may contain `var` declarations that are hoisted and may still be exported with
    /// `export {}`.
    pub(super) fn visit_top_level_nested_block(&mut self, node: Node) -> Node {
        self.visit_each_child_with(VisitorKind::TopLevelNested, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1334 CommonJSModuleTransformer.visitForStatement
    pub(super) fn visit_for_statement(&mut self, node: Node) -> Node {
        let initializer = self.visit_node_with(VisitorKind::DiscardedValue, node.initializer());
        let condition = self.visit_node_with(VisitorKind::Root, node.condition());
        let incrementor = self.visit_node_with(VisitorKind::DiscardedValue, node.incrementor());
        let body = self.visit_iteration_body_with(VisitorKind::Root, node.statement());
        self.emit_context.factory().update_for_statement(
            node,
            initializer,
            condition,
            incrementor,
            body,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1344 CommonJSModuleTransformer.visitForInOrOfStatement
    pub(super) fn visit_for_in_or_of_statement(&mut self, node: Node) -> Node {
        let initializer = self.visit_node_with(VisitorKind::DiscardedValue, node.initializer());
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let body = self.visit_iteration_body_with(VisitorKind::Root, node.statement());
        self.emit_context.factory().update_for_in_or_of_statement(
            node,
            node.await_modifier(),
            initializer,
            expression,
            body,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1351 CommonJSModuleTransformer.visitExpressionStatement
    /// Visits an expression statement whose value will be discarded at runtime.
    pub(super) fn visit_expression_statement(&mut self, node: Node) -> Node {
        self.visit_each_child_with(VisitorKind::DiscardedValue, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1356 CommonJSModuleTransformer.visitVoidExpression
    /// Visits a `void` expression whose value will be discarded at runtime.
    pub(super) fn visit_void_expression(&mut self, node: Node) -> Node {
        self.visit_each_child_with(VisitorKind::DiscardedValue, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1361 CommonJSModuleTransformer.visitParenthesizedExpression
    /// Visits a parenthesized expression whose value may be discarded at runtime.
    pub(super) fn visit_parenthesized_expression(
        &mut self,
        node: Node,
        result_is_discarded: bool,
    ) -> Node {
        let kind = if result_is_discarded {
            VisitorKind::DiscardedValue
        } else {
            VisitorKind::Root
        };
        let expression = self.visit_node_with(kind, node.expression());
        self.emit_context
            .factory()
            .update_parenthesized_expression(node, expression)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1367 CommonJSModuleTransformer.visitPartiallyEmittedExpression
    /// Visits a partially emitted expression whose value may be discarded at runtime.
    pub(super) fn visit_partially_emitted_expression(
        &mut self,
        node: Node,
        result_is_discarded: bool,
    ) -> Node {
        let kind = if result_is_discarded {
            VisitorKind::DiscardedValue
        } else {
            VisitorKind::Root
        };
        let expression = self.visit_node_with(kind, node.expression());
        self.emit_context
            .factory()
            .update_partially_emitted_expression(node, expression)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1374 CommonJSModuleTransformer.visitBinaryExpression
    /// Visits a binary expression whose value may be discarded, or which might contain an assignment to an exported
    /// identifier.
    pub(super) fn visit_binary_expression(
        &mut self,
        node: Node,
        result_is_discarded: bool,
    ) -> Node {
        if is_destructuring_assignment(node) {
            return self.visit_destructuring_assignment(node, result_is_discarded);
        }

        if is_assignment_expression(node, false /*excludeCompoundAssignment*/) {
            return self.visit_assignment_expression(node);
        }

        if is_comma_expression(node) {
            return self.visit_comma_expression(node, result_is_discarded);
        }

        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1390 CommonJSModuleTransformer.visitAssignmentExpression
    fn visit_assignment_expression(&mut self, node: Node) -> Node {
        // When we see an assignment expression whose left-hand side is an exported symbol,
        // we should ensure all exports of that symbol are updated with the correct value.
        //
        // - We do not transform generated identifiers unless they are file-level reserved names.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We only transform identifiers that are exported at the top level.
        let ec = self.emit_context.clone();
        let left = node.left();
        if is_identifier(left)
            && (!is_generated_identifier(&ec, left)
                || is_file_level_reserved_generated_identifier(&ec, left))
            && !is_local_name(&ec, left)
        {
            let exported_names = self.get_exports(left);
            if !exported_names.is_empty() {
                // For each additional export of the declaration, apply an export assignment.
                let mut expression = self.visit_each_child_with(VisitorKind::Root, node);
                for export_name in exported_names {
                    expression = self.create_export_expression(
                        export_name,
                        expression,
                        Some(node.loc()), /*location*/
                        false,            /*liveBinding*/
                    );
                }
                return expression;
            }
        }

        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1415 CommonJSModuleTransformer.visitDestructuringAssignment
    /// Visits a destructuring assignment which might target an exported identifier.
    fn visit_destructuring_assignment(&mut self, node: Node, value_is_discarded: bool) -> Node {
        if self.destructuring_needs_flattening(node.left()) {
            // PORT: Go passes `&tx.Transformer` (its root visitor and emit
            // context) and the bound method `tx.createAllExportExpressions`.
            // Here the root visitor carries the transformer as its context, so
            // the callback reaches it through `v.ctx`.
            let emit_context = self.emit_context.clone();
            return self.with_visitor(VisitorKind::Root, |v| {
                flatten_destructuring_assignment(
                    &emit_context,
                    v,
                    node,
                    !value_is_discarded, /*needsValue*/
                    FlattenLevel::All,
                    Some(&mut |v, name, value, location| {
                        v.ctx.create_all_export_expressions(name, value, location)
                    }),
                )
            });
        }
        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1430 CommonJSModuleTransformer.destructuringNeedsFlattening
    /// destructuringNeedsFlattening checks whether a destructuring assignment target contains any
    /// exported identifiers that need to be flattened into individual export assignments.
    fn destructuring_needs_flattening(&mut self, node: Node) -> bool {
        if is_object_literal_expression(node) {
            for elem in node.properties().to_vec() {
                match elem.kind() {
                    SyntaxKind::PropertyAssignment => {
                        if self.destructuring_needs_flattening(elem.initializer()) {
                            return true;
                        }
                    }
                    SyntaxKind::ShorthandPropertyAssignment => {
                        if self.destructuring_needs_flattening(elem.name()) {
                            return true;
                        }
                    }
                    SyntaxKind::SpreadAssignment => {
                        if self.destructuring_needs_flattening(elem.expression()) {
                            return true;
                        }
                    }
                    SyntaxKind::MethodDeclaration
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor => {
                        return false;
                    }
                    _ => {}
                }
            }
        } else if is_array_literal_expression(node) {
            for elem in node.elements().to_vec() {
                if is_spread_element(elem) {
                    if self.destructuring_needs_flattening(elem.expression()) {
                        return true;
                    }
                } else if self.destructuring_needs_flattening(elem) {
                    return true;
                }
            }
        } else if is_identifier(node) {
            let exported_names = self.get_exports(node);
            if is_export_name(&self.emit_context, node) {
                // The identifier is already wrapped to be an export reference; tolerate up to one
                // matching export.
                return exported_names.len() > 1;
            }
            if exported_names.is_empty() {
                return false;
            }
            // A single direct export whose export name matches the identifier text can be handled
            // natively: substitution will rewrite the identifier to `exports.X`, so no flattening
            // is needed. Re-aliased exports (where the export name differs from the local name) or
            // multi-exported names cannot be expressed natively in a destructuring assignment.
            if exported_names.len() == 1
                && self.is_direct_export(node)
                && exported_names[0].text() == node.text()
            {
                return false;
            }
            return true;
        }
        false
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1484 CommonJSModuleTransformer.createAllExportExpressions
    /// createAllExportExpressions is the callback used during destructuring flattening to create
    /// export expressions for each exported identifier binding.
    fn create_all_export_expressions(
        &mut self,
        name: Node,
        value: Node,
        location: Option<TextRange>,
    ) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let exported_names = self.get_exports(name);
        if !exported_names.is_empty() {
            // If the name is directly exported (i.e., `export let x`), assign to exports.name directly.
            // Otherwise, assign to the local binding first (i.e., `let x; export { x }`).
            let mut expression;
            if self.is_direct_export(name) {
                // Create exports.name = value to handle the direct export assignment,
                // since the Go port doesn't have an onSubstituteNode mechanism to rewrite identifiers.
                let export_name = f.clone_node(name);
                ec.add_emit_flags(
                    export_name,
                    EmitFlags::NO_COMMENTS | EmitFlags::NO_SOURCE_MAP,
                );
                let property_access = f.new_property_access_expression(
                    f.new_identifier("exports"),
                    Node::NIL, /*questionDotToken*/
                    export_name,
                    NodeFlags::NONE,
                );
                ec.add_emit_flags(property_access, EmitFlags::NO_COMMENTS);
                expression = f.new_assignment_expression(property_access, value);
                ec.assign_comment_and_source_map_ranges(expression, name);
            } else {
                expression = f.new_assignment_expression(name, value);
            }
            for export_name in exported_names {
                expression = self.create_export_expression(
                    export_name,
                    expression,
                    location,
                    false, /*liveBinding*/
                );
            }
            return expression;
        }
        // If the identifier is directly exported but has no additional export aliases,
        // still write to exports.name.
        if self.is_direct_export(name) {
            let export_name = f.clone_node(name);
            ec.add_emit_flags(
                export_name,
                EmitFlags::NO_COMMENTS | EmitFlags::NO_SOURCE_MAP,
            );
            let property_access = f.new_property_access_expression(
                f.new_identifier("exports"),
                Node::NIL, /*questionDotToken*/
                export_name,
                NodeFlags::NONE,
            );
            ec.add_emit_flags(property_access, EmitFlags::NO_COMMENTS);
            let result = f.new_assignment_expression(property_access, value);
            ec.assign_comment_and_source_map_ranges(result, name);
            return result;
        }
        f.new_assignment_expression(name, value)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1534 CommonJSModuleTransformer.isDirectExport
    /// isDirectExport checks whether the identifier is directly exported from the source file
    /// (e.g., `export let x` or `export function f()`), as opposed to being re-exported via
    /// `export { x }` for a locally-declared variable.
    fn is_direct_export(&self, name: Node) -> bool {
        let export_container = self.resolver.get_referenced_export_container(
            self.emit_context.most_original(name),
            false, /*prefixLocals*/
        );
        export_container.is_some() && is_source_file(export_container)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1539 CommonJSModuleTransformer.visitAssignmentProperty
    pub(super) fn visit_assignment_property(&mut self, node: Node) -> Node {
        let name = self.visit_node_with(VisitorKind::Root, node.name());
        let initializer = self.visit_node_with(VisitorKind::AssignmentPattern, node.initializer());
        self.emit_context.factory().update_property_assignment(
            node,
            ModifierList::NIL, /*modifiers*/
            name,
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            initializer,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1550 CommonJSModuleTransformer.visitShorthandAssignmentProperty
    pub(super) fn visit_shorthand_assignment_property(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut target = self.visit_destructuring_assignment_target_no_stack(node.name());
        if is_identifier(target) {
            let object_assignment_initializer =
                self.visit_node_with(VisitorKind::Root, node.object_assignment_initializer());
            return f.update_shorthand_property_assignment(
                node,
                ModifierList::NIL, /*modifiers*/
                target,
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                node.equals_token(),
                object_assignment_initializer,
            );
        }
        if node.object_assignment_initializer().is_some() {
            let mut equals_token = node.equals_token();
            if equals_token.is_nil() {
                equals_token = f.new_token(SyntaxKind::EqualsToken);
            }
            let right =
                self.visit_node_with(VisitorKind::Root, node.object_assignment_initializer());
            target = f.new_binary_expression(
                ModifierList::NIL, /*modifiers*/
                target,
                Node::NIL, /*typeNode*/
                equals_token,
                right,
            );
        }
        let updated = f.new_property_assignment(
            ModifierList::NIL, /*modifiers*/
            node.name(),
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            target,
        );
        ec.set_original(updated, node);
        ec.assign_comment_and_source_map_ranges(updated, node);
        updated
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1588 CommonJSModuleTransformer.visitAssignmentRestProperty
    pub(super) fn visit_assignment_rest_property(&mut self, node: Node) -> Node {
        let expression = self.visit_destructuring_assignment_target(node.expression());
        self.emit_context
            .factory()
            .update_spread_assignment(node, expression)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1595 CommonJSModuleTransformer.visitAssignmentRestElement
    pub(super) fn visit_assignment_rest_element(&mut self, node: Node) -> Node {
        let expression = self.visit_destructuring_assignment_target(node.expression());
        self.emit_context
            .factory()
            .update_spread_element(node, expression)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1602 CommonJSModuleTransformer.visitAssignmentElement
    pub(super) fn visit_assignment_element(&mut self, node: Node) -> Node {
        if is_binary_expression(node) {
            let n = node;
            if n.operator_token().kind() == SyntaxKind::EqualsToken {
                let left = self.visit_destructuring_assignment_target(n.left());
                let right = self.visit_node_with(VisitorKind::Root, n.right());
                return self.emit_context.factory().update_binary_expression(
                    n,
                    ModifierList::NIL, /*modifiers*/
                    left,
                    Node::NIL, /*typeNode*/
                    n.operator_token(),
                    right,
                );
            }
        }

        self.visit_destructuring_assignment_target_no_stack(node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1620 CommonJSModuleTransformer.visitDestructuringAssignmentTarget
    fn visit_destructuring_assignment_target(&mut self, node: Node) -> Node {
        let grandparent_node = self.push_node(node);

        let result = match node.kind() {
            SyntaxKind::ObjectLiteralExpression | SyntaxKind::ArrayLiteralExpression => {
                self.visit_assignment_pattern_no_stack(node)
            }
            _ => self.visit_destructuring_assignment_target_no_stack(node),
        };
        self.pop_node(grandparent_node);
        result
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1633 CommonJSModuleTransformer.visitDestructuringAssignmentTargetNoStack
    fn visit_destructuring_assignment_target_no_stack(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        if is_identifier(node)
            && (!is_generated_identifier(&ec, node)
                || is_file_level_reserved_generated_identifier(&ec, node))
            && !is_local_name(&ec, node)
        {
            let f = ec.factory();
            let mut expression = self.visit_expression_identifier(node);
            let exported_names = self.get_exports(node);
            if !exported_names.is_empty() {
                // transforms:
                //  var x;
                //  export { x }
                //  { x: x } = y
                // to:
                //  { x: { set value(v) { exports.x = x = v; } }.value } = y

                let value = f.new_unique_name_ex(
                    "value",
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..Default::default()
                    },
                );
                expression = f.new_assignment_expression(expression, value);

                for export_name in exported_names {
                    expression = self.create_export_expression(
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                }

                let statement = f.new_expression_statement(expression);
                let statement_list = f.new_node_list(&[statement]);
                let param = f.new_parameter_declaration(
                    ModifierList::NIL, /*modifiers*/
                    Node::NIL,         /*dotDotDotToken*/
                    value,
                    Node::NIL, /*questionToken*/
                    Node::NIL, /*type*/
                    Node::NIL, /*initializer*/
                );
                let value_setter = f.new_set_accessor_declaration(
                    ModifierList::NIL, /*modifiers*/
                    f.new_identifier("value"),
                    NodeList::NIL, /*typeParameters*/
                    f.new_node_list(&[param]),
                    Node::NIL, /*returnType*/
                    Node::NIL, /*fullSignature*/
                    f.new_block(statement_list, false /*multiLine*/),
                );
                let property_list = f.new_node_list(&[value_setter]);
                expression =
                    f.new_object_literal_expression(property_list, false /*multiLine*/);
                expression = f.new_property_access_expression(
                    expression,
                    Node::NIL, /*questionDotToken*/
                    f.new_identifier("value"),
                    NodeFlags::NONE,
                );
            }
            return expression;
        }

        self.visit_no_stack(node, false /*resultIsDiscarded*/)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1686 CommonJSModuleTransformer.visitCommaExpression
    /// Visits a comma expression whose left-hand value is always discard, and whose right-hand value may be discarded at runtime.
    fn visit_comma_expression(&mut self, node: Node, result_is_discarded: bool) -> Node {
        let left = self.visit_node_with(VisitorKind::DiscardedValue, node.left());
        let kind = if result_is_discarded {
            VisitorKind::DiscardedValue
        } else {
            VisitorKind::Root
        };
        let right = self.visit_node_with(kind, node.right());
        self.emit_context.factory().update_binary_expression(
            node,
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            node.operator_token(),
            right,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1693 CommonJSModuleTransformer.visitPrefixUnaryExpression
    /// Visits a prefix unary expression that might modify an exported identifier.
    pub(super) fn visit_prefix_unary_expression(
        &mut self,
        node: Node,
        _result_is_discarded: bool,
    ) -> Node {
        // When we see a prefix increment expression whose operand is an exported
        // symbol, we should ensure all exports of that symbol are updated with the correct
        // value.
        //
        // - We do not transform generated identifiers for any reason.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We do not transform identifiers that were originally the name of an enum or
        //   namespace due to how they are transformed in TypeScript.
        // - We only transform identifiers that are exported at the top level.
        let ec = self.emit_context.clone();
        let operand = node.operand();
        if (node.operator() == SyntaxKind::PlusPlusToken
            || node.operator() == SyntaxKind::MinusMinusToken)
            && is_identifier(operand)
            && !is_local_name(&ec, operand)
        {
            let exported_names = self.get_exports(operand);
            if !exported_names.is_empty() {
                // given:
                //   var x = 0;
                //   export { x }
                //   ++x;
                // emits:
                //   var x = 0;
                //   exports.x = x;
                //   exports.x = ++x;
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment.

                let visited_operand = self.visit_node_with(VisitorKind::Root, operand);
                let mut expression = ec.factory().update_prefix_unary_expression(
                    node,
                    node.operator(),
                    visited_operand,
                );
                for export_name in exported_names {
                    expression = self.create_export_expression(
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                    ec.assign_comment_and_source_map_ranges(expression, node);
                }
                return expression;
            }
        }
        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1731 CommonJSModuleTransformer.visitPostfixUnaryExpression
    /// Visits a postfix unary expression that might modify an exported identifier.
    pub(super) fn visit_postfix_unary_expression(
        &mut self,
        node: Node,
        result_is_discarded: bool,
    ) -> Node {
        // When we see a postfix increment expression whose operand is an exported
        // symbol, we should ensure all exports of that symbol are updated with the correct
        // value.
        //
        // - We do not transform generated identifiers for any reason.
        // - We do not transform identifiers tagged with the LocalName flag.
        // - We do not transform identifiers that were originally the name of an enum or
        //   namespace due to how they are transformed in TypeScript.
        // - We only transform identifiers that are exported at the top level.
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let operand = node.operand();
        if (node.operator() == SyntaxKind::PlusPlusToken
            || node.operator() == SyntaxKind::MinusMinusToken)
            && is_identifier(operand)
            && !is_local_name(&ec, operand)
        {
            let exported_names = self.get_exports(operand);
            if !exported_names.is_empty() {
                // given (value is discarded):
                //   var x = 0;
                //   export { x }
                //   x++;
                // emits:
                //   var x = 0, y;
                //   exports.x = x;
                //   exports.x = (x++, x);
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment.
                //
                // given (value is not discarded):
                //   var x = 0, y;
                //   export { x }
                //   y = x++;
                // emits:
                //   var _a;
                //   var x = 0, y;
                //   exports.x = x;
                //   y = (exports.x = (_a = x++, x), _a);
                // note:
                //   after the operation, `exports.x` will hold the value of `x` after the increment, while
                //   `y` will hold the value of `x` before the increment.

                let mut temp = Node::NIL;
                let visited_operand = self.visit_node_with(VisitorKind::Root, operand);
                let mut expression =
                    f.update_postfix_unary_expression(node, visited_operand, node.operator());
                if !result_is_discarded {
                    temp = f.new_temp_variable();
                    ec.add_variable_declaration(temp);

                    expression = f.new_assignment_expression(temp, expression);
                    ec.assign_comment_and_source_map_ranges(expression, node);
                }

                expression = f.new_comma_expression(expression, f.clone_node(operand));
                ec.assign_comment_and_source_map_ranges(expression, node);

                for export_name in exported_names {
                    expression = self.create_export_expression(
                        export_name,
                        expression,
                        None,  /*location*/
                        false, /*liveBinding*/
                    );
                    ec.assign_comment_and_source_map_ranges(expression, node);
                }

                if temp.is_some() {
                    expression = f.new_comma_expression(expression, temp);
                    ec.assign_comment_and_source_map_ranges(expression, node);
                }

                return expression;
            }
        }

        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1806 CommonJSModuleTransformer.visitCallExpression
    /// Visits a call expression that might reference an imported symbol and thus require an indirect call, or that might
    /// be an `import()` or `require()` call that may need to be rewritten.
    pub(super) fn visit_call_expression(&mut self, node: Node) -> Node {
        let mut needs_rewrite = false;
        if self
            .compiler_options
            .rewrite_relative_import_extensions
            .is_true()
            && ((is_import_call(node) && !node.arguments().is_empty())
                || (is_in_js_file(node)
                    && is_require_call(node, false /*requireStringLiteralLikeArgument*/)))
        {
            needs_rewrite = true;
        }
        if node.expression().kind() == SyntaxKind::ImportKeyword
            && self.should_transform_import_call()
        {
            return self.visit_import_call_expression(node, needs_rewrite);
        }
        if needs_rewrite {
            return self.shim_or_rewrite_import_or_require_call(node);
        }
        if is_identifier(node.expression()) {
            // given:
            //   import { f } from "mod";
            //   f();
            // emits:
            //   const mod_1 = require("mod");
            //   (0, mod_1.f)();
            // note:
            //   the indirect call is applied by the printer by way of the `EFIndirectCall` emit flag.
            let ec = self.emit_context.clone();
            let expression = self.visit_expression_identifier(node.expression());
            let arguments = self.visit_nodes_with(VisitorKind::Root, node.argument_list());
            let updated = ec.factory().update_call_expression(
                node,
                expression,
                node.question_dot_token(),
                NodeList::NIL, /*typeArguments*/
                arguments,
                node.flags(),
            );
            if !is_identifier(expression) && !is_helper_name(&ec, node.expression()) {
                ec.add_emit_flags(updated, EmitFlags::INDIRECT_CALL);
            }
            return updated;
        }
        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1842 CommonJSModuleTransformer.shouldTransformImportCall
    fn should_transform_import_call(&self) -> bool {
        should_transform_import_call(
            source_file_file_name(self.current_source_file),
            self.compiler_options,
            (self.get_emit_module_format_of_file)(self.current_source_file),
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1846 CommonJSModuleTransformer.visitImportCallExpression
    fn visit_import_call_expression(&mut self, node: Node, rewrite_or_shim: bool) -> Node {
        if self.module_kind == ModuleKind::NONE && self.language_version >= ScriptTarget::ES2020 {
            return self.visit_each_child_with(VisitorKind::Root, node);
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let external_module_name = get_external_module_name_literal(
            f,
            node,
            self.current_source_file,
            None, /*resolver*/
            self.compiler_options,
        );
        let first = node.arguments().first().unwrap_or(Node::NIL);
        let first_argument = self.visit_node_with(VisitorKind::Root, first);

        // Only use the external module name if it differs from the first argument. This allows us to preserve the quote style of the argument on output.
        let argument = if external_module_name.is_some()
            && (first_argument.is_nil()
                || !is_string_literal(first_argument)
                || first_argument.text() != external_module_name.text())
        {
            external_module_name
        } else if first_argument.is_some() && rewrite_or_shim {
            if is_string_literal(first_argument) {
                rewrite_module_specifier(&ec, first_argument, self.compiler_options)
            } else {
                f.new_rewrite_relative_import_extensions_helper(
                    first_argument,
                    self.compiler_options.jsx == JsxEmit::PRESERVE,
                )
            }
        } else {
            first_argument
        };
        self.create_import_call_expression_common_js(argument)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1870 CommonJSModuleTransformer.createImportCallExpressionCommonJS
    fn create_import_call_expression_common_js(&self, arg: Node) -> Node {
        // import(x)
        // emit as
        // Promise.resolve(`${x}`).then((s) => require(s)) /*CommonJS Require*/
        // We have to wrap require in then callback so that require is done in asynchronously
        // if we simply do require in resolve callback in Promise constructor. We will execute the loading immediately
        // If the arg is not inlineable, we have to evaluate and ToString() it in the current scope
        // Otherwise, we inline it in require() so that it's statically analyzable

        let f = self.emit_context.factory();
        let need_sync_eval = arg.is_some() && !is_simple_inlineable_expression(arg);

        let mut promise_resolve_arguments: Vec<Node> = Vec::new();
        if need_sync_eval {
            promise_resolve_arguments = vec![f.new_template_expression(
                f.new_template_head("", "", TokenFlags::NONE),
                f.new_node_list(&[
                    f.new_template_span(arg, f.new_template_tail("", "", TokenFlags::NONE)),
                ]),
            )];
        }
        let promise_resolve_call = f.new_call_expression(
            f.new_property_access_expression(
                f.new_identifier("Promise"),
                Node::NIL, /*questionDotToken*/
                f.new_identifier("resolve"),
                NodeFlags::NONE,
            ),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            f.new_node_list(&promise_resolve_arguments),
            NodeFlags::NONE,
        );

        let mut require_arguments: Vec<Node> = Vec::new();
        if need_sync_eval {
            require_arguments = vec![f.new_identifier("s")];
        } else if arg.is_some() {
            require_arguments = vec![arg];
        }

        let require_call = f.new_import_star_helper(f.new_call_expression(
            f.new_identifier("require"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            f.new_node_list(&require_arguments),
            NodeFlags::NONE,
        ));

        let mut parameters: Vec<Node> = Vec::new();
        if need_sync_eval {
            parameters = vec![f.new_parameter_declaration(
                ModifierList::NIL, /*modifiers*/
                Node::NIL,         /*dotDotDotToken*/
                f.new_identifier("s"),
                Node::NIL, /*questionToken*/
                Node::NIL, /*type*/
                Node::NIL, /*initializer*/
            )];
        }

        let function = f.new_arrow_function(
            ModifierList::NIL, /*modifiers*/
            NodeList::NIL,     /*typeParameters*/
            f.new_node_list(&parameters),
            Node::NIL,                                       /*type*/
            Node::NIL,                                       /*fullSignature*/
            f.new_token(SyntaxKind::EqualsGreaterThanToken), /*equalsGreaterThanToken*/
            require_call,
        );

        f.new_call_expression(
            f.new_property_access_expression(
                promise_resolve_call,
                Node::NIL, /*questionDotToken*/
                f.new_identifier("then"),
                NodeFlags::NONE,
            ),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            f.new_node_list(&[function]),
            NodeFlags::NONE,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1963 CommonJSModuleTransformer.shimOrRewriteImportOrRequireCall
    fn shim_or_rewrite_import_or_require_call(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let expression = self.visit_node_with(VisitorKind::Root, node.expression());
        let mut arguments_list = node.argument_list();
        let args = node.arguments().to_vec();
        if !args.is_empty() {
            let mut first_argument = self.visit_node_with(VisitorKind::Root, args[0]);
            let first_argument_changed;
            if is_string_literal_like(first_argument) {
                let rewritten =
                    rewrite_module_specifier(&ec, first_argument, self.compiler_options);
                first_argument_changed = rewritten != first_argument;
                first_argument = rewritten;
            } else {
                first_argument = f.new_rewrite_relative_import_extensions_helper(
                    first_argument,
                    self.compiler_options.jsx == JsxEmit::PRESERVE,
                );
                first_argument_changed = true;
            }

            let (rest, rest_changed) = self.visit_slice_with(VisitorKind::Root, &args[1..]);
            if first_argument_changed || rest_changed {
                let mut arguments = vec![first_argument];
                arguments.extend(rest);
                arguments_list = f.new_node_list_with_loc(&arguments, node.argument_list().loc());
            }
        }

        f.update_call_expression(
            node,
            expression,
            node.question_dot_token(),
            NodeList::NIL, /*typeArguments*/
            arguments_list,
            node.flags(),
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:1997 CommonJSModuleTransformer.visitTaggedTemplateExpression
    /// Visits a tagged template expression that might reference an imported symbol and thus require an indirect call.
    pub(super) fn visit_tagged_template_expression(&mut self, node: Node) -> Node {
        if is_identifier(node.tag()) {
            // given:
            //   import { f } from "mod";
            //   f``;
            // emits:
            //   const mod_1 = require("mod");
            //   (0, mod_1.f) ``;
            // note:
            //   the indirect call is applied by the printer by way of the `EFIndirectCall` emit flag.

            let ec = self.emit_context.clone();
            let expression = self.visit_expression_identifier(node.tag());
            let template = self.visit_node_with(VisitorKind::Root, node.template());
            let updated = ec.factory().update_tagged_template_expression(
                node,
                expression,
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                template,
                node.flags(),
            );
            if !is_identifier(expression) && !is_helper_name(&ec, node.tag()) {
                ec.add_emit_flags(updated, EmitFlags::INDIRECT_CALL);
            }
            return updated;
        }
        self.visit_each_child_with(VisitorKind::Root, node)
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:2026 CommonJSModuleTransformer.visitShorthandPropertyAssignment
    /// Visits a shorthand property assignment that might reference an imported or exported symbol.
    pub(super) fn visit_shorthand_property_assignment(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let name = node.name();
        let exported_or_imported_name = self.visit_expression_identifier(name);
        if exported_or_imported_name != name {
            // A shorthand property with an assignment initializer is probably part of a
            // destructuring assignment
            let mut expression = exported_or_imported_name;
            if node.object_assignment_initializer().is_some() {
                let initializer =
                    self.visit_node_with(VisitorKind::Root, node.object_assignment_initializer());
                expression = f.new_assignment_expression(expression, initializer);
            }
            let assignment = f.new_property_assignment(
                ModifierList::NIL, /*modifiers*/
                name,
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                expression,
            );
            set_node_loc(assignment, node.loc());
            ec.assign_comment_and_source_map_ranges(assignment, node);
            return assignment;
        }
        let object_assignment_initializer =
            self.visit_node_with(VisitorKind::Root, node.object_assignment_initializer());
        f.update_shorthand_property_assignment(
            node,
            ModifierList::NIL, /*modifiers*/
            exported_or_imported_name,
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            node.equals_token(),
            object_assignment_initializer,
        )
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:2056 CommonJSModuleTransformer.visitIdentifier
    /// Visits an identifier that, if it is in an expression position, might reference an imported or exported symbol.
    pub(super) fn visit_identifier(&mut self, node: Node) -> Node {
        if is_identifier_reference(node, self.parent_node) {
            return self.visit_expression_identifier(node);
        }
        node
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:2064 CommonJSModuleTransformer.visitExpressionIdentifier
    /// Visits an identifier in an expression position that might reference an imported or exported symbol.
    pub(super) fn visit_expression_identifier(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let info = ec.get_auto_generate_info(node);
        if !info
            .as_ref()
            .is_some_and(|info| !info.flags.has_allow_name_substitution())
            && !is_helper_name(&ec, node)
            && !is_local_name(&ec, node)
            && !is_declaration_name_of_enum_or_namespace(&ec, node)
        {
            let export_container = self
                .resolver
                .get_referenced_export_container(ec.most_original(node), is_export_name(&ec, node));
            if export_container.is_some() && is_source_file(export_container) {
                let reference = f.new_property_access_expression(
                    f.new_identifier("exports"),
                    Node::NIL, /*questionDotToken*/
                    f.clone_node(node),
                    NodeFlags::NONE,
                );
                ec.assign_comment_and_source_map_ranges(reference, node);
                set_node_loc(reference, node.loc());
                return reference;
            }

            let import_declaration = self
                .resolver
                .get_referenced_import_declaration(ec.most_original(node));
            if import_declaration.is_some() {
                if is_import_clause(import_declaration) {
                    let reference = f.new_property_access_expression(
                        f.new_generated_name_for_node(import_declaration.parent()),
                        Node::NIL, /*questionDotToken*/
                        f.new_identifier("default"),
                        NodeFlags::NONE,
                    );
                    ec.assign_comment_and_source_map_ranges(reference, node);
                    set_node_loc(reference, node.loc());
                    return reference;
                }
                if is_import_specifier(import_declaration) {
                    let name = import_declaration.property_name_or_name();
                    let decl = find_ancestor(import_declaration, is_import_declaration);
                    let target = f.new_generated_name_for_node(if decl.is_some() {
                        decl
                    } else {
                        import_declaration
                    });
                    let reference = if is_string_literal(name) {
                        f.new_element_access_expression(
                            target,
                            Node::NIL, /*questionDotToken*/
                            f.new_string_literal_from_node(name),
                            NodeFlags::NONE,
                        )
                    } else {
                        let reference_name = f.clone_node(name);
                        ec.add_emit_flags(
                            reference_name,
                            EmitFlags::NO_SOURCE_MAP | EmitFlags::NO_COMMENTS,
                        );
                        f.new_property_access_expression(
                            target,
                            Node::NIL, /*questionDotToken*/
                            reference_name,
                            NodeFlags::NONE,
                        )
                    };
                    ec.assign_comment_and_source_map_ranges(reference, node);
                    set_node_loc(reference, node.loc());
                    return reference;
                }
            }
        }
        node
    }

    // Go: transformers/moduletransforms/commonjsmodule.go:2127 CommonJSModuleTransformer.getExports
    /// Gets the exported names of an identifier, if it is exported.
    pub(super) fn get_exports(&self, name: Node) -> Vec<Node> {
        let ec = &self.emit_context;
        let info = self.module_info();
        if !is_generated_identifier(ec, name) {
            let import_declaration = self
                .resolver
                .get_referenced_import_declaration(ec.most_original(name));
            if import_declaration.is_some() {
                return info.exported_bindings.get(&import_declaration);
            }

            // An exported namespace or enum may merge with an ambient declaration, which won't show up in .js emit, so
            // we analyze all value exports of a symbol.
            let mut bindings_set: FxHashSet<Node> = FxHashSet::default();
            let mut bindings: Vec<Node> = Vec::new();
            let declarations = self
                .resolver
                .get_referenced_value_declarations(ec.most_original(name));
            // PORT: Go checks `declarations != nil`; an empty Rust vec is Go nil,
            // and both branches then return an empty result.
            for declaration in declarations {
                let exported_bindings = info.exported_bindings.get(&declaration);
                for binding in exported_bindings {
                    if !bindings_set.contains(&binding) {
                        bindings_set.insert(binding);
                        bindings.push(binding);
                    }
                }
            }
            return bindings;
        } else if is_file_level_reserved_generated_identifier(ec, name) {
            let export_specifiers = info.export_specifiers.get(name.text());
            if !export_specifiers.is_empty() {
                let mut exported_names: Vec<Node> = Vec::new();
                for export_specifier in export_specifiers {
                    exported_names.push(export_specifier.name());
                }
                return exported_names;
            }
        }
        Vec::new()
    }
}
