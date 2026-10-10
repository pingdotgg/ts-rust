//! Port of Go `transformers/estransforms/esdecorator.go` lines 1722 to 2718:
//! `visitThisExpression` through `createSymbolMetadataReference`. The first
//! part is in `es_decorator.rs`.

use super::class_fields::expand_pre_or_postfix_increment_or_decrement_expression;
use super::es_decorator::{
    ClassInfoRef, EsDecoratorTransformer, can_ignore_empty_string_literal_in_assigned_name,
    is_anonymous_class_needing_assigned_name,
};
use super::named_evaluation::{is_named_evaluation_and, transform_named_evaluation};
use super::utilities::TxVisitors;
use crate::prelude::*;
use crate::printer::{AutoGenerateOptions, EmitFlags};
use crate::transformers::utilities::{
    get_non_assignment_operator_for_compound_assignment, is_simple_inlineable_expression,
    move_range_past_decorators, move_range_past_modifiers,
};

impl EsDecoratorTransformer {
    /// Go `isNamedEvaluationAnd(tx.EmitContext(), node, isAnonymousClassNeedingAssignedName)`.
    fn is_named_evaluation_of_anonymous_decorated_class(&self, node: Node) -> bool {
        is_named_evaluation_and(
            &self.emit_context,
            node,
            Some(&mut |n: Node| is_anonymous_class_needing_assigned_name(n)),
        )
    }

    // Go: transformers/estransforms/esdecorator.go:1724 esDecoratorTransformer.visitThisExpression
    pub(super) fn visit_this_expression(&mut self, node: Node) -> Node {
        if self.class_this.is_some() {
            return self.class_this;
        }
        node
    }

