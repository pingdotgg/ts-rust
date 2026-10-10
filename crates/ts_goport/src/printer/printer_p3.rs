//! Go `printer/printer.go` lines 2455 to 3689: expressions, misc and
//! statements. The `Printer` struct is in `printer_p1.rs`.

use crate::gostd::debug::kind_string;
use crate::prelude::*;

// PORT: Go uses local `printerState`, `tokenEmitFlags`, `ListFormat`,
// `WriteKind`, `EmitFlags`, `getLiteralTextFlags`, `SynthesizedComment` and
// `ast.CommentRange` from the other printer units. Their Rust names are
// `PrinterState`, `TokenEmitFlags::X`, `ListFormat::X`, `WriteKind::X`,
// `EmitFlags::X`, `GetLiteralTextFlags::X`, `SynthesizedComment` and
// `CommentRange` (Go constant prefixes `tef`, `LF`, `EF` and `WriteKind` are
// dropped, as for `TypeFlagsAny` -> `TypeFlags::ANY`).

//
// Expressions
//

impl Printer {
    // Go: printer/printer.go:2475 emitKeywordExpression
    pub(crate) fn emit_keyword_expression(&mut self, node: Node) {
        self.emit_keyword_node(node);
    }

    // Go: printer/printer.go:2479 emitArrayLiteralExpressionElement
    pub(crate) fn emit_array_literal_expression_element(&mut self, node: Node) {
        self.emit_expression(node, OperatorPrecedence::SPREAD);
    }

    // Go: printer/printer.go:2483 emitArrayLiteralExpression
    pub(crate) fn emit_array_literal_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_list(
            Printer::emit_array_literal_expression_element,
            node,
            node.element_list(),
            ListFormat::ARRAY_LITERAL_EXPRESSION_ELEMENTS
                | if node.multi_line() {
                    ListFormat::PREFER_NEW_LINE
                } else {
                    ListFormat::NONE
                },
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2489 emitObjectLiteralExpression
    pub(crate) fn emit_object_literal_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(node);
        self.generate_all_member_names(node.property_list());
        let format = ListFormat::OBJECT_LITERAL_EXPRESSION_PROPERTIES
            | if node.multi_line() {
                ListFormat::PREFER_NEW_LINE
            } else {
                ListFormat::NONE
            }
            | if self.should_allow_trailing_comma(node, node.property_list()) {
                ListFormat::ALLOW_TRAILING_COMMA
            } else {
                ListFormat::NONE
            };
        self.emit_list(
            Printer::emit_object_literal_element,
            node,
            node.property_list(),
            format,
        );
        self.pop_name_generation_scope(node);
        self.decrease_indent_if(indented);
        self.exit_node(node, state);
    }

    // 1..toString is a valid property access, emit a dot after the literal
    // Also emit a dot if expression is a integer const enum value - it will appear in generated code as numeric literal
    // Go: printer/printer.go:2505 mayNeedDotDotForPropertyAccess
    pub(crate) fn may_need_dot_dot_for_property_access(&mut self, expression: Node) -> bool {
        let expression = skip_partially_emitted_expressions(expression);
        if is_numeric_literal(expression) {
            // check if numeric literal is a decimal literal that was originally written with a dot
            let text = self.get_literal_text_of_node(
                expression,
                Node::NIL, /*sourceFile*/
                GetLiteralTextFlags::NEVER_ASCII_ESCAPE,
            );
            // If the number will be printed verbatim and it doesn't already contain a dot or an exponent indicator, add one
            // if the expression doesn't have any comments that will be emitted.
            return !expression
                .token_flags()
                .intersects(TokenFlags::WITH_SPECIFIER)
                && !text.contains(token_to_string(SyntaxKind::DotToken))
                && !text.contains('E')
                && !text.contains('e');
        }
        false
    }