    // Go: transformers/estransforms/esdecorator.go:1731 esDecoratorTransformer.visitCallExpression
    pub(super) fn visit_call_expression(&mut self, node: Node) -> Node {
        if is_super_property(node.expression()) && self.class_this.is_some() {
            let expression = self.visit_node(node.expression());
            let arguments_list = self.visit_nodes(node.argument_list());
            let ec = self.ec();
            let invocation = ec.factory().new_function_call_call(
                expression,
                self.class_this,
                &arguments_list.nodes().to_vec(),
            );
            ec.set_original(invocation, node);
            set_node_loc(invocation, node.loc());
            return invocation;
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:1744 esDecoratorTransformer.visitTaggedTemplateExpression
    pub(super) fn visit_tagged_template_expression(&mut self, node: Node) -> Node {
        if is_super_property(node.tag()) && self.class_this.is_some() {
            let tag = self.visit_node(node.tag());
            let ec = self.ec();
            let f = ec.factory();
            let bound_tag = f.new_function_bind_call(tag, self.class_this, &[]);
            ec.set_original(bound_tag, node);
            set_node_loc(bound_tag, node.loc());
            let template = self.visit_node(node.template());
            return f.update_tagged_template_expression(
                node,
                bound_tag,
                Node::NIL,
                NodeList::NIL,
                template,
                node.flags(),
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:1757 esDecoratorTransformer.visitPropertyAccessExpression
    pub(super) fn visit_property_access_expression(&mut self, node: Node) -> Node {
        if is_super_property(node)
            && is_identifier(node.name())
            && self.class_this.is_some()
            && self.class_super.is_some()
        {
            let ec = self.ec();
            let f = ec.factory();
            let property_name = f.new_string_literal_from_node(node.name());
            let super_property =
                f.new_reflect_get_call(self.class_super, property_name, self.class_this);
            ec.set_original(super_property, node.expression());
            set_node_loc(super_property, node.expression().loc());
            return super_property;
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:1769 esDecoratorTransformer.visitElementAccessExpression
    pub(super) fn visit_element_access_expression(&mut self, node: Node) -> Node {
        if is_super_property(node) && self.class_this.is_some() && self.class_super.is_some() {
            let property_name = self.visit_node(node.argument_expression());
            let ec = self.ec();
            let super_property =
                ec.factory()
                    .new_reflect_get_call(self.class_super, property_name, self.class_this);
            ec.set_original(super_property, node.expression());
            set_node_loc(super_property, node.expression().loc());
            return super_property;
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:1798 esDecoratorTransformer.visitParameterDeclaration
    // 8.6.3 RS: IteratorBindingInitialization
    //
    //	SingleNameBinding : BindingIdentifier Initializer?
    //	  ...
    //	  5. If |Initializer| is present and _v_ is *undefined*, then
    //	     a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
    //	        i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
    //	  ...
    //
    // 14.3.3.3 RS: KeyedBindingInitialization
    //
    //	SingleNameBinding : BindingIdentifier Initializer?
    //	  ...
    //	  4. If |Initializer| is present and _v_ is *undefined*, then
    //	     a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
    //	        i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _bindingId_.
    //	  ...
    pub(super) fn visit_parameter_declaration(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        let mut param_node = node;
        if self.is_named_evaluation_of_anonymous_decorated_class(param_node) {
            param_node = transform_named_evaluation(
                &ec,
                param_node,
                can_ignore_empty_string_literal_in_assigned_name(param_node.initializer()),
                "",
            );
        }

        let name = self.visit_node(param_node.name());
        let initializer = self.visit_node(param_node.initializer());
        let updated = f.update_parameter_declaration(
            param_node,
            ModifierList::NIL, // modifiers - strip all modifiers (including decorators)
            param_node.dot_dot_dot_token(),
            name,
            Node::NIL, // questionToken
            Node::NIL, // type
            initializer,
        );
        if updated != param_node {
            // While we emit the source map for the node after skipping decorators and modifiers,
            // we need to emit the comments for the original range.
            ec.set_comment_range(updated, param_node.loc());
            let new_loc = move_range_past_modifiers(param_node);
            set_node_loc(updated, new_loc);
            ec.set_source_map_range(updated, new_loc);
            ec.set_emit_flags(updated.name(), EmitFlags::NO_TRAILING_SOURCE_MAP);
        }
        updated
    }

    // Go: transformers/estransforms/esdecorator.go:1870 esDecoratorTransformer.visitNamedEvaluationSite
    /// visitNamedEvaluationSite replaces Strada's visitPropertyAssignment, visitVariableDeclaration,
    /// and visitBindingElement, which all share the same logic.
    pub(super) fn visit_named_evaluation_site(&mut self, mut node: Node, class_expr: Node) -> Node {
        if self.is_named_evaluation_of_anonymous_decorated_class(node) {
            node = transform_named_evaluation(
                &self.ec(),
                node,
                can_ignore_empty_string_literal_in_assigned_name(class_expr),
                "",
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:1893 esDecoratorTransformer.visitForStatement
    pub(super) fn visit_for_statement(&mut self, node: Node) -> Node {
        let initializer = self.discarded_visitor_visit_node(node.initializer());
        let condition = self.visit_node(node.condition());
        let incrementor = self.discarded_visitor_visit_node(node.incrementor());
        let statement = self.visit_iteration_body(node.statement());
        self.ec().factory().update_for_statement(
            node,
            initializer,
            condition,
            incrementor,
            statement,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:1905 esDecoratorTransformer.visitExpressionStatement
    pub(super) fn visit_expression_statement(&mut self, node: Node) -> Node {
        self.with_visitor(Self::discarded_value_visit, |v| v.visit_each_child(node))
    }

    // Go: transformers/estransforms/esdecorator.go:1909 esDecoratorTransformer.visitBinaryExpression
    pub(super) fn visit_binary_expression(&mut self, mut node: Node, discarded: bool) -> Node {
        let ec = self.ec();
        let f = ec.factory();

        if is_destructuring_assignment(node) {
            let left = self.visit_assignment_pattern(node.left());
            let right = self.visit_node(node.right());
            return f.update_binary_expression(
                node,
                ModifierList::NIL,
                left,
                Node::NIL,
                node.operator_token(),
                right,
            );
        }

        if is_assignment_expression(node, false) {
            // 13.15.2 RS: Evaluation
            //   AssignmentExpression : LeftHandSideExpression `=` AssignmentExpression
            //     1. If |LeftHandSideExpression| is neither an |ObjectLiteral| nor an |ArrayLiteral|, then
            //        a. Let _lref_ be ? Evaluation of |LeftHandSideExpression|.
            //        b. If IsAnonymousFunctionDefinition(|AssignmentExpression|) and IsIdentifierRef of |LeftHandSideExpression| are both *true*, then
            //           i. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `&&=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `||=` AssignmentExpression
            //     ...
            //     5. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...
            //
            //   AssignmentExpression : LeftHandSideExpression `??=` AssignmentExpression
            //     ...
            //     4. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true* and IsIdentifierRef of |LeftHandSideExpression| is *true*, then
            //        a. Let _rval_ be ? NamedEvaluation of |AssignmentExpression| with argument _lref_.[[ReferencedName]].
            //     ...

            if self.is_named_evaluation_of_anonymous_decorated_class(node) {
                node = transform_named_evaluation(
                    &ec,
                    node,
                    can_ignore_empty_string_literal_in_assigned_name(node.right()),
                    "",
                );
                return self.visit_each_child(node);
            }

            if is_super_property(node.left())
                && self.class_this.is_some()
                && self.class_super.is_some()
            {
                let mut setter_name = Node::NIL;
                if is_element_access_expression(node.left()) {
                    setter_name = self.visit_node(node.left().argument_expression());
                } else if is_property_access_expression(node.left())
                    && is_identifier(node.left().name())
                {
                    setter_name = f.new_string_literal_from_node(node.left().name());
                }
                if setter_name.is_some() {
                    // super.x = ...
                    // super.x += ...
                    // super[x] = ...
                    // super[x] += ...
                    let mut expression = self.visit_node(node.right());
                    if is_compound_assignment(node.operator_token().kind()) {
                        let mut getter_name = setter_name;
                        if !is_simple_inlineable_expression(setter_name) {
                            getter_name = f.new_temp_variable();
                            ec.add_variable_declaration(getter_name);
                            setter_name = f.new_assignment_expression(getter_name, setter_name);
                        }
                        let super_property_get =
                            f.new_reflect_get_call(self.class_super, getter_name, self.class_this);
                        ec.set_original(super_property_get, node.left());
                        set_node_loc(super_property_get, node.left().loc());
                        expression = f.as_node_factory().new_binary_expression(
                            ModifierList::NIL,
                            super_property_get,
                            Node::NIL,
                            f.new_token(get_non_assignment_operator_for_compound_assignment(
                                node.operator_token().kind(),
                            )),
                            expression,
                        );
                        set_node_loc(expression, node.loc());
                    }
                    let mut temp = Node::NIL;
                    if !discarded {
                        temp = f.new_temp_variable();
                        ec.add_variable_declaration(temp);
                    }
                    if temp.is_some() {
                        expression = f.new_assignment_expression(temp, expression);
                        set_node_loc(expression, node.loc());
                    }
                    expression = f.new_reflect_set_call(
                        self.class_super,
                        setter_name,
                        expression,
                        self.class_this,
                    );
                    ec.set_original(expression, node);
                    set_node_loc(expression, node.loc());
                    if temp.is_some() {
                        expression = f.new_comma_expression(expression, temp);
                        set_node_loc(expression, node.loc());
                    }
                    return expression;
                }
            }
        }

        if node.operator_token().kind() == SyntaxKind::CommaToken {
            let left = self.discarded_visitor_visit_node(node.left());
            let right = if discarded {
                self.discarded_visitor_visit_node(node.right())
            } else {
                self.visit_node(node.right())
            };
            return f.update_binary_expression(
                node,
                ModifierList::NIL,
                left,
                Node::NIL,
                node.operator_token(),
                right,
            );
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2019 esDecoratorTransformer.visitPreOrPostfixUnaryExpression
    pub(super) fn visit_pre_or_postfix_unary_expression(
        &mut self,
        node: Node,
        discarded: bool,
    ) -> Node {
        let ec = self.ec();
        let f = ec.factory();

        let operator = node.operator();
        let operand_node = node.operand();

        if operator == SyntaxKind::PlusPlusToken || operator == SyntaxKind::MinusMinusToken {
            let operand = skip_parentheses(operand_node);
            if is_super_property(operand) && self.class_this.is_some() && self.class_super.is_some()
            {
                let mut setter_name = Node::NIL;
                if is_element_access_expression(operand) {
                    setter_name = self.visit_node(operand.argument_expression());
                } else if is_property_access_expression(operand) && is_identifier(operand.name()) {
                    setter_name = f.new_string_literal_from_node(operand.name());
                }
                if setter_name.is_some() {
                    let mut getter_name = setter_name;
                    if !is_simple_inlineable_expression(setter_name) {
                        getter_name = f.new_temp_variable();
                        ec.add_variable_declaration(getter_name);
                        setter_name = f.new_assignment_expression(getter_name, setter_name);
                    }

                    let mut expression =
                        f.new_reflect_get_call(self.class_super, getter_name, self.class_this);
                    ec.set_original(expression, node);
                    set_node_loc(expression, node.loc());

                    // If the result of this expression is discarded (i.e., it's in a position where the result
                    // will be otherwise unused, such as in an expression statement or the left side of a comma), we
                    // don't need to create an extra temp variable to hold the result:
                    //
                    //  source (discarded):
                    //    super.x++;
                    //  generated:
                    //    _a = Reflect.get(_super, "x"), _a++, Reflect.set(_super, "x", _a);
                    //
                    // Above, the temp variable `_a` is used to perform the correct coercion (i.e., number or
                    // bigint). Since the result of the postfix unary is discarded, we don't need to capture the
                    // result of the expression.
                    //
                    //  source (not discarded):
                    //    y = super.x++;
                    //  generated:
                    //    y = (_a = Reflect.get(_super, "x"), _b = _a++, Reflect.set(_super, "x", _a), _b);
                    //
                    // When the result isn't discarded, we introduce a new temp variable (`_b`) to capture the
                    // result of the operation so that we can provide it to `y` when the assignment is complete.
                    let mut temp = Node::NIL;
                    if !discarded {
                        temp = f.new_temp_variable();
                        ec.add_variable_declaration(temp);
                    }

                    expression = expand_pre_or_postfix_increment_or_decrement_expression(
                        f, &ec, node, expression, temp,
                    );

                    expression = f.new_reflect_set_call(
                        self.class_super,
                        setter_name,
                        expression,
                        self.class_this,
                    );
                    ec.set_original(expression, node);
                    set_node_loc(expression, node.loc());

                    if temp.is_some() {
                        expression = f.new_comma_expression(expression, temp);
                        set_node_loc(expression, node.loc());
                    }

                    return expression;
                }
            }
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2099 esDecoratorTransformer.visitReferencedPropertyName
    /// Returns `(referencedName, updatedName)`.
    pub(super) fn visit_referenced_property_name(&mut self, node: Node) -> (Node, Node) {
        let ec = self.ec();
        let f = ec.factory();
        if is_property_name_literal(node) || is_private_identifier(node) {
            let referenced = f.new_string_literal_from_node(node);
            return (referenced, self.visit_node(node));
        }

        let cpn_expression = node.expression();
        if is_property_name_literal(cpn_expression) && !is_identifier(cpn_expression) {
            let referenced = f.new_string_literal_from_node(cpn_expression);
            return (referenced, self.visit_node(node));
        }

        let referenced_name = f.new_generated_name_for_node(node);
        ec.add_variable_declaration(referenced_name);

        let visited = self.visit_node(cpn_expression);
        let key = f.new_prop_key_helper(visited);
        let assignment = f.new_assignment_expression(referenced_name, key);
        let injected = self.inject_pending_expressions(assignment);
        let updated_name = f.update_computed_property_name(node, injected);
        (referenced_name, updated_name)
    }

    // Go: transformers/estransforms/esdecorator.go:2118 esDecoratorTransformer.visitPropertyName
    pub(super) fn visit_property_name(&mut self, node: Node) -> Node {
        if is_computed_property_name(node) {
            return self.visit_computed_property_name(node);
        }
        self.visit_node(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2125 esDecoratorTransformer.visitComputedPropertyName
    pub(super) fn visit_computed_property_name(&mut self, node: Node) -> Node {
        let mut expression = self.visit_node(node.expression());
        if !is_simple_inlineable_expression(expression) {
            expression = self.inject_pending_expressions(expression);
        }
        self.ec()
            .factory()
            .update_computed_property_name(node, expression)
    }

    // Go: transformers/estransforms/esdecorator.go:2134 esDecoratorTransformer.visitDestructuringAssignmentTarget
    pub(super) fn visit_destructuring_assignment_target(&mut self, node: Node) -> Node {
        if is_object_literal_expression(node) || is_array_literal_expression(node) {
            return self.visit_assignment_pattern(node);
        }

        if is_super_property(node) && self.class_this.is_some() && self.class_super.is_some() {
            let ec = self.ec();
            let f = ec.factory();
            let mut property_name = Node::NIL;
            if is_element_access_expression(node) {
                property_name = self.visit_node(node.argument_expression());
            } else if is_property_access_expression(node) && is_identifier(node.name()) {
                property_name = f.new_string_literal_from_node(node.name());
            }
            if property_name.is_some() {
                let param_name = f.new_temp_variable();
                let expression = f.new_assignment_target_wrapper(
                    param_name,
                    f.new_reflect_set_call(
                        self.class_super,
                        property_name,
                        param_name,
                        self.class_this,
                    ),
                );
                ec.set_original(expression, node);
                set_node_loc(expression, node.loc());
                return expression;
            }
        }

        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2168 esDecoratorTransformer.visitAssignmentElement
    pub(super) fn visit_assignment_element(&mut self, mut node: Node) -> Node {
        // 13.15.5.5 RS: IteratorDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     4. If |Initializer| is present and _value_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _v_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...
        if is_assignment_expression(node, true /*excludeCompoundAssignment*/) {
            if self.is_named_evaluation_of_anonymous_decorated_class(node) {
                node = transform_named_evaluation(
                    &self.ec(),
                    node,
                    can_ignore_empty_string_literal_in_assigned_name(node.right()),
                    "",
                );
            }
            let assignment_target = self.visit_destructuring_assignment_target(node.left());
            let initializer = self.visit_node(node.right());
            return self.ec().factory().update_binary_expression(
                node,
                ModifierList::NIL,
                assignment_target,
                Node::NIL,
                node.operator_token(),
                initializer,
            );
        }
        self.visit_destructuring_assignment_target(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2190 esDecoratorTransformer.visitAssignmentRestElement
    pub(super) fn visit_assignment_rest_element(&mut self, node: Node) -> Node {
        if is_left_hand_side_expression(node.expression()) {
            let expression = self.visit_destructuring_assignment_target(node.expression());
            return self.ec().factory().update_spread_element(node, expression);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2200 esDecoratorTransformer.visitArrayAssignmentElement
    pub(super) fn visit_array_assignment_element(&mut self, node: Node) -> Node {
        go_assert!(node.is_some() && is_expression(node));
        if is_spread_element(node) {
            return self.visit_assignment_rest_element(node);
        }
        if !is_omitted_expression(node) {
            return self.visit_assignment_element(node);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2211 esDecoratorTransformer.visitAssignmentPropertyNode
    pub(super) fn visit_assignment_property_node(&mut self, node: Node) -> Node {
        // AssignmentProperty : PropertyName `:` AssignmentElement
        // AssignmentElement : DestructuringAssignmentTarget Initializer?

        // 13.15.5.6 RS: KeyedDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     3. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousfunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _rhsValue_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...

        let name = self.visit_node(node.name());
        let initializer = node.initializer();
        if is_assignment_expression(initializer, true /*excludeCompoundAssignment*/) {
            let assignment_element = self.visit_assignment_element(initializer);
            return self.ec().factory().update_property_assignment(
                node,
                ModifierList::NIL,
                name,
                Node::NIL,
                Node::NIL,
                assignment_element,
            );
        }
        if is_left_hand_side_expression(initializer) {
            let assignment_element = self.visit_destructuring_assignment_target(initializer);
            return self.ec().factory().update_property_assignment(
                node,
                ModifierList::NIL,
                name,
                Node::NIL,
                Node::NIL,
                assignment_element,
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2237 esDecoratorTransformer.visitShorthandAssignmentProperty
    pub(super) fn visit_shorthand_assignment_property(&mut self, mut node: Node) -> Node {
        // AssignmentProperty : IdentifierReference Initializer?

        // 13.15.5.3 RS: PropertyDestructuringAssignmentEvaluation
        //   AssignmentProperty : IdentifierReference Initializer?
        //     ...
        //     4. If |Initializer?| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _P_.
        //     ...
        if self.is_named_evaluation_of_anonymous_decorated_class(node) {
            node = transform_named_evaluation(
                &self.ec(),
                node,
                can_ignore_empty_string_literal_in_assigned_name(
                    node.object_assignment_initializer(),
                ),
                "",
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2253 esDecoratorTransformer.visitAssignmentRestProperty
    pub(super) fn visit_assignment_rest_property(&mut self, node: Node) -> Node {
        if is_left_hand_side_expression(node.expression()) {
            let expression = self.visit_destructuring_assignment_target(node.expression());
            return self
                .ec()
                .factory()
                .update_spread_assignment(node, expression);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2263 esDecoratorTransformer.visitObjectAssignmentElement
    pub(super) fn visit_object_assignment_element(&mut self, node: Node) -> Node {
        go_assert!(node.is_some() && is_object_literal_element(node));
        if is_spread_assignment(node) {
            return self.visit_assignment_rest_property(node);
        }
        if is_shorthand_property_assignment(node) {
            return self.visit_shorthand_assignment_property(node);
        }
        if is_property_assignment(node) {
            return self.visit_assignment_property_node(node);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/esdecorator.go:2277 esDecoratorTransformer.visitAssignmentPattern
    pub(super) fn visit_assignment_pattern(&mut self, node: Node) -> Node {
        if is_array_literal_expression(node) {
            let elements = self.with_visitor(Self::visit_array_assignment_element, |v| {
                v.visit_nodes(node.element_list())
            });
            return self.ec().factory().update_array_literal_expression(
                node,
                elements,
                node.multi_line(),
            );
        }
        let properties = self.with_visitor(Self::visit_object_assignment_element, |v| {
            v.visit_nodes(node.property_list())
        });
        self.ec()
            .factory()
            .update_object_literal_expression(node, properties, node.multi_line())
    }

    // Go: transformers/estransforms/esdecorator.go:2289 esDecoratorTransformer.visitExportAssignment
    pub(super) fn visit_export_assignment(&mut self, node: Node) -> Node {
        // 16.2.3.7 RS: Evaluation
        //   ExportDeclaration : `export` `default` AssignmentExpression `;`
        //     1. If IsAnonymousFunctionDefinition(|AssignmentExpression|) is *true*, then
        //        a. Let _value_ be ? NamedEvaluation of |AssignmentExpression| with argument `"default"`.
        //     ...
        self.visit_named_evaluation_site(node, node.expression())
    }

    // Go: transformers/estransforms/esdecorator.go:2298 esDecoratorTransformer.visitParenthesizedExpression
    pub(super) fn visit_parenthesized_expression(&mut self, node: Node, discarded: bool) -> Node {
        // 8.4.5 RS: NamedEvaluation
        //   ParenthesizedExpression : `(` Expression `)`
        //     ...
        //     2. Return ? NamedEvaluation of |Expression| with argument _name_.

        let expression = if discarded {
            self.discarded_visitor_visit_node(node.expression())
        } else {
            self.visit_node(node.expression())
        };
        self.ec()
            .factory()
            .update_parenthesized_expression(node, expression)
    }

    // Go: transformers/estransforms/esdecorator.go:2315 esDecoratorTransformer.visitPartiallyEmittedExpression
    pub(super) fn visit_partially_emitted_expression(
        &mut self,
        node: Node,
        discarded: bool,
    ) -> Node {
        // Emulates 8.4.5 RS: NamedEvaluation
        let expression = if discarded {
            self.discarded_visitor_visit_node(node.expression())
        } else {
            self.visit_node(node.expression())
        };
        self.ec()
            .factory()
            .update_partially_emitted_expression(node, expression)
    }

    // Go: transformers/estransforms/esdecorator.go:2329 esDecoratorTransformer.prependExpressions
    /// prependExpressions prepends a list of expressions before a target expression, preserving
    /// parenthesization. If expression is nil, the pending expressions are inlined alone.
    pub(super) fn prepend_expressions(&self, pending: &[Node], expression: Node) -> Node {
        let f = self.emit_context.factory();
        if pending.is_empty() {
            return expression;
        }
        if expression.is_nil() {
            return f.inline_expressions(pending);
        }
        if is_parenthesized_expression(expression) {
            let mut exprs: Vec<Node> = pending.to_vec();
            exprs.push(expression.expression());
            return f.update_parenthesized_expression(expression, f.inline_expressions(&exprs));
        }
        let mut exprs: Vec<Node> = pending.to_vec();
        exprs.push(expression);
        f.inline_expressions(&exprs)
    }

    // Go: transformers/estransforms/esdecorator.go:2350 esDecoratorTransformer.injectPendingExpressions
    pub(super) fn inject_pending_expressions(&mut self, expression: Node) -> Node {
        let result = self.prepend_expressions(&self.pending_expressions, expression);
        go_assert!(result.is_some());
        if result != expression {
            self.pending_expressions = Vec::new();
        }
        result
    }

    // Go: transformers/estransforms/esdecorator.go:2359 esDecoratorTransformer.injectPendingInitializers
    pub(super) fn inject_pending_initializers(
        &self,
        ci: &ClassInfoRef,
        is_static: bool,
        expression: Node,
    ) -> Node {
        let pending = if is_static {
            ci.borrow().pending_static_initializers.clone()
        } else {
            ci.borrow().pending_instance_initializers.clone()
        };
        let result = self.prepend_expressions(&pending, expression);
        if result != expression {
            let mut ci = ci.borrow_mut();
            if is_static {
                ci.pending_static_initializers = Vec::new();
            } else {
                ci.pending_instance_initializers = Vec::new();
            }
        }
        result
    }

    // Go: transformers/estransforms/esdecorator.go:2374 esDecoratorTransformer.transformAllDecoratorsOfDeclaration
    /// Transforms all of the decorators for a declaration into an array of expressions.
    pub(super) fn transform_all_decorators_of_declaration(
        &mut self,
        decorators: &[Node],
    ) -> Vec<Node> {
        if decorators.is_empty() {
            return Vec::new();
        }
        let mut result: Vec<Node> = Vec::with_capacity(decorators.len());
        for &d in decorators {
            result.push(self.transform_decorator(d));
        }
        result
    }

    // Go: transformers/estransforms/esdecorator.go:2386 esDecoratorTransformer.transformDecorator
    /// Transforms a decorator into an expression.
    pub(super) fn transform_decorator(&mut self, decorator: Node) -> Node {
        let expression = self.visit_node(decorator.expression());
        let ec = self.ec();
        let f = ec.factory();
        ec.set_emit_flags(expression, EmitFlags::NO_COMMENTS);

        // preserve the 'this' binding for an access expression
        let inner_expression = skip_outer_expressions(expression, OuterExpressionKinds::OEK_ALL);
        if is_access_expression(inner_expression) {
            let (target, this_arg) = self.create_call_binding(expression);
            let bind_call = f.new_function_bind_call(target, this_arg, &[]);
            return f.restore_outer_expressions(
                expression,
                bind_call,
                OuterExpressionKinds::OEK_ALL,
            );
        }
        expression
    }

    // Go: transformers/estransforms/esdecorator.go:2400 esDecoratorTransformer.createCallBinding
    /// Returns `(target, thisArg)`.
    pub(super) fn create_call_binding(&self, expression: Node) -> (Node, Node) {
        let ec = &self.emit_context;
        let f = ec.factory();
        let callee = skip_outer_expressions(expression, OuterExpressionKinds::OEK_ALL);
        if is_super_property(callee) {
            return (callee, f.new_this_expression());
        }
        if callee.kind() == SyntaxKind::SuperKeyword {
            return (callee, f.new_this_expression());
        }
        if ec.emit_flags(callee).intersects(EmitFlags::HELPER_NAME) {
            return (callee, f.new_void_zero_expression());
        }
        if is_property_access_expression(callee) {
            if self.should_be_captured_in_temp_variable(callee.expression()) {
                let this_arg = f.new_temp_variable();
                ec.add_variable_declaration(this_arg);
                let assign = f.new_assignment_expression(this_arg, callee.expression());
                set_node_loc(assign, callee.expression().loc());
                let target = f.new_property_access_expression(
                    assign,
                    Node::NIL,
                    callee.name(),
                    NodeFlags::NONE,
                );
                set_node_loc(target, callee.loc());
                return (target, this_arg);
            }
            return (callee, callee.expression());
        }
        if is_element_access_expression(callee) {
            if self.should_be_captured_in_temp_variable(callee.expression()) {
                let this_arg = f.new_temp_variable();
                ec.add_variable_declaration(this_arg);
                let assign = f.new_assignment_expression(this_arg, callee.expression());
                set_node_loc(assign, callee.expression().loc());
                let target = f.new_element_access_expression(
                    assign,
                    Node::NIL,
                    callee.argument_expression(),
                    NodeFlags::NONE,
                );
                set_node_loc(target, callee.loc());
                return (target, this_arg);
            }
            return (callee, callee.expression());
        }
        (expression, f.new_void_zero_expression())
    }

    // Go: transformers/estransforms/esdecorator.go:2441 esDecoratorTransformer.shouldBeCapturedInTempVariable
    pub(super) fn should_be_captured_in_temp_variable(&self, node: Node) -> bool {
        // This is a simplified version of the general shouldBeCapturedInTempVariable from
        // nodeFactory with cacheIdentifiers=true, since createCallBinding in this transform
        // always caches identifiers.
        let target = skip_parentheses(node);
        match target.kind() {
            // cacheIdentifiers is always true for this transform's createCallBinding
            SyntaxKind::Identifier => true,
            SyntaxKind::ThisKeyword
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral => false,
            _ => true,
        }
    }

    // Go: transformers/estransforms/esdecorator.go:2462 esDecoratorTransformer.createDescriptorMethod
    /// Creates a "value", "get", or "set" method for a pseudo-PropertyDescriptor object created for
    /// a private element.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_descriptor_method(
        &self,
        original: Node,
        name: Node, // PrivateIdentifier
        modifiers: ModifierList,
        asterisk_token: Node,
        kind: &str,
        parameters: NodeList,
        mut body: Node,
    ) -> Node {
        let ec = &self.emit_context;
        let f = ec.factory();

        if body.is_nil() {
            body = f.new_block(f.new_node_list(&[]), false);
        }

        let func_expr = f.new_function_expression(
            modifiers,
            asterisk_token,
            Node::NIL,     // name
            NodeList::NIL, // typeParameters
            parameters,
            Node::NIL, // type
            Node::NIL, // fullSignature
            body,
        );
        ec.set_original(func_expr, original);
        ec.set_source_map_range(func_expr, move_range_past_decorators(original));
        ec.set_emit_flags(func_expr, EmitFlags::NO_COMMENTS);

        let prefix = if kind == "get" || kind == "set" {
            kind
        } else {
            ""
        };
        let function_name = f.new_string_literal_from_node(name);
        let named_function = f.new_set_function_name_helper(func_expr, function_name, prefix);

        let method = f.new_property_assignment(
            ModifierList::NIL,
            f.new_identifier(kind),
            Node::NIL,
            Node::NIL,
            named_function,
        );
        ec.set_original(method, original);
        ec.set_source_map_range(method, move_range_past_decorators(original));
        ec.set_emit_flags(method, EmitFlags::NO_COMMENTS);
        method
    }

    // Go: transformers/estransforms/esdecorator.go:2507 esDecoratorTransformer.createMethodDescriptorObject
    /// Creates a pseudo-PropertyDescriptor object used when decorating a private MethodDeclaration.
    pub(super) fn create_method_descriptor_object(
        &mut self,
        member: Node,
        modifiers: ModifierList,
    ) -> Node {
        let parameters = self.visit_nodes(member.parameter_list());
        let body = self.visit_node(member.body());
        let f = self.emit_context.factory();
        f.new_object_literal_expression(
            f.new_node_list(&[self.create_descriptor_method(
                member,
                member.name(),
                modifiers,
                member.asterisk_token(),
                "value",
                parameters,
                body,
            )]),
            false,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2521 esDecoratorTransformer.createGetAccessorDescriptorObject
    /// Creates a pseudo-PropertyDescriptor object used when decorating a private GetAccessor.
    pub(super) fn create_get_accessor_descriptor_object(
        &mut self,
        member: Node,
        modifiers: ModifierList,
    ) -> Node {
        let body = self.visit_node(member.body());
        let f = self.emit_context.factory();
        f.new_object_literal_expression(
            f.new_node_list(&[self.create_descriptor_method(
                member,
                member.name(),
                modifiers,
                Node::NIL,
                "get",
                f.new_node_list(&[]),
                body,
            )]),
            false,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2533 esDecoratorTransformer.createSetAccessorDescriptorObject
    /// Creates a pseudo-PropertyDescriptor object used when decorating a private SetAccessor.
    pub(super) fn create_set_accessor_descriptor_object(
        &mut self,
        member: Node,
        modifiers: ModifierList,
    ) -> Node {
        let parameters = self.visit_nodes(member.parameter_list());
        let body = self.visit_node(member.body());
        let f = self.emit_context.factory();
        f.new_object_literal_expression(
            f.new_node_list(&[self.create_descriptor_method(
                member,
                member.name(),
                modifiers,
                Node::NIL,
                "set",
                parameters,
                body,
            )]),
            false,
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2547 esDecoratorTransformer.createAccessorPropertyDescriptorObject
    /// Creates a pseudo-PropertyDescriptor object used when decorating a private auto-accessor PropertyDeclaration.
    /// The descriptor contains get/set methods that access the generated backing field.
    pub(super) fn create_accessor_property_descriptor_object(
        &mut self,
        member: Node,
        _modifiers: ModifierList,
    ) -> Node {
        //  {
        //      get() { return this.${privateName}; },
        //      set(value) { this.${privateName} = value; },
        //  }
        let f = self.emit_context.factory();
        let backing_field_name = f.new_generated_private_name_for_node_ex(
            member.name(),
            AutoGenerateOptions {
                suffix: "_accessor_storage".to_string(),
                ..Default::default()
            },
        );
        f.new_object_literal_expression(
            f.new_node_list(&[
                self.create_descriptor_method(
                    member,
                    member.name(),
                    ModifierList::NIL,
                    Node::NIL,
                    "get",
                    f.new_node_list(&[]),
                    f.new_block(
                        f.new_node_list(&[f.new_return_statement(
                            f.new_property_access_expression(
                                f.new_this_expression(),
                                Node::NIL,
                                backing_field_name,
                                NodeFlags::NONE,
                            ),
                        )]),
                        false,
                    ),
                ),
                self.create_descriptor_method(
                    member,
                    member.name(),
                    ModifierList::NIL,
                    Node::NIL,
                    "set",
                    f.new_node_list(&[f.new_parameter_declaration(
                        ModifierList::NIL,
                        Node::NIL,
                        f.new_identifier("value"),
                        Node::NIL,
                        Node::NIL,
                        Node::NIL,
                    )]),
                    f.new_block(
                        f.new_node_list(&[f.new_expression_statement(
                            f.new_assignment_expression(
                                f.new_property_access_expression(
                                    f.new_this_expression(),
                                    Node::NIL,
                                    backing_field_name,
                                    NodeFlags::NONE,
                                ),
                                f.new_identifier("value"),
                            ),
                        )]),
                        false,
                    ),
                ),
            ]),
            false,
        )
    }

    /// Go `tx.staticOnlyModifierVisitor.VisitModifiers(modifiers)`.
    fn static_only_modifiers(&mut self, modifiers: ModifierList) -> ModifierList {
        self.with_visitor(Self::static_only_modifier_visit, |v| {
            v.visit_modifiers(modifiers)
        })
    }

    // Go: transformers/estransforms/esdecorator.go:2585 esDecoratorTransformer.createMethodDescriptorForwarder
    /// Creates a MethodDeclaration that forwards its invocation to a PropertyDescriptor object.
    pub(super) fn create_method_descriptor_forwarder(
        &mut self,
        modifiers: ModifierList,
        name: Node,
        descriptor_name: Node,
    ) -> Node {
        let static_only = self.static_only_modifiers(modifiers);
        let f = self.emit_context.factory();
        f.new_get_accessor_declaration(
            static_only,
            name,
            NodeList::NIL, // typeParameters
            f.new_node_list(&[]),
            Node::NIL, // type
            Node::NIL, // fullSignature
            f.new_block(
                f.new_node_list(&[f.new_return_statement(f.new_property_access_expression(
                    descriptor_name,
                    Node::NIL,
                    f.new_identifier("value"),
                    NodeFlags::NONE,
                ))]),
                false,
            ),
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2604 esDecoratorTransformer.createGetAccessorDescriptorForwarder
    /// Creates a GetAccessor that forwards its invocation to a PropertyDescriptor object.
    pub(super) fn create_get_accessor_descriptor_forwarder(
        &mut self,
        modifiers: ModifierList,
        name: Node,
        descriptor_name: Node,
    ) -> Node {
        let static_only = self.static_only_modifiers(modifiers);
        let f = self.emit_context.factory();
        f.new_get_accessor_declaration(
            static_only,
            name,
            NodeList::NIL, // typeParameters
            f.new_node_list(&[]),
            Node::NIL, // type
            Node::NIL, // fullSignature
            f.new_block(
                f.new_node_list(&[f.new_return_statement(f.new_function_call_call(
                    f.new_property_access_expression(
                        descriptor_name,
                        Node::NIL,
                        f.new_identifier("get"),
                        NodeFlags::NONE,
                    ),
                    f.new_this_expression(),
                    &[],
                ))]),
                false,
            ),
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2627 esDecoratorTransformer.createSetAccessorDescriptorForwarder
    /// Creates a SetAccessor that forwards its invocation to a PropertyDescriptor object.
    pub(super) fn create_set_accessor_descriptor_forwarder(
        &mut self,
        modifiers: ModifierList,
        name: Node,
        descriptor_name: Node,
    ) -> Node {
        let static_only = self.static_only_modifiers(modifiers);
        let f = self.emit_context.factory();
        f.new_set_accessor_declaration(
            static_only,
            name,
            NodeList::NIL, // typeParameters
            f.new_node_list(&[f.new_parameter_declaration(
                ModifierList::NIL,
                Node::NIL,
                f.new_identifier("value"),
                Node::NIL,
                Node::NIL,
                Node::NIL,
            )]),
            Node::NIL, // type
            Node::NIL, // fullSignature
            f.new_block(
                f.new_node_list(&[f.new_return_statement(f.new_function_call_call(
                    f.new_property_access_expression(
                        descriptor_name,
                        Node::NIL,
                        f.new_identifier("set"),
                        NodeFlags::NONE,
                    ),
                    f.new_this_expression(),
                    &[f.new_identifier("value")],
                ))]),
                false,
            ),
        )
    }

    // Go: transformers/estransforms/esdecorator.go:2651 esDecoratorTransformer.createMetadata
    pub(super) fn create_metadata(&self, name: Node, class_super: Node) -> Node {
        let f = self.emit_context.factory();

        let super_metadata = if class_super.is_some() {
            self.create_symbol_metadata_reference(class_super)
        } else {
            f.new_token(SyntaxKind::NullKeyword)
        };

        let object_create = f.new_call_expression(
            f.new_property_access_expression(
                f.new_identifier("Object"),
                Node::NIL,
                f.new_identifier("create"),
                NodeFlags::NONE,
            ),
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[super_metadata]),
            NodeFlags::NONE,
        );

        let symbol_check = f.new_logical_and_expression(
            f.new_type_check(f.new_identifier("Symbol"), "function"),
            f.new_property_access_expression(
                f.new_identifier("Symbol"),
                Node::NIL,
                f.new_identifier("metadata"),
                NodeFlags::NONE,
            ),
        );

        let conditional = f.new_conditional_expression(
            symbol_check,
            f.new_token(SyntaxKind::QuestionToken),
            object_create,
            f.new_token(SyntaxKind::ColonToken),
            f.new_void_zero_expression(),
        );

        let var_decl = f.new_variable_declaration(name, Node::NIL, Node::NIL, conditional);
        let var_decl_list =
            f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST);
        f.new_variable_statement(ModifierList::NIL, var_decl_list)
    }

    // Go: transformers/estransforms/esdecorator.go:2686 esDecoratorTransformer.createSymbolMetadata
    pub(super) fn create_symbol_metadata(&self, target: Node, value: Node) -> Node {
        let ec = &self.emit_context;
        let f = ec.factory();

        // Object.defineProperty(target, Symbol.metadata, { configurable: true, writable: true, enumerable: true, value })
        let symbol_metadata = f.new_property_access_expression(
            f.new_identifier("Symbol"),
            Node::NIL,
            f.new_identifier("metadata"),
            NodeFlags::NONE,
        );

        let descriptor_props = [
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("enumerable"),
                Node::NIL,
                Node::NIL,
                f.new_true_expression(),
            ),
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("configurable"),
                Node::NIL,
                Node::NIL,
                f.new_true_expression(),
            ),
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("writable"),
                Node::NIL,
                Node::NIL,
                f.new_true_expression(),
            ),
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("value"),
                Node::NIL,
                Node::NIL,
                value,
            ),
        ];
        let descriptor = f.new_object_literal_expression(f.new_node_list(&descriptor_props), false);

        let define_property = f.new_call_expression(
            f.new_property_access_expression(
                f.new_identifier("Object"),
                Node::NIL,
                f.new_identifier("defineProperty"),
                NodeFlags::NONE,
            ),
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[target, symbol_metadata, descriptor]),
            NodeFlags::NONE,
        );

        let if_statement = f.new_if_statement(
            value,
            f.new_expression_statement(define_property),
            Node::NIL,
        );
        ec.set_emit_flags(if_statement, EmitFlags::SINGLE_LINE);
        if_statement
    }

    // Go: transformers/estransforms/esdecorator.go:2712 esDecoratorTransformer.createSymbolMetadataReference
    pub(super) fn create_symbol_metadata_reference(&self, class_super: Node) -> Node {
        let f = self.emit_context.factory();
        let symbol_metadata = f.new_property_access_expression(
            f.new_identifier("Symbol"),
            Node::NIL,
            f.new_identifier("metadata"),
            NodeFlags::NONE,
        );
        let element_access = f.new_element_access_expression(
            class_super,
            Node::NIL,
            symbol_metadata,
            NodeFlags::NONE,
        );
        f.new_binary_expression(
            ModifierList::NIL,
            element_access,
            Node::NIL,
            f.new_token(SyntaxKind::QuestionQuestionToken),
            f.new_token(SyntaxKind::NullKeyword),
        )
    }
}