    // Go: printer/printer.go:2520 emitPropertyAccessExpression
    pub(crate) fn emit_property_access_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(
            node.expression(),
            if is_optional_chain(node) {
                OperatorPrecedence::OPTIONAL_CHAIN
            } else {
                OperatorPrecedence::MEMBER
            },
        );
        let mut token = node.question_dot_token();
        if token.is_nil() {
            token = self.emit_context.factory.new_token(SyntaxKind::DotToken);
            set_node_loc(
                token,
                TextRange::new(node.expression().end(), node.name().pos()),
            );
            self.emit_context
                .add_emit_flags(token, EmitFlags::NO_SOURCE_MAP);
        }
        let lines_before_dot = self.get_lines_between_nodes(node, node.expression(), token);
        self.write_line_repeat(lines_before_dot);
        self.increase_indent_if(lines_before_dot > 0);
        let should_emit_dot_dot = token.kind() != SyntaxKind::QuestionDotToken
            && self.may_need_dot_dot_for_property_access(node.expression())
            && !self.writer().has_trailing_comment()
            && !self.writer().has_trailing_whitespace();
        if should_emit_dot_dot {
            self.write_punctuation(".");
        }
        if node.question_dot_token().is_some() {
            self.emit_token_node(token);
        } else {
            self.emit_token(
                SyntaxKind::DotToken,
                node.expression().end(),
                WriteKind::PUNCTUATION,
                node,
            );
        }
        let lines_after_dot = self.get_lines_between_nodes(node, token, node.name());
        self.write_line_repeat(lines_after_dot);
        self.increase_indent_if(lines_after_dot > 0);
        self.emit_member_name(node.name());
        self.decrease_indent_if(lines_after_dot > 0);
        self.decrease_indent_if(lines_before_dot > 0);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2553 emitElementAccessExpression
    pub(crate) fn emit_element_access_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(
            node.expression(),
            if is_optional_chain(node) {
                OperatorPrecedence::OPTIONAL_CHAIN
            } else {
                OperatorPrecedence::MEMBER
            },
        );
        self.emit_token_node(node.question_dot_token());
        self.emit_token(
            SyntaxKind::OpenBracketToken,
            greatest_end(-1, &[&node.expression(), &node.question_dot_token()]),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(node.argument_expression(), OperatorPrecedence::COMMA);
        self.emit_token(
            SyntaxKind::CloseBracketToken,
            node.argument_expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2563 emitArgument
    pub(crate) fn emit_argument(&mut self, node: Node) {
        self.emit_expression(node, OperatorPrecedence::SPREAD);
    }

    // Go: printer/printer.go:2567 emitCallee
    pub(crate) fn emit_callee(&mut self, callee: Node, parent_node: Node) {
        if self.should_emit_indirect_call(parent_node) {
            self.write_punctuation("(");
            self.write_literal("0");
            self.write_punctuation(",");
            self.write_space();
            self.emit_expression(callee, OperatorPrecedence::COMMA);
            self.write_punctuation(")");
        } else if parent_node.kind() == SyntaxKind::CallExpression
            && is_new_expression_without_arguments(skip_partially_emitted_expressions(callee))
        {
            // Parenthesize `new C` inside of a CallExpression so it is treated as `(new C)()` and not `new C()`
            self.emit_expression(callee, OperatorPrecedence::PARENTHESES);
        } else {
            self.emit_expression(
                callee,
                if is_optional_chain(parent_node) {
                    OperatorPrecedence::OPTIONAL_CHAIN
                } else {
                    OperatorPrecedence::MEMBER
                },
            );
        }
    }

    // Go: printer/printer.go:2583 emitCallExpression
    pub(crate) fn emit_call_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_callee(node.expression(), node);
        self.emit_token_node(node.question_dot_token());
        self.emit_type_arguments(node, node.type_argument_list());
        self.emit_list(
            Printer::emit_argument,
            node,
            node.argument_list(),
            ListFormat::CALL_EXPRESSION_ARGUMENTS,
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2592 emitNewExpression
    pub(crate) fn emit_new_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(SyntaxKind::NewKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        if skip_partially_emitted_expressions(node.expression()).kind()
            == SyntaxKind::CallExpression
        {
            // Parenthesize `C()` inside of a NewExpression so it is treated as `new (C())` and not `new C()`
            self.emit_expression(node.expression(), OperatorPrecedence::PARENTHESES);
        } else {
            self.emit_expression(node.expression(), OperatorPrecedence::MEMBER);
        }
        self.emit_type_arguments(node, node.type_argument_list());
        self.emit_list(
            Printer::emit_argument,
            node,
            node.argument_list(),
            ListFormat::NEW_EXPRESSION_ARGUMENTS,
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2607 emitTemplateLiteral
    pub(crate) fn emit_template_literal(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::NoSubstitutionTemplateLiteral => {
                self.emit_no_substitution_template_literal(node)
            }
            SyntaxKind::TemplateExpression => self.emit_template_expression(node),
            _ => panic!("unhandled TemplateLiteral: {}", kind_string(node.kind())),
        }
    }

    // Go: printer/printer.go:2618 emitTaggedTemplateExpression
    pub(crate) fn emit_tagged_template_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_callee(node.tag(), node);
        self.emit_type_arguments(node, node.type_argument_list());
        self.write_space();
        self.emit_template_literal(node.template());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2627 emitTypeAssertionExpression
    pub(crate) fn emit_type_assertion_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.write_punctuation("<");
        self.emit_type_node_outside_extends(node.type_());
        self.write_punctuation(">");
        self.emit_expression(node.expression(), OperatorPrecedence::UPDATE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2636 emitParenthesizedExpression
    pub(crate) fn emit_parenthesized_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        let open_paren_pos = self.emit_token(
            SyntaxKind::OpenParenToken,
            node.pos(),
            WriteKind::PUNCTUATION,
            node,
        );
        let indented = self.write_line_separators_and_indent_before(node.expression(), node);
        self.emit_expression(node.expression(), OperatorPrecedence::COMMA);
        self.write_line_separators_after(node.expression(), node);
        self.decrease_indent_if(indented);
        let mut close_paren_pos = open_paren_pos;
        if node.expression().is_some() {
            close_paren_pos = node.expression().end();
        }
        self.emit_token(
            SyntaxKind::CloseParenToken,
            close_paren_pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2651 emitFunctionExpression
    pub(crate) fn emit_function_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.generate_name_if_needed(node.name());
        self.emit_modifier_list(node, node.modifiers(), false /*allowDecorators*/);
        self.write_keyword("function");
        self.emit_token_node(node.asterisk_token());
        self.write_space();
        self.emit_identifier_name_node(node.name());
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(node);
        self.emit_signature(node);
        self.emit_function_body_node(node.body());
        self.pop_name_generation_scope(node);
        self.decrease_indent_if(indented);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2669 emitConciseBody
    pub(crate) fn emit_concise_body(&mut self, node: Node) {
        if is_block(node) {
            self.emit_function_body(node);
        } else if is_object_literal_expression(get_leftmost_expression(
            node, false, /*stopAtCallExpressions*/
        )) {
            // Wrap in ParenthesizedExpression to ensure parens are emitted after any leading
            // PartiallyEmittedExpression comments, matching TypeScript's factory-time wrapping
            // via parenthesizeConciseBodyOfArrowFunction.
            let paren = self.emit_context.factory.new_parenthesized_expression(node);
            set_node_loc(paren, node.loc());
            self.emit_expression(paren, OperatorPrecedence::LOWEST);
        } else if is_expression(node) {
            self.emit_expression(node, OperatorPrecedence::YIELD);
        } else {
            panic!("unexpected ConciseBody: {}", kind_string(node.kind()));
        }
    }

    // Go: printer/printer.go:2687 emitArrowFunction
    pub(crate) fn emit_arrow_function(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_modifier_list(node, node.modifiers(), false /*allowDecorators*/);
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.push_name_generation_scope(node);
        self.emit_type_parameters(node, node.type_parameter_list());
        self.emit_parameters_for_arrow(node, node.parameter_list());
        self.emit_type_annotation(node.type_());
        self.write_space();
        self.emit_token_node(node.equals_greater_than_token());
        self.write_space();
        self.emit_concise_body(node.body());
        self.pop_name_generation_scope(node);
        self.decrease_indent_if(indented);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2705 emitDeleteExpression
    pub(crate) fn emit_delete_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::DeleteKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::UNARY);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2713 emitTypeOfExpression
    pub(crate) fn emit_type_of_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::TypeOfKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::UNARY);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2721 emitVoidExpression
    pub(crate) fn emit_void_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::VoidKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::UNARY);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2729 emitAwaitExpression
    pub(crate) fn emit_await_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::AwaitKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::UNARY);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2737 emitPrefixUnaryExpression
    pub(crate) fn emit_prefix_unary_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        let operator = node.operator();
        let operand = node.operand();
        self.emit_token(operator, node.pos(), WriteKind::OPERATOR, node);

        // In some cases, we need to emit a space between the operator and the operand. One obvious case
        // is when the operator is an identifier, like delete or typeof. We also need to do this for plus
        // and minus expressions in certain cases. Specifically, consider the following two cases (parens
        // are just for clarity of exposition, and not part of the source code):
        //
        //  (+(+1))
        //  (+(++1))
        //
        // We need to emit a space in both cases. In the first case, the absence of a space will make
        // the resulting expression a prefix increment operation. And in the second, it will make the resulting
        // expression a prefix increment whose operand is a plus expression - (++(+x))
        // The same is true of minus of course.
        if operand.kind() == SyntaxKind::PrefixUnaryExpression {
            let inner = operand.operator();
            if (operator == SyntaxKind::PlusToken
                && (inner == SyntaxKind::PlusToken || inner == SyntaxKind::PlusPlusToken))
                || (operator == SyntaxKind::MinusToken
                    && (inner == SyntaxKind::MinusToken || inner == SyntaxKind::MinusMinusToken))
            {
                self.write_space();
            }
        }

        self.emit_expression(node.operand(), OperatorPrecedence::UNARY);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2767 emitPostfixUnaryExpression
    pub(crate) fn emit_postfix_unary_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.operand(), OperatorPrecedence::LEFT_HAND_SIDE);
        self.emit_token(
            node.operator(),
            node.operand().end(),
            WriteKind::OPERATOR,
            node,
        );
        self.exit_node(node, state);
    }

    // This function determines whether an expression consists of a homogeneous set of
    // literal expressions or binary plus expressions that all share the same literal kind.
    // It is used to determine whether the right-hand operand of a binary plus expression can be
    // emitted without parentheses.
    // Go: printer/printer.go:2778 getLiteralKindOfBinaryPlusOperand
    pub(crate) fn get_literal_kind_of_binary_plus_operand(&mut self, node: Node) -> SyntaxKind {
        let node = skip_partially_emitted_expressions(node);

        if is_literal_kind(node.kind()) {
            return node.kind();
        }

        if node.kind() == SyntaxKind::BinaryExpression
            && node.operator_token().kind() == SyntaxKind::PlusToken
        {
            // !!! Determine if caching this is worthwhile over recomputing
            let left_kind = self.get_literal_kind_of_binary_plus_operand(node.left());
            let mut literal_kind = SyntaxKind::Unknown;
            if is_literal_kind(left_kind)
                && left_kind == self.get_literal_kind_of_binary_plus_operand(node.right())
            {
                literal_kind = left_kind;
            }
            return literal_kind;
        }

        SyntaxKind::Unknown
    }

    // Go: printer/printer.go:2806 getBinaryExpressionPrecedence
    pub(crate) fn get_binary_expression_precedence(
        &mut self,
        node: Node,
    ) -> (OperatorPrecedence, OperatorPrecedence) {
        let precedence = get_expression_precedence(node);
        let mut left_prec = precedence;
        let mut right_prec = precedence;
        match precedence {
            OperatorPrecedence::COMMA => {
                // No need to parenthesize the right operand when the binary operator and
                // operand are both ,:
                //  x,(a,b)     => x,a,b
            }
            OperatorPrecedence::ASSIGNMENT => {
                // assignment is right-associative
                left_prec = OperatorPrecedence::CONDITIONAL;
                right_prec = OperatorPrecedence::YIELD;
            }
            OperatorPrecedence::LOGICAL_OR => right_prec = OperatorPrecedence::LOGICAL_AND,
            OperatorPrecedence::LOGICAL_AND => right_prec = OperatorPrecedence::BITWISE_OR,
            OperatorPrecedence::BITWISE_OR => {
                // No need to parenthesize the right operand when the binary operator and
                // operand are both | due to the associative property of mathematics:
                //  x|(a|b)     => x|a|b
            }
            OperatorPrecedence::BITWISE_XOR => {
                // No need to parenthesize the right operand when the binary operator and
                // operand are both ^ due to the associative property of mathematics:
                //  x^(a^b)     => x^a^b
            }
            OperatorPrecedence::BITWISE_AND => {
                // No need to parenthesize the right operand when the binary operator and
                // operand are both & due to the associative property of mathematics:
                //  x&(a&b)     => x&a&b
            }
            OperatorPrecedence::EQUALITY => right_prec = OperatorPrecedence::RELATIONAL,
            OperatorPrecedence::RELATIONAL => right_prec = OperatorPrecedence::SHIFT,
            OperatorPrecedence::SHIFT => right_prec = OperatorPrecedence::ADDITIVE,
            OperatorPrecedence::ADDITIVE => {
                // PORT: Go `break` out of the switch skips the final assignment.
                let mut skip = false;
                if node.operator_token().kind() == SyntaxKind::PlusToken
                    && is_binary_operation(node.right(), SyntaxKind::PlusToken)
                {
                    let left_kind = self.get_literal_kind_of_binary_plus_operand(node.left());
                    if is_literal_kind(left_kind)
                        && left_kind == self.get_literal_kind_of_binary_plus_operand(node.right())
                    {
                        // No need to parenthesize the right operand when the binary operator
                        // is plus (+) if both the left and right operands consist solely of either
                        // literals of the same kind or binary plus (+) expressions for literals of
                        // the same kind (recursively).
                        //  "a"+(1+2)       => "a"+(1+2)
                        //  "a"+("b"+"c")   => "a"+"b"+"c"
                        skip = true;
                    }
                }
                if !skip {
                    right_prec = OperatorPrecedence::MULTIPLICATIVE;
                }
            }
            OperatorPrecedence::MULTIPLICATIVE => {
                if node.operator_token().kind() == SyntaxKind::AsteriskToken
                    && is_binary_operation(node.right(), SyntaxKind::AsteriskToken)
                {
                    // No need to parenthesize the right operand when the binary operator and
                    // operand are both * due to the associative property of mathematics:
                    //  x*(a*b)     => x*a*b
                } else {
                    right_prec = OperatorPrecedence::EXPONENTIATION;
                }
            }
            OperatorPrecedence::EXPONENTIATION => {
                // exponentiation is right-associative
                left_prec = OperatorPrecedence::UPDATE;
            }
            // Go `%v` of an `ast.OperatorPrecedence` (an int with no
            // `String` method) is the number.
            _ => panic!("unhandled precedence: {}", precedence.0),
        }
        (left_prec, right_prec)
    }

    // Go: printer/printer.go:2872 emitBinaryExpression
    pub(crate) fn emit_binary_expression(&mut self, node: Node) {
        let (mut left_prec, mut right_prec) = self.get_binary_expression_precedence(node);
        let emitted_left = skip_partially_emitted_expressions(node.left());
        if node_is_synthesized(emitted_left)
            && emitted_left.kind() == SyntaxKind::BinaryExpression
            && mixing_binary_operators_requires_parentheses(
                node.operator_token().kind(),
                emitted_left.operator_token().kind(),
            )
        {
            left_prec = OperatorPrecedence::HIGHEST;
        }
        let emitted_right = skip_partially_emitted_expressions(node.right());
        if node_is_synthesized(emitted_right)
            && emitted_right.kind() == SyntaxKind::BinaryExpression
            && mixing_binary_operators_requires_parentheses(
                node.operator_token().kind(),
                emitted_right.operator_token().kind(),
            )
        {
            right_prec = OperatorPrecedence::HIGHEST;
        }
        let state = self.enter_node(node);
        self.emit_expression(node.left(), left_prec);
        let lines_before_operator =
            self.get_lines_between_nodes(node, node.left(), node.operator_token());
        let lines_after_operator =
            self.get_lines_between_nodes(node, node.operator_token(), node.right());
        self.write_lines_and_indent(
            lines_before_operator,
            node.operator_token().kind() != SyntaxKind::CommaToken, /*writeSpaceIfNotIndenting*/
        );
        self.emit_token_node_ex(node.operator_token(), TokenEmitFlags::NO_SOURCE_MAPS);
        self.write_lines_and_indent(lines_after_operator, true /*writeSpaceIfNotIndenting*/); // Binary operators should have a space before the comment starts
        self.emit_expression(node.right(), right_prec);
        self.decrease_indent_if(lines_after_operator > 0);
        self.decrease_indent_if(lines_before_operator > 0);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2893 emitShortCircuitExpression
    pub(crate) fn emit_short_circuit_expression(&mut self, node: Node) {
        if is_binary_operation(
            skip_partially_emitted_expressions(node),
            SyntaxKind::QuestionQuestionToken,
        ) {
            self.emit_expression(node, OperatorPrecedence::COALESCE);
        } else {
            self.emit_expression(node, OperatorPrecedence::LOGICAL_OR);
        }
    }

    // Go: printer/printer.go:2901 emitConditionalExpression
    pub(crate) fn emit_conditional_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        let lines_before_question =
            self.get_lines_between_nodes(node, node.condition(), node.question_token());
        let lines_after_question =
            self.get_lines_between_nodes(node, node.question_token(), node.when_true());
        let lines_before_colon =
            self.get_lines_between_nodes(node, node.when_true(), node.colon_token());
        let lines_after_colon =
            self.get_lines_between_nodes(node, node.colon_token(), node.when_false());
        self.emit_short_circuit_expression(node.condition());
        self.write_lines_and_indent(
            lines_before_question,
            true, /*writeSpaceIfNotIndenting*/
        );
        self.emit_punctuation_node(node.question_token());
        self.write_lines_and_indent(lines_after_question, true /*writeSpaceIfNotIndenting*/);
        self.emit_expression(node.when_true(), OperatorPrecedence::YIELD);
        self.decrease_indent_if(lines_after_question > 0);
        self.decrease_indent_if(lines_before_question > 0);
        self.write_lines_and_indent(lines_before_colon, true /*writeSpaceIfNotIndenting*/);
        self.emit_punctuation_node(node.colon_token());
        self.write_lines_and_indent(lines_after_colon, true /*writeSpaceIfNotIndenting*/);
        self.emit_expression(node.when_false(), OperatorPrecedence::YIELD);
        self.decrease_indent_if(lines_after_colon > 0);
        self.decrease_indent_if(lines_before_colon > 0);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2923 emitTemplateExpression
    pub(crate) fn emit_template_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_template_head(node.head());
        self.emit_list(
            Printer::emit_template_span_node,
            node,
            node.template_spans(),
            ListFormat::TEMPLATE_EXPRESSION_SPANS,
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2930 emitYieldExpression
    pub(crate) fn emit_yield_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::YieldKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.emit_punctuation_node(node.asterisk_token());
        if node.expression().is_some() {
            self.write_space();
            self.emit_expression_no_asi(node.expression(), OperatorPrecedence::DISALLOW_COMMA);
        }
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2941 emitSpreadElement
    pub(crate) fn emit_spread_element(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::DotDotDotToken,
            node.pos(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(node.expression(), OperatorPrecedence::DISALLOW_COMMA);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2948 emitClassExpression
    pub(crate) fn emit_class_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.generate_name_if_needed(node.name());

        let pos = self.emit_modifier_list(node, node.modifiers(), true /*allowDecorators*/);
        self.emit_token(SyntaxKind::ClassKeyword, pos, WriteKind::KEYWORD, node);

        if node.name().is_some() {
            self.write_space();
            self.emit_identifier_name(node.name());
        }

        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);

        self.emit_type_parameters(node, node.type_parameter_list());
        self.emit_list(
            Printer::emit_heritage_clause_node,
            node,
            node.heritage_clauses(),
            ListFormat::CLASS_HERITAGE_CLAUSES,
        );
        self.write_space();
        self.write_punctuation("{");
        self.push_name_generation_scope(node);
        self.generate_all_member_names(node.member_list());
        self.emit_list(
            Printer::emit_class_element,
            node,
            node.member_list(),
            ListFormat::CLASS_MEMBERS,
        );
        self.pop_name_generation_scope(node);
        self.write_punctuation("}");

        self.decrease_indent_if(indented);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2977 emitOmittedExpression
    pub(crate) fn emit_omitted_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2981 emitExpressionWithTypeArguments
    pub(crate) fn emit_expression_with_type_arguments(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.expression(), OperatorPrecedence::MEMBER);
        self.emit_type_arguments(node, node.type_argument_list());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2988 emitAsExpression
    pub(crate) fn emit_as_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.expression(), OperatorPrecedence::RELATIONAL);
        self.write_space();
        self.write_keyword("as");
        self.write_space();
        self.emit_type_node_outside_extends(node.type_());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:2998 emitSatisfiesExpression
    pub(crate) fn emit_satisfies_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.expression(), OperatorPrecedence::RELATIONAL);
        self.write_space();
        self.write_keyword("satisfies");
        self.write_space();
        self.emit_type_node_outside_extends(node.type_());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3008 emitNonNullExpression
    pub(crate) fn emit_non_null_expression(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.expression(), OperatorPrecedence::MEMBER);
        self.write_operator("!");
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3015 emitMetaProperty
    pub(crate) fn emit_meta_property(&mut self, node: Node) {
        let state = self.enter_node(node);
        let pos = self.emit_token(
            node.keyword_token(),
            node.pos(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_token(SyntaxKind::DotToken, pos, WriteKind::PUNCTUATION, node);
        self.emit_identifier_name(node.name());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3023 emitPartiallyEmittedExpression
    pub(crate) fn emit_partially_emitted_expression(&mut self, mut node: Node) {
        // avoid reprinting parens for nested partially emitted expressions
        // PORT: Go `core.Stack[entry]` is a `Vec` of (node, state).
        let mut stack: Vec<(Node, PrinterState)> = Vec::new();
        loop {
            let state = self.enter_node(node);
            let emit_flags = self.emit_context.emit_flags(node);
            if !emit_flags.intersects(EmitFlags::NO_LEADING_COMMENTS)
                && node.pos() != node.expression().pos()
            {
                self.emit_trailing_comments_of_position(
                    node.expression().pos(),
                    false, /*prefixSpace*/
                    false, /*forceNoNewline*/
                );
            }
            stack.push((node, state));
            if !is_partially_emitted_expression(node.expression()) {
                break;
            }
            node = node.expression();
        }

        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);

        // unwind stack
        while let Some((entry_node, entry_state)) = stack.pop() {
            let emit_flags = self.emit_context.emit_flags(node);
            if !emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS)
                && node.end() != node.expression().end()
            {
                self.emit_leading_comments_of_position(node.expression().end());
            }
            self.exit_node(node, entry_state);
            node = entry_node;
        }
    }

    // Go: printer/printer.go:3057 commentWillEmitNewLine
    pub(crate) fn comment_will_emit_new_line(&self, comment: &CommentRange) -> bool {
        comment.kind == SyntaxKind::SingleLineCommentTrivia || comment.has_trailing_new_line
    }

    // Go: printer/printer.go:3061 syntheticCommentWillEmitNewLine
    pub(crate) fn synthetic_comment_will_emit_new_line(
        &self,
        comment: &SynthesizedComment,
    ) -> bool {
        comment.kind == SyntaxKind::SingleLineCommentTrivia || comment.has_trailing_new_line
    }

    // Go: printer/printer.go:3065 willEmitLeadingNewLine
    pub(crate) fn will_emit_leading_new_line(&mut self, node: Node) -> bool {
        if self.current_source_file.is_nil() {
            return false;
        }
        let text = self.current_source_file_text();
        let mut has_leading_comment_ranges = false;
        let mut has_new_line_comment = false;
        for comment in crate::frontend::scanner::get_leading_comment_ranges(
            self.emit_context.factory().as_node_factory(),
            &text,
            node.pos(),
        ) {
            has_leading_comment_ranges = true;
            if self.comment_will_emit_new_line(&comment) {
                has_new_line_comment = true;
            }
        }
        if has_leading_comment_ranges {
            let parse_node = self.emit_context.parse_node(node);
            if parse_node.is_some() && is_parenthesized_expression(parse_node.parent()) {
                return true;
            }
        }
        if has_new_line_comment {
            return true;
        }
        if self
            .emit_context
            .get_synthetic_leading_comments(node)
            .iter()
            .any(|comment| self.synthetic_comment_will_emit_new_line(comment))
        {
            return true;
        }
        if is_partially_emitted_expression(node) {
            let expression = node.expression();
            if node.pos() != expression.pos() {
                for comment in crate::frontend::scanner::get_trailing_comment_ranges(
                    self.emit_context.factory().as_node_factory(),
                    &text,
                    expression.pos(),
                ) {
                    if self.comment_will_emit_new_line(&comment) {
                        return true;
                    }
                }
            }
            return self.will_emit_leading_new_line(expression);
        }
        false
    }

    // parenthesizeExpressionForNoAsi wraps an expression in parens if we would emit a leading comment
    // that would introduce a line separator between the node and its parent.
    // Go: printer/printer.go:3105 parenthesizeExpressionForNoAsi
    pub(crate) fn parenthesize_expression_for_no_asi(&mut self, node: Node) -> Node {
        if !self.comments_disabled {
            match node.kind() {
                SyntaxKind::PartiallyEmittedExpression => {
                    if self.will_emit_leading_new_line(node) {
                        let parse_node = self.emit_context.parse_node(node);
                        if parse_node.is_some() && is_parenthesized_expression(parse_node) {
                            // If the original node was a parenthesized expression, restore it to preserve comment and source map emit
                            let parens = self
                                .emit_context
                                .factory
                                .new_parenthesized_expression(node.expression());
                            self.emit_context.set_original(parens, node);
                            set_node_loc(parens, parse_node.loc());
                            return parens;
                        }
                        return self.emit_context.factory.new_parenthesized_expression(node);
                    }
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self
                        .emit_context
                        .factory
                        .update_partially_emitted_expression(node, expression);
                }
                SyntaxKind::PropertyAccessExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_property_access_expression(
                        node,
                        expression,
                        node.question_dot_token(),
                        node.name(),
                        node.flags(),
                    );
                }
                SyntaxKind::ElementAccessExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_element_access_expression(
                        node,
                        expression,
                        node.question_dot_token(),
                        node.argument_expression(),
                        node.flags(),
                    );
                }
                SyntaxKind::CallExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_call_expression(
                        node,
                        expression,
                        node.question_dot_token(),
                        node.type_argument_list(),
                        node.argument_list(),
                        node.flags(),
                    );
                }
                SyntaxKind::TaggedTemplateExpression => {
                    let tag = self.parenthesize_expression_for_no_asi(node.tag());
                    return self.emit_context.factory.update_tagged_template_expression(
                        node,
                        tag,
                        node.question_dot_token(),
                        node.type_argument_list(),
                        node.template(),
                        node.flags(),
                    );
                }
                SyntaxKind::PostfixUnaryExpression => {
                    let operand = self.parenthesize_expression_for_no_asi(node.operand());
                    return self.emit_context.factory.update_postfix_unary_expression(
                        node,
                        operand,
                        node.operator(),
                    );
                }
                SyntaxKind::BinaryExpression => {
                    let left = self.parenthesize_expression_for_no_asi(node.left());
                    return self.emit_context.factory.update_binary_expression(
                        node,
                        node.modifiers(),
                        left,
                        node.type_(),
                        node.operator_token(),
                        node.right(),
                    );
                }
                SyntaxKind::ConditionalExpression => {
                    let condition = self.parenthesize_expression_for_no_asi(node.condition());
                    return self.emit_context.factory.update_conditional_expression(
                        node,
                        condition,
                        node.question_token(),
                        node.when_true(),
                        node.colon_token(),
                        node.when_false(),
                    );
                }
                SyntaxKind::AsExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_as_expression(
                        node,
                        expression,
                        node.type_(),
                    );
                }
                SyntaxKind::SatisfiesExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_satisfies_expression(
                        node,
                        expression,
                        node.type_(),
                    );
                }
                SyntaxKind::NonNullExpression => {
                    let expression = self.parenthesize_expression_for_no_asi(node.expression());
                    return self.emit_context.factory.update_non_null_expression(
                        node,
                        expression,
                        node.flags(),
                    );
                }
                _ => {}
            }
        }
        node
    }

    // Go: printer/printer.go:3217 emitExpressionNoASI
    pub(crate) fn emit_expression_no_asi(&mut self, node: Node, precedence: OperatorPrecedence) {
        let node = self.parenthesize_expression_for_no_asi(node);
        self.emit_expression(node, precedence);
    }

    // Go: printer/printer.go:3222 emitExpression
    pub(crate) fn emit_expression(&mut self, node: Node, precedence: OperatorPrecedence) {
        let parens =
            get_expression_precedence(skip_partially_emitted_expressions(node)) < precedence;
        if parens {
            self.write_punctuation("(");
        }

        match node.kind() {
            // Keywords
            SyntaxKind::TrueKeyword | SyntaxKind::FalseKeyword | SyntaxKind::NullKeyword => {
                self.emit_token_node(node);
            }
            SyntaxKind::ThisKeyword | SyntaxKind::SuperKeyword | SyntaxKind::ImportKeyword => {
                self.emit_keyword_expression(node);
            }

            // Literals
            SyntaxKind::NumericLiteral => self.emit_numeric_literal(node),
            SyntaxKind::BigIntLiteral => self.emit_big_int_literal(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            SyntaxKind::RegularExpressionLiteral => self.emit_regular_expression_literal(node),
            SyntaxKind::NoSubstitutionTemplateLiteral => {
                self.emit_no_substitution_template_literal(node)
            }

            // Identifiers
            SyntaxKind::Identifier => self.emit_identifier_reference(node),
            SyntaxKind::PrivateIdentifier => self.emit_private_identifier(node),

            // Expressions
            SyntaxKind::ArrayLiteralExpression => self.emit_array_literal_expression(node),
            SyntaxKind::ObjectLiteralExpression => self.emit_object_literal_expression(node),
            SyntaxKind::PropertyAccessExpression => self.emit_property_access_expression(node),
            SyntaxKind::ElementAccessExpression => self.emit_element_access_expression(node),
            SyntaxKind::CallExpression => self.emit_call_expression(node),
            SyntaxKind::NewExpression => self.emit_new_expression(node),
            SyntaxKind::TaggedTemplateExpression => self.emit_tagged_template_expression(node),
            SyntaxKind::TypeAssertionExpression => self.emit_type_assertion_expression(node),
            SyntaxKind::ParenthesizedExpression => self.emit_parenthesized_expression(node),
            SyntaxKind::FunctionExpression => self.emit_function_expression(node),
            SyntaxKind::ArrowFunction => self.emit_arrow_function(node),
            SyntaxKind::DeleteExpression => self.emit_delete_expression(node),
            SyntaxKind::TypeOfExpression => self.emit_type_of_expression(node),
            SyntaxKind::VoidExpression => self.emit_void_expression(node),
            SyntaxKind::AwaitExpression => self.emit_await_expression(node),
            SyntaxKind::PrefixUnaryExpression => self.emit_prefix_unary_expression(node),
            SyntaxKind::PostfixUnaryExpression => self.emit_postfix_unary_expression(node),
            SyntaxKind::BinaryExpression => self.emit_binary_expression(node),
            SyntaxKind::ConditionalExpression => self.emit_conditional_expression(node),
            SyntaxKind::TemplateExpression => self.emit_template_expression(node),
            SyntaxKind::YieldExpression => self.emit_yield_expression(node),
            SyntaxKind::SpreadElement => self.emit_spread_element(node),
            SyntaxKind::ClassExpression => self.emit_class_expression(node),
            SyntaxKind::OmittedExpression => self.emit_omitted_expression(node),
            SyntaxKind::AsExpression => self.emit_as_expression(node),
            SyntaxKind::NonNullExpression => self.emit_non_null_expression(node),
            SyntaxKind::ExpressionWithTypeArguments => {
                self.emit_expression_with_type_arguments(node)
            }
            SyntaxKind::SatisfiesExpression => self.emit_satisfies_expression(node),
            SyntaxKind::MetaProperty => self.emit_meta_property(node),
            SyntaxKind::SyntheticExpression => {
                panic!("SyntheticExpression should never be printed.")
            }
            SyntaxKind::MissingDeclaration => {
                // Missing declarations do not emit an expression.
            }

            // JSX
            SyntaxKind::JsxElement => self.emit_jsx_element(node),
            SyntaxKind::JsxSelfClosingElement => self.emit_jsx_self_closing_element(node),
            SyntaxKind::JsxFragment => self.emit_jsx_fragment(node),

            // Synthesized list
            SyntaxKind::SyntaxList => panic!("SyntaxList should not be printed"),

            // Transformation nodes
            SyntaxKind::NotEmittedStatement => return,
            SyntaxKind::PartiallyEmittedExpression => self.emit_partially_emitted_expression(node),
            SyntaxKind::SyntheticReferenceExpression => {
                panic!("SyntheticReferenceExpression should not be printed")
            }

            _ => panic!("unexpected Expression: {}", kind_string(node.kind())),
        }

        if parens {
            self.write_punctuation(")");
        }
    }
}

//
// Misc
//

impl Printer {
    // Go: printer/printer.go:3350 emitTemplateSpan
    pub(crate) fn emit_template_span(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_expression(node.expression(), OperatorPrecedence::COMMA);
        self.emit_template_middle_tail(node.literal());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3357 emitTemplateSpanNode
    pub(crate) fn emit_template_span_node(&mut self, node: Node) {
        self.emit_template_span(node);
    }

    // Go: printer/printer.go:3361 emitSemicolonClassElement
    pub(crate) fn emit_semicolon_class_element(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }
}

//
// Statements
//

impl Printer {
    // Go: printer/printer.go:3371 isEmptyBlock
    pub(crate) fn is_empty_block(&self, block: Node, statements: NodeList) -> bool {
        statements.nodes().is_empty()
            && (self.current_source_file.is_nil()
                || range_end_is_on_same_line_as_range_start(
                    block.loc(),
                    block.loc(),
                    self.current_source_file,
                ))
    }

    // Go: printer/printer.go:3376 emitBlock
    pub(crate) fn emit_block(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.generate_names(node);
        self.emit_token(
            SyntaxKind::OpenBraceToken,
            node.pos(),
            WriteKind::PUNCTUATION,
            node,
        );

        let format = if !node.multi_line() && self.is_empty_block(node, node.statement_list())
            || self.should_emit_on_single_line(node)
        {
            ListFormat::SINGLE_LINE_BLOCK_STATEMENTS
        } else {
            ListFormat::MULTI_LINE_BLOCK_STATEMENTS
        };
        self.emit_list(Printer::emit_statement, node, node.statement_list(), format);

        self.emit_token_ex(
            SyntaxKind::CloseBraceToken,
            node.statement_list().end(),
            WriteKind::PUNCTUATION,
            node,
            if format.intersects(ListFormat::MULTI_LINE) {
                TokenEmitFlags::INDENT_LEADING_COMMENTS
            } else {
                TokenEmitFlags::NONE
            },
        );
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3390 emitVariableStatement
    pub(crate) fn emit_variable_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_modifier_list(node, node.modifiers(), false /*allowDecorators*/);
        self.emit_variable_declaration_list(node.declaration_list());
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3398 emitEmptyStatement
    pub(crate) fn emit_empty_statement(&mut self, node: Node, is_embedded_statement: bool) {
        let state = self.enter_node(node);

        // While most trailing semicolons are possibly insignificant, an embedded "empty"
        // statement is significant and cannot be elided by a trailing-semicolon-omitting writer.
        if is_embedded_statement {
            self.write_punctuation(";");
        } else {
            self.write_trailing_semicolon();
        }
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3411 emitExpressionStatement
    pub(crate) fn emit_expression_statement(&mut self, node: Node) {
        let state = self.enter_node(node);

        if self.current_source_file.is_some()
            && source_file_script_kind(self.current_source_file) == ScriptKind::JSON
        {
            // !!! In strada, this was handled by an undefined parenthesizerRule, so this is a hack.
            self.emit_expression(node.expression(), OperatorPrecedence::COMMA);
        } else if is_immediately_invoked_function_expression_or_arrow_function(node.expression()) {
            // For IIFEs, parenthesize just the callee (not the whole call), matching TypeScript's
            // parenthesizeExpressionOfExpressionStatement which wraps the function/arrow in parens:
            //   (function() { })()  -- not (function() { }())
            self.emit_iife_with_parenthesized_callee(node.expression());
        } else {
            match get_leftmost_expression(node.expression(), false /*stopAtCallExpression*/).kind()
            {
                SyntaxKind::FunctionExpression | SyntaxKind::ObjectLiteralExpression => {
                    self.emit_expression(node.expression(), OperatorPrecedence::PARENTHESES);
                }
                _ => self.emit_expression(node.expression(), OperatorPrecedence::COMMA),
            }
        }

        // Emit semicolon in non json files
        // or if json file that created synthesized expression(eg.define expression statement when --out and amd code generation)
        if self.current_source_file.is_nil()
            || source_file_script_kind(self.current_source_file) != ScriptKind::JSON
            || node_is_synthesized(node.expression())
        {
            self.write_trailing_semicolon();
        }

        self.exit_node(node, state);
    }

    // emitIIFEWithParenthesizedCallee emits a call expression that is an IIFE,
    // wrapping just the callee in parens rather than the entire call expression.
    // This matches TypeScript's parenthesizeExpressionOfExpressionStatement behavior:
    //
    //	(function() { })()   -- parens around callee only
    //
    // instead of:
    //
    //	(function() { }())   -- parens around entire call
    // Go: printer/printer.go:3451 emitIIFEWithParenthesizedCallee
    pub(crate) fn emit_iife_with_parenthesized_callee(&mut self, node: Node) {
        // Walk through PartiallyEmittedExpression wrappers to find the call
        let call = skip_partially_emitted_expressions(node);
        // PORT: Go `.AsCallExpression()` panics on other kinds.
        debug_assert!(call.kind() == SyntaxKind::CallExpression);
        let state = self.enter_node(call);
        // Emit the callee wrapped in parens
        self.write_punctuation("(");
        self.emit_expression(call.expression(), OperatorPrecedence::LOWEST);
        self.write_punctuation(")");
        self.emit_token_node(call.question_dot_token());
        self.emit_type_arguments(call, call.type_argument_list());
        self.emit_list(
            Printer::emit_argument,
            call,
            call.argument_list(),
            ListFormat::CALL_EXPRESSION_ARGUMENTS,
        );
        self.exit_node(call, state);
    }

    // Go: printer/printer.go:3465 emitIfStatement
    pub(crate) fn emit_if_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let pos = self.emit_token(SyntaxKind::IfKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            node.expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_embedded_statement(node, node.then_statement());
        if node.else_statement().is_some() {
            self.write_line_or_space(node, node.then_statement(), node.else_statement());
            self.emit_token(
                SyntaxKind::ElseKeyword,
                node.then_statement().end(),
                WriteKind::KEYWORD,
                node,
            );
            if node.else_statement().kind() == SyntaxKind::IfStatement {
                self.write_space();
                self.emit_if_statement(node.else_statement());
            } else {
                self.emit_embedded_statement(node, node.else_statement());
            }
        }
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3486 emitWhileClause
    pub(crate) fn emit_while_clause(&mut self, node: Node, expression: Node, start_pos: i32) {
        let pos = self.emit_token(
            SyntaxKind::WhileKeyword,
            start_pos,
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(expression, OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            expression.end(),
            WriteKind::PUNCTUATION,
            node,
        );
    }

    // Go: printer/printer.go:3494 emitDoStatement
    pub(crate) fn emit_do_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(SyntaxKind::DoKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.emit_embedded_statement(node, node.statement());
        if is_block(node.statement()) && !self.options.preserve_source_newlines {
            self.write_space();
        } else {
            self.write_line_or_space(node, node.statement(), node.expression());
        }

        self.emit_while_clause(node, node.expression(), node.statement().end());
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3509 emitWhileStatement
    pub(crate) fn emit_while_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_while_clause(node, node.expression(), node.pos());
        self.emit_embedded_statement(node, node.statement());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3516 emitForInitializer
    pub(crate) fn emit_for_initializer(&mut self, node: Node) {
        if node.kind() == SyntaxKind::VariableDeclarationList {
            self.emit_variable_declaration_list(node);
        } else {
            self.emit_expression(node, OperatorPrecedence::LOWEST);
        }
    }

    // Go: printer/printer.go:3524 emitForStatement
    pub(crate) fn emit_for_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let mut pos = self.emit_token(SyntaxKind::ForKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        pos = self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        if node.initializer().is_some() {
            self.emit_for_initializer(node.initializer());
            pos = node.initializer().end();
        }
        pos = self.emit_token(
            SyntaxKind::SemicolonToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        if node.condition().is_some() {
            self.write_space();
            self.emit_expression(node.condition(), OperatorPrecedence::LOWEST);
            pos = node.condition().end();
        }
        pos = self.emit_token(
            SyntaxKind::SemicolonToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        if node.incrementor().is_some() {
            self.write_space();
            self.emit_expression(node.incrementor(), OperatorPrecedence::LOWEST);
            pos = node.incrementor().end();
        }
        self.emit_token(
            SyntaxKind::CloseParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_embedded_statement(node, node.statement());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3550 emitForInStatement
    pub(crate) fn emit_for_in_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let pos = self.emit_token(SyntaxKind::ForKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_for_initializer(node.initializer());
        self.write_space();
        self.emit_token(
            SyntaxKind::InKeyword,
            node.initializer().end(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            node.expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_embedded_statement(node, node.statement());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3565 emitForOfStatement
    pub(crate) fn emit_for_of_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let open_paren_pos =
            self.emit_token(SyntaxKind::ForKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        if node.await_modifier().is_some() {
            self.emit_keyword_node(node.await_modifier());
            self.write_space();
        }
        self.emit_token(
            SyntaxKind::OpenParenToken,
            open_paren_pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_for_initializer(node.initializer());
        self.write_space();
        self.emit_token(
            SyntaxKind::OfKeyword,
            node.initializer().end(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            node.expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_embedded_statement(node, node.statement());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3584 emitContinueStatement
    pub(crate) fn emit_continue_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::ContinueKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        if node.label().is_some() {
            self.write_space();
            self.emit_label_identifier(node.label());
        }
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3595 emitBreakStatement
    pub(crate) fn emit_break_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::BreakKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        if node.label().is_some() {
            self.write_space();
            self.emit_label_identifier(node.label());
        }
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3606 emitReturnStatement
    pub(crate) fn emit_return_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::ReturnKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        if node.expression().is_some() {
            self.write_space();
            self.emit_expression_no_asi(node.expression(), OperatorPrecedence::LOWEST);
        }
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3617 emitWithStatement
    pub(crate) fn emit_with_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let pos = self.emit_token(
            SyntaxKind::WithKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            node.expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_embedded_statement(node, node.statement());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3628 emitSwitchStatement
    pub(crate) fn emit_switch_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        let pos = self.emit_token(
            SyntaxKind::SwitchKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_token(
            SyntaxKind::OpenParenToken,
            pos,
            WriteKind::PUNCTUATION,
            node,
        );
        self.emit_expression(node.expression(), OperatorPrecedence::LOWEST);
        self.emit_token(
            SyntaxKind::CloseParenToken,
            node.expression().end(),
            WriteKind::PUNCTUATION,
            node,
        );
        self.write_space();
        self.emit_case_block(node.case_block());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3640 emitLabeledStatement
    pub(crate) fn emit_labeled_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_label_identifier(node.label());
        self.emit_token(
            SyntaxKind::ColonToken,
            node.label().end(),
            WriteKind::PUNCTUATION,
            node,
        );

        // TODO: use emitEmbeddedStatement rather than writeSpace/emitStatement here after Strada migration as it is
        //       more consistent with similar emit elsewhere. writeSpace/emitStatement is used here to reduce spurious
        //       diffs when testing the Strada migration.

        self.write_space();
        self.emit_statement(node.statement());

        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3656 emitThrowStatement
    pub(crate) fn emit_throw_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::ThrowKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_space();
        self.emit_expression_no_asi(node.expression(), OperatorPrecedence::LOWEST);
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3665 emitTryStatement
    pub(crate) fn emit_try_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(SyntaxKind::TryKeyword, node.pos(), WriteKind::KEYWORD, node);
        self.write_space();
        self.emit_block(node.try_block());
        if node.catch_clause().is_some() {
            self.write_line_or_space(node, node.try_block(), node.catch_clause());
            self.emit_catch_clause(node.catch_clause());
        }
        if node.finally_block().is_some() {
            // Go: core.Coalesce(node.CatchClause, node.TryBlock)
            let prev = if node.catch_clause().is_some() {
                node.catch_clause()
            } else {
                node.try_block()
            };
            self.write_line_or_space(node, prev, node.finally_block());
            self.emit_token(
                SyntaxKind::FinallyKeyword,
                prev.end(),
                WriteKind::KEYWORD,
                node,
            );
            self.write_space();
            self.emit_block(node.finally_block());
        }
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3683 emitDebuggerStatement
    pub(crate) fn emit_debugger_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_token(
            SyntaxKind::DebuggerKeyword,
            node.pos(),
            WriteKind::KEYWORD,
            node,
        );
        self.write_trailing_semicolon();
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3690 emitNotEmittedStatement
    pub(crate) fn emit_not_emitted_statement(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:3694 emitNotEmittedTypeElement
    pub(crate) fn emit_not_emitted_type_element(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.exit_node(node, state);
    }
}
