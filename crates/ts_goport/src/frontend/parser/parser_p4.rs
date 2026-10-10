use crate::frontend::prelude::*;

// Go: internal/parser/parser.go lines 4184 to 5533 (yield, arrow functions,
// conditional and binary expressions, unary and update expressions, JSX,
// left-hand-side and member expressions, calls, tagged templates).
//
// PORT: Go evaluates call arguments left to right. Rust cannot pass a
// `&mut self` call as an argument to another `&mut self` call, so each such
// argument is moved into a local first, in the Go order.

impl<'a> Parser<'a> {
    // Go: parser.go:4230 parseYieldExpression
    pub fn parse_yield_expression(&mut self) -> Node {
        let pos = self.node_pos();
        // YieldExpression[In] :
        //      yield
        //      yield [no LineTerminator here] [Lexical goal InputElementRegExp]AssignmentExpression[?In, Yield]
        //      yield [no LineTerminator here] * [Lexical goal InputElementRegExp]AssignmentExpression[?In, Yield]
        self.next_token();
        let result;
        if !self.has_preceding_line_break()
            && (self.token == SyntaxKind::AsteriskToken || self.is_start_of_expression())
        {
            let asterisk_token = self.parse_optional_token(SyntaxKind::AsteriskToken);
            let expression = self.parse_assignment_expression_or_higher();
            result = self
                .factory
                .new_yield_expression(asterisk_token, expression);
        } else {
            // if the next token is not on the same line as yield.  or we don't have an '*' or
            // the start of an expression, then this is just a simple "yield" expression.
            result = self.factory.new_yield_expression(
                Node::NIL, /*asteriskToken*/
                Node::NIL, /*expression*/
            );
        }
        self.finish_node(result, pos)
    }

    // Go: parser.go:4248 isParenthesizedArrowFunctionExpression
    pub fn is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        if self.token == SyntaxKind::OpenParenToken
            || self.token == SyntaxKind::LessThanToken
            || self.token == SyntaxKind::AsyncKeyword
        {
            let state = self.mark();
            let result = self.next_is_parenthesized_arrow_function_expression();
            self.rewind(state);
            return result;
        }
        if self.token == SyntaxKind::EqualsGreaterThanToken {
            // ERROR RECOVERY TWEAK:
            // If we see a standalone => try to parse it as an arrow function expression as that's
            // likely what the user intended to write.
            return Tristate::True;
        }
        // Definitely not a parenthesized arrow function.
        Tristate::False
    }

    // Go: parser.go:4265 nextIsParenthesizedArrowFunctionExpression
    pub fn next_is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        if self.token == SyntaxKind::AsyncKeyword {
            self.next_token();
            if self.has_preceding_line_break() {
                return Tristate::False;
            }
            if self.token != SyntaxKind::OpenParenToken && self.token != SyntaxKind::LessThanToken {
                return Tristate::False;
            }
        }
        let first = self.token;
        let second = self.next_token();
        if first == SyntaxKind::OpenParenToken {
            if second == SyntaxKind::CloseParenToken {
                // Simple cases: "() =>", "(): ", and "() {".
                // This is an arrow function with no parameters.
                // The last one is not actually an arrow function,
                // but this is probably what the user intended.
                let third = self.next_token();
                return match third {
                    SyntaxKind::EqualsGreaterThanToken
                    | SyntaxKind::ColonToken
                    | SyntaxKind::OpenBraceToken => Tristate::True,
                    _ => Tristate::False,
                };
            }
            // If encounter "([" or "({", this could be the start of a binding pattern.
            // Examples:
            //      ([ x ]) => { }
            //      ({ x }) => { }
            //      ([ x ])
            //      ({ x })
            if second == SyntaxKind::OpenBracketToken || second == SyntaxKind::OpenBraceToken {
                return Tristate::Unknown;
            }
            // Simple case: "(..."
            // This is an arrow function with a rest parameter.
            if second == SyntaxKind::DotDotDotToken {
                return Tristate::True;
            }
            // Check for "(xxx yyy", where xxx is a modifier and yyy is an identifier. This
            // isn't actually allowed, but we want to treat it as a lambda so we can provide
            // a good error message.
            if is_modifier_kind(second)
                && second != SyntaxKind::AsyncKeyword
                && self.look_ahead(Parser::next_token_is_identifier)
            {
                if self.next_token() == SyntaxKind::AsKeyword {
                    // https://github.com/microsoft/TypeScript/issues/44466
                    return Tristate::False;
                }
                return Tristate::True;
            }
            // If we had "(" followed by something that's not an identifier,
            // then this definitely doesn't look like a lambda.  "this" is not
            // valid, but we want to parse it and then give a semantic error.
            if !self.is_identifier() && second != SyntaxKind::ThisKeyword {
                return Tristate::False;
            }
            match self.next_token() {
                SyntaxKind::ColonToken => {
                    // If we have something like "(a:", then we must have a
                    // type-annotated parameter in an arrow function expression.
                    return Tristate::True;
                }
                SyntaxKind::QuestionToken => {
                    self.next_token();
                    // If we have "(a?:" or "(a?," or "(a?=" or "(a?)" then it is definitely a lambda.
                    if self.token == SyntaxKind::ColonToken
                        || self.token == SyntaxKind::CommaToken
                        || self.token == SyntaxKind::EqualsToken
                        || self.token == SyntaxKind::CloseParenToken
                    {
                        return Tristate::True;
                    }
                    // Otherwise it is definitely not a lambda.
                    return Tristate::False;
                }
                SyntaxKind::CommaToken | SyntaxKind::EqualsToken | SyntaxKind::CloseParenToken => {
                    // If we have "(a," or "(a=" or "(a)" this *could* be an arrow function
                    return Tristate::Unknown;
                }
                _ => {}
            }
            // It is definitely not an arrow function
            Tristate::False
        } else {
            debug_assert!(first == SyntaxKind::LessThanToken);
            // If we have "<" not followed by an identifier,
            // then this definitely is not an arrow function.
            if !self.is_identifier() && self.token != SyntaxKind::ConstKeyword {
                return Tristate::False;
            }
            // JSX overrides
            if self.language_variant == LanguageVariant::JSX {
                let is_arrow_function_in_jsx = self.look_ahead(|p: &mut Parser<'a>| {
                    p.parse_optional(SyntaxKind::ConstKeyword);
                    let third = p.next_token();
                    if third == SyntaxKind::ExtendsKeyword {
                        let fourth = p.next_token();
                        return !matches!(
                            fourth,
                            SyntaxKind::EqualsToken
                                | SyntaxKind::GreaterThanToken
                                | SyntaxKind::SlashToken
                        );
                    } else if third == SyntaxKind::CommaToken || third == SyntaxKind::EqualsToken {
                        return true;
                    }
                    false
                });
                if is_arrow_function_in_jsx {
                    return Tristate::True;
                }
                return Tristate::False;
            }
            // This *could* be a parenthesized arrow function.
            Tristate::Unknown
        }
    }

    // Go: parser.go:4373 tryParseParenthesizedArrowFunctionExpression
    pub fn try_parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        let tristate = self.is_parenthesized_arrow_function_expression();
        if tristate == Tristate::False {
            // It's definitely not a parenthesized arrow function expression.
            return Node::NIL;
        }
        // If we definitely have an arrow function, then we can just parse one, not requiring a
        // following => or { token. Otherwise, we *might* have an arrow function.  Try to parse
        // it out, but don't allow any ambiguity, and return 'undefined' if this could be an
        // expression instead.
        if tristate == Tristate::True {
            return self.parse_parenthesized_arrow_function_expression(
                true, /*allowAmbiguity*/
                true, /*allowReturnTypeInArrowFunction*/
            );
        }
        let state = self.mark();
        let result = self.parse_possible_parenthesized_arrow_function_expression(
            allow_return_type_in_arrow_function,
        );
        if result.is_nil() {
            self.rewind(state);
        }
        result
    }

    // Go: parser.go:4394 parseParenthesizedArrowFunctionExpression
    pub fn parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_ambiguity: bool,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_arrow_function();
        let is_async = modifier_list_has_async(modifiers);
        let signature_flags = if is_async {
            ParseFlags::AWAIT
        } else {
            ParseFlags::NONE
        };
        // Arrow functions are never generators.
        //
        // If we're speculatively parsing a signature for a parenthesized arrow function, then
        // we have to have a complete parameter list.  Otherwise we might see something like
        // a => (b => c)
        // And think that "(b =>" was actually a parenthesized arrow function with a missing
        // close paren.
        let type_parameters = self.parse_type_parameters();
        let parameters;
        if !self.parse_expected(SyntaxKind::OpenParenToken) {
            if !allow_ambiguity {
                return Node::NIL;
            }
            parameters = self.create_missing_list();
        } else {
            if !allow_ambiguity {
                let maybe_parameters =
                    self.parse_parameters_worker(signature_flags, allow_ambiguity);
                if maybe_parameters.is_nil() {
                    return Node::NIL;
                }
                parameters = maybe_parameters;
            } else {
                parameters = self.parse_parameters_worker(signature_flags, allow_ambiguity);
            }
            if !self.parse_expected(SyntaxKind::CloseParenToken) && !allow_ambiguity {
                return Node::NIL;
            }
        }
        let has_return_colon = self.token == SyntaxKind::ColonToken;
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
        if return_type.is_some()
            && !allow_ambiguity
            && type_has_arrow_function_blocking_parse_error(return_type)
        {
            return Node::NIL;
        }
        // Parsing a signature isn't enough.
        // Parenthesized arrow signatures often look like other valid expressions.
        // For instance:
        //  - "(x = 10)" is an assignment expression parsed as a signature with a default parameter value.
        //  - "(x,y)" is a comma expression parsed as a signature with two parameters.
        //  - "a ? (b): c" will have "(b):" parsed as a signature with a return type annotation.
        //  - "a ? (b): function() {}" will too, since function() is a valid JSDoc function type.
        //  - "a ? (b): (function() {})" as well, but inside of a parenthesized type with an arbitrary amount of nesting.
        //
        // So we need just a bit of lookahead to ensure that it can only be a signature.
        // PORT: Go computes unwrappedType and never reads it. Kept for parity.
        let mut unwrapped_type = return_type;
        while unwrapped_type.is_some() && unwrapped_type.kind() == SyntaxKind::ParenthesizedType {
            unwrapped_type = unwrapped_type.type_(); // Skip parens if need be
        }
        let _ = unwrapped_type;
        if !allow_ambiguity
            && self.token != SyntaxKind::EqualsGreaterThanToken
            && self.token != SyntaxKind::OpenBraceToken
        {
            // Returning undefined here will cause our caller to rewind to where we started from.
            return Node::NIL;
        }
        // If we have an arrow, then try to parse the body. Even if not, try to parse if we
        // have an opening brace, just in case we're in an error state.
        let last_token = self.token;
        let equals_greater_than_token =
            self.parse_expected_token(SyntaxKind::EqualsGreaterThanToken);
        let body = if last_token == SyntaxKind::EqualsGreaterThanToken
            || last_token == SyntaxKind::OpenBraceToken
        {
            self.parse_arrow_function_expression_body(is_async, allow_return_type_in_arrow_function)
        } else {
            self.parse_identifier()
        };
        // Given:
        //     x ? y => ({ y }) : z => ({ z })
        // We try to parse the body of the first arrow function by looking at:
        //     ({ y }) : z => ({ z })
        // This is a valid arrow function with "z" as the return type.
        //
        // But, if we're in the true side of a conditional expression, this colon
        // terminates the expression, so we cannot allow a return type if we aren't
        // certain whether or not the preceding text was parsed as a parameter list.
        //
        // For example,
        //     a() ? (b: number, c?: string): void => d() : e
        // is determined by isParenthesizedArrowFunctionExpression to unambiguously
        // be an arrow expression, so we allow a return type.
        if !allow_return_type_in_arrow_function && has_return_colon {
            // However, if the arrow function we were able to parse is followed by another colon
            // as in:
            //     a ? (x): string => x : null
            // Then allow the arrow function, and treat the second colon as terminating
            // the conditional expression. It's okay to do this because this code would
            // be a syntax error in JavaScript (as the second colon shouldn't be there).
            if self.token != SyntaxKind::ColonToken {
                return Node::NIL;
            }
        }
        let arrow = self.factory.new_arrow_function(
            modifiers,
            type_parameters,
            parameters,
            return_type,
            Node::NIL, /*fullSignature*/
            equals_greater_than_token,
            body,
        );
        let result = self.finish_node(arrow, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser.go:4492 parseModifiersForArrowFunction
    pub fn parse_modifiers_for_arrow_function(&mut self) -> ModifierList {
        if self.token == SyntaxKind::AsyncKeyword {
            let pos = self.node_pos();
            self.next_token();
            let modifier = self.factory.new_modifier(SyntaxKind::AsyncKeyword);
            let modifier = self.finish_node(modifier, pos);
            // PORT: Go allocates the one-element slice from nodeSliceArena.
            return self.new_modifier_list(modifier.loc(), &[modifier]);
        }
        ModifierList::NIL
    }

    // Go: parser.go:4515 parseArrowFunctionExpressionBody
    pub fn parse_arrow_function_expression_body(
        &mut self,
        is_async: bool,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        if self.token == SyntaxKind::OpenBraceToken {
            return self.parse_function_block(
                if is_async {
                    ParseFlags::AWAIT
                } else {
                    ParseFlags::NONE
                },
                None, /*diagnosticMessage*/
            );
        }
        if self.token != SyntaxKind::SemicolonToken
            && self.token != SyntaxKind::FunctionKeyword
            && self.token != SyntaxKind::ClassKeyword
            && self.is_start_of_statement()
            && !self.is_start_of_expression_statement()
        {
            // Check if we got a plain statement (i.e. no expression-statements, no function/class expressions/declarations)
            //
            // Here we try to recover from a potential error situation in the case where the
            // user meant to supply a block. For example, if the user wrote:
            //
            //  a =>
            //      let v = 0;
            //  }
            //
            // they may be missing an open brace.  Check to see if that's the case so we can
            // try to recover better.  If we don't do this, then the next close curly we see may end
            // up preemptively closing the containing construct.
            //
            // Note: even when 'IgnoreMissingOpenBrace' is passed, parseBody will still error.
            return self.parse_function_block(
                ParseFlags::IGNORE_MISSING_OPEN_BRACE
                    | if is_async {
                        ParseFlags::AWAIT
                    } else {
                        ParseFlags::NONE
                    },
                None, /*diagnosticMessage*/
            );
        }
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, is_async);
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, false);
        let node =
            self.parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function);
        self.context_flags = save_context_flags;
        node
    }

    // Go: parser.go:4544 isStartOfExpressionStatement
    pub fn is_start_of_expression_statement(&mut self) -> bool {
        // As per the grammar, none of '{' or 'function' or 'class' can start an expression statement.
        self.token != SyntaxKind::OpenBraceToken
            && self.token != SyntaxKind::FunctionKeyword
            && self.token != SyntaxKind::ClassKeyword
            && self.token != SyntaxKind::AtToken
            && self.is_start_of_expression()
    }

    // Go: parser.go:4549 parsePossibleParenthesizedArrowFunctionExpression
    pub fn parse_possible_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        let token_pos = self.scanner.token_start();
        if self.not_parenthesized_arrow.contains(&token_pos) {
            return Node::NIL;
        }
        let result = self.parse_parenthesized_arrow_function_expression(
            false, /*allowAmbiguity*/
            allow_return_type_in_arrow_function,
        );
        if result.is_nil() {
            self.not_parenthesized_arrow.insert(token_pos);
        }
        result
    }

    // Go: parser.go:4561 tryParseAsyncSimpleArrowFunctionExpression
    pub fn try_parse_async_simple_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        // We do a check here so that we won't be doing unnecessarily call to "lookAhead"
        if self.token == SyntaxKind::AsyncKeyword
            && self.look_ahead(Parser::next_is_un_parenthesized_async_arrow_function)
        {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let async_modifier = self.parse_modifiers_for_arrow_function();
            let expr = self.parse_binary_expression_or_higher(OperatorPrecedence::LOWEST);
            return self.parse_simple_arrow_function_expression(
                pos,
                expr,
                allow_return_type_in_arrow_function,
                jsdoc,
                async_modifier,
            );
        }
        Node::NIL
    }

    // Go: parser.go:4573 nextIsUnParenthesizedAsyncArrowFunction
    pub fn next_is_un_parenthesized_async_arrow_function(&mut self) -> bool {
        // AsyncArrowFunctionExpression:
        //      1) async[no LineTerminator here]AsyncArrowBindingIdentifier[?Yield][no LineTerminator here]=>AsyncConciseBody[?In]
        //      2) CoverCallExpressionAndAsyncArrowHead[?Yield, ?Await][no LineTerminator here]=>AsyncConciseBody[?In]
        if self.token == SyntaxKind::AsyncKeyword {
            self.next_token();
            // If the "async" is followed by "=>" token then it is not a beginning of an async arrow-function
            // but instead a simple arrow-function which will be parsed inside "parseAssignmentExpressionOrHigher"
            if self.has_preceding_line_break() || self.token == SyntaxKind::EqualsGreaterThanToken {
                return false;
            }
            // Check for un-parenthesized AsyncArrowFunction
            if !self.is_identifier() {
                return false;
            }
            self.next_token_without_check();
            return !self.has_preceding_line_break()
                && self.token == SyntaxKind::EqualsGreaterThanToken;
        }
        false
    }

    // Go: parser.go:4594 parseSimpleArrowFunctionExpression
    pub fn parse_simple_arrow_function_expression(
        &mut self,
        pos: i32,
        identifier: Node,
        allow_return_type_in_arrow_function: bool,
        jsdoc: JsdocScannerInfo,
        async_modifier: ModifierList,
    ) -> Node {
        debug_assert!(
            self.token == SyntaxKind::EqualsGreaterThanToken,
            "parseSimpleArrowFunctionExpression should only have been called if we had a =>"
        );
        let parameter = self.factory.new_parameter_declaration(
            ModifierList::NIL, /*modifiers*/
            Node::NIL,         /*dotDotDotToken*/
            identifier,
            Node::NIL, /*questionToken*/
            Node::NIL, /*typeNode*/
            Node::NIL, /*initializer*/
        );
        let parameter = self.finish_node(parameter, identifier.pos());
        let parameters = self.new_node_list(parameter.loc(), &[parameter]);
        let equals_greater_than_token =
            self.parse_expected_token(SyntaxKind::EqualsGreaterThanToken);
        let body = self.parse_arrow_function_expression_body(
            async_modifier.is_some(), /*isAsync*/
            allow_return_type_in_arrow_function,
        );
        let arrow = self.factory.new_arrow_function(
            async_modifier,
            NodeList::NIL, /*typeParameters*/
            parameters,
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            equals_greater_than_token,
            body,
        );
        let result = self.finish_node(arrow, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:4605 parseConditionalExpressionRest
    pub fn parse_conditional_expression_rest(
        &mut self,
        left_operand: Node,
        pos: i32,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        // Note: we are passed in an expression which was produced from parseBinaryExpressionOrHigher.
        let question_token = self.parse_optional_token(SyntaxKind::QuestionToken);
        if question_token.is_nil() {
            return left_operand;
        }
        // Note: we explicitly 'allowIn' in the whenTrue part of the condition expression, and
        // we do not that for the 'whenFalse' part.
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let true_expression = self.parse_assignment_expression_or_higher_worker(
            false, /*allowReturnTypeInArrowFunction*/
        );
        self.context_flags = save_context_flags;
        let colon_token = self.parse_expected_token(SyntaxKind::ColonToken);
        let false_expression = if node_is_present(colon_token) {
            self.parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function)
        } else {
            self.create_missing_identifier()
        };
        let conditional = self.factory.new_conditional_expression(
            left_operand,
            question_token,
            true_expression,
            colon_token,
            false_expression,
        );
        self.finish_node(conditional, pos)
    }

    // Go: parser.go:4627 parseBinaryExpressionOrHigher
    pub fn parse_binary_expression_or_higher(&mut self, precedence: OperatorPrecedence) -> Node {
        let pos = self.node_pos();
        let left_operand = self.parse_unary_expression_or_higher();
        self.parse_binary_expression_rest(precedence, left_operand, pos)
    }

    // Go: parser.go:4642 parseBinaryExpressionRest
    pub fn parse_binary_expression_rest(
        &mut self,
        precedence: OperatorPrecedence,
        mut left_operand: Node,
        pos: i32,
    ) -> Node {
        let mut last_operand = left_operand;
        loop {
            // We either have a binary operator here, or we're finished.  We call
            // reScanGreaterToken so that we merge token sequences like > and = into >=
            let operator = self.re_scan_greater_than_token();
            let new_precedence = get_binary_operator_precedence(operator);
            // Check the precedence to see if we should "take" this operator
            // - For left associative operator (all operator but **), consume the operator,
            //   recursively call the function below, and parse binaryExpression as a rightOperand
            //   of the caller if the new precedence of the operator is greater then or equal to the current precedence.
            //   For example:
            //      a - b - c;
            //            ^token; leftOperand = b. Return b to the caller as a rightOperand
            //      a * b - c
            //            ^token; leftOperand = b. Return b to the caller as a rightOperand
            //      a - b * c;
            //            ^token; leftOperand = b. Return b * c to the caller as a rightOperand
            // - For right associative operator (**), consume the operator, recursively call the function
            //   and parse binaryExpression as a rightOperand of the caller if the new precedence of
            //   the operator is strictly grater than the current precedence
            //   For example:
            //      a ** b ** c;
            //             ^^token; leftOperand = b. Return b ** c to the caller as a rightOperand
            //      a - b ** c;
            //            ^^token; leftOperand = b. Return b ** c to the caller as a rightOperand
            //      a ** b - c
            //             ^token; leftOperand = b. Return b to the caller as a rightOperand
            if !should_consume_binary_operator(operator, new_precedence, precedence) {
                break;
            }
            if operator == SyntaxKind::InKeyword && self.in_disallow_in_context() {
                break;
            }
            if operator == SyntaxKind::AsKeyword || operator == SyntaxKind::SatisfiesKeyword {
                // Make sure we *do* perform ASI for constructs like this:
                //    var x = foo
                //    as (Bar)
                // This should be parsed as an initialized variable, followed
                // by a function call to 'as' with the argument 'Bar'
                if self.has_preceding_line_break() {
                    break;
                } else {
                    self.next_token();
                    // When we have 'a ## b as SomeType $$ c' or 'a ## b satisfies SomeType $$ c', where ## and $$
                    // are binary operators, we want to stop parsing when $$ would bind before ## after erasing the
                    // assertion. See https://github.com/microsoft/TypeScript/issues/63527.
                    let mut last_precedence = OperatorPrecedence::HIGHEST;
                    if is_binary_expression(last_operand) {
                        last_precedence =
                            get_binary_operator_precedence(last_operand.operator_token().kind());
                    }
                    if operator == SyntaxKind::SatisfiesKeyword {
                        let type_node = self.parse_type();
                        left_operand = self.make_satisfies_expression(left_operand, type_node);
                    } else {
                        let type_node = self.parse_type();
                        left_operand = self.make_as_expression(left_operand, type_node);
                    }
                    // Stop if the next operator would bind before the last operator when the assertion is erased.
                    let next_operator = self.re_scan_greater_than_token();
                    let next_precedence = get_binary_operator_precedence(next_operator);
                    if should_consume_binary_operator(
                        next_operator,
                        next_precedence,
                        last_precedence,
                    ) {
                        break;
                    }
                }
            } else {
                let operator_token = self.parse_token_node();
                let right = self.parse_binary_expression_or_higher(new_precedence);
                left_operand =
                    self.make_binary_expression(left_operand, operator_token, right, pos);
                last_operand = left_operand;
            }
        }
        left_operand
    }

    // Go: parser.go:4713 makeSatisfiesExpression
    pub fn make_satisfies_expression(&mut self, expression: Node, type_node: Node) -> Node {
        let node = self.factory.new_satisfies_expression(expression, type_node);
        let node = self.finish_node(node, expression.pos());
        self.check_js_syntax(node)
    }

    // Go: parser.go:4717 makeAsExpression
    pub fn make_as_expression(&mut self, left: Node, right: Node) -> Node {
        let node = self.factory.new_as_expression(left, right);
        let node = self.finish_node(node, left.pos());
        self.check_js_syntax(node)
    }

    // Go: parser.go:4721 makeBinaryExpression
    pub fn make_binary_expression(
        &mut self,
        left: Node,
        operator_token: Node,
        right: Node,
        pos: i32,
    ) -> Node {
        let node = self.factory.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            operator_token,
            right,
        );
        self.finish_node(node, pos)
    }

    // Go: parser.go:4725 parseUnaryExpressionOrHigher
    pub fn parse_unary_expression_or_higher(&mut self) -> Node {
        // ES7 UpdateExpression:
        //      1) LeftHandSideExpression[?Yield]
        //      2) LeftHandSideExpression[?Yield][no LineTerminator here]++
        //      3) LeftHandSideExpression[?Yield][no LineTerminator here]--
        //      4) ++UnaryExpression[?Yield]
        //      5) --UnaryExpression[?Yield]
        if self.is_update_expression() {
            let pos = self.node_pos();
            let update_expression = self.parse_update_expression();
            if self.token == SyntaxKind::AsteriskAsteriskToken {
                return self.parse_binary_expression_rest(
                    get_binary_operator_precedence(self.token),
                    update_expression,
                    pos,
                );
            }
            return update_expression;
        }
        // ES7 UnaryExpression:
        //      1) UpdateExpression[?yield]
        //      2) delete UpdateExpression[?yield]
        //      3) void UpdateExpression[?yield]
        //      4) typeof UpdateExpression[?yield]
        //      5) + UpdateExpression[?yield]
        //      6) - UpdateExpression[?yield]
        //      7) ~ UpdateExpression[?yield]
        //      8) ! UpdateExpression[?yield]
        let unary_operator = self.token;
        let simple_unary_expression = self.parse_simple_unary_expression();
        if self.token == SyntaxKind::AsteriskAsteriskToken {
            let pos = skip_trivia(self.source_text, simple_unary_expression.pos());
            let end = simple_unary_expression.end();
            if simple_unary_expression.kind() == SyntaxKind::TypeAssertionExpression {
                self.parse_error_at(
                    pos,
                    end,
                    diag::A_type_assertion_expression_is_not_allowed_in_the_left_hand_side_of_an_exponentiation_expression_Consider_enclosing_the_expression_in_parentheses,
                    args![],
                );
            } else {
                debug_assert!(is_keyword_or_punctuation(unary_operator));
                self.parse_error_at(
                    pos,
                    end,
                    diag::An_unary_expression_with_the_0_operator_is_not_allowed_in_the_left_hand_side_of_an_exponentiation_expression_Consider_enclosing_the_expression_in_parentheses,
                    args![token_to_string(unary_operator)],
                );
            }
        }
        simple_unary_expression
    }

    // Go: parser.go:4764 isUpdateExpression
    pub fn is_update_expression(&self) -> bool {
        match self.token {
            SyntaxKind::PlusToken
            | SyntaxKind::MinusToken
            | SyntaxKind::TildeToken
            | SyntaxKind::ExclamationToken
            | SyntaxKind::DeleteKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::AwaitKeyword => false,
            SyntaxKind::LessThanToken => self.language_variant == LanguageVariant::JSX,
            _ => true,
        }
    }

    // Go: parser.go:4774 parseUpdateExpression
    pub fn parse_update_expression(&mut self) -> Node {
        let pos = self.node_pos();
        if self.token == SyntaxKind::PlusPlusToken || self.token == SyntaxKind::MinusMinusToken {
            let operator = self.token;
            self.next_token();
            let operand = self.parse_left_hand_side_expression_or_higher();
            let node = self.factory.new_prefix_unary_expression(operator, operand);
            return self.finish_node(node, pos);
        } else if self.language_variant == LanguageVariant::JSX
            && self.token == SyntaxKind::LessThanToken
            && self.look_ahead(Parser::next_token_is_identifier_or_keyword_or_greater_than)
        {
            // JSXElement is part of primaryExpression
            return self.parse_jsx_element_or_self_closing_element_or_fragment(
                true,      /*inExpressionContext*/
                -1,        /*topInvalidNodePosition*/
                Node::NIL, /*openingTag*/
                false,     /*mustBeUnary*/
            );
        }
        let expression = self.parse_left_hand_side_expression_or_higher();
        if (self.token == SyntaxKind::PlusPlusToken || self.token == SyntaxKind::MinusMinusToken)
            && !self.has_preceding_line_break()
        {
            let operator = self.token;
            self.next_token();
            let node = self
                .factory
                .new_postfix_unary_expression(expression, operator);
            return self.finish_node(node, pos);
        }
        expression
    }

    // Go: parser.go:4793 parseJsxElementOrSelfClosingElementOrFragment
    pub fn parse_jsx_element_or_self_closing_element_or_fragment(
        &mut self,
        in_expression_context: bool,
        top_invalid_node_position: i32,
        opening_tag: Node,
        must_be_unary: bool,
    ) -> Node {
        let pos = self.node_pos();
        let opening = self
            .parse_jsx_opening_or_self_closing_element_or_opening_fragment(in_expression_context);
        let mut result;
        match opening.kind() {
            SyntaxKind::JsxOpeningElement => {
                let mut children = self.parse_jsx_children(opening);
                let closing_element;
                let last_child = children.nodes().last().unwrap_or(Node::NIL);
                if last_child.is_some()
                    && last_child.kind() == SyntaxKind::JsxElement
                    && !tag_names_are_equivalent(
                        last_child.opening_element().tag_name(),
                        last_child.closing_element().tag_name(),
                    )
                    && tag_names_are_equivalent(
                        opening.tag_name(),
                        last_child.closing_element().tag_name(),
                    )
                {
                    // when an unclosed JsxOpeningElement incorrectly parses its parent's JsxClosingElement,
                    // restructure (<div>(...<span>...</div>)) --> (<div>(...<span>...</>)</div>)
                    // (no need to error; the parent will error)
                    let end = last_child.children().end();
                    let missing_identifier = self.new_identifier("");
                    let missing_identifier =
                        self.finish_node_with_end(missing_identifier, end, end);
                    let new_closing_element =
                        self.factory.new_jsx_closing_element(missing_identifier);
                    let new_closing_element =
                        self.finish_node_with_end(new_closing_element, end, end);
                    let new_last = self.factory.new_jsx_element(
                        last_child.opening_element(),
                        last_child.children(),
                        new_closing_element,
                    );
                    let new_last = self.finish_node_with_end(
                        new_last,
                        last_child.opening_element().pos(),
                        end,
                    );
                    // force reset parent pointers from discarded parse result
                    if last_child.opening_element().is_some() {
                        set_node_parent(last_child.opening_element(), new_last);
                    }
                    if last_child.children().is_some() {
                        for c in last_child.children().nodes().iter() {
                            set_node_parent(c, new_last);
                        }
                    }
                    set_node_parent(new_closing_element, new_last);
                    let mut nodes = children.nodes().to_vec();
                    nodes.pop();
                    nodes.push(new_last);
                    children =
                        self.new_node_list(TextRange::new(children.pos(), new_last.end()), &nodes);
                    closing_element = last_child.closing_element();
                } else {
                    closing_element =
                        self.parse_jsx_closing_element(opening, in_expression_context);
                    if !tag_names_are_equivalent(opening.tag_name(), closing_element.tag_name()) {
                        if opening_tag.is_some()
                            && is_jsx_opening_element(opening_tag)
                            && tag_names_are_equivalent(
                                closing_element.tag_name(),
                                opening_tag.tag_name(),
                            )
                        {
                            // opening incorrectly matched with its parent's closing -- put error on opening
                            let text = get_text_of_node_from_source_text(
                                self.source_text,
                                opening.tag_name(),
                                false, /*includeTrivia*/
                            );
                            self.parse_error_at_range(
                                opening.tag_name().loc(),
                                diag::JSX_element_0_has_no_corresponding_closing_tag,
                                args![text],
                            );
                        } else {
                            // other opening/closing mismatches -- put error on closing
                            let text = get_text_of_node_from_source_text(
                                self.source_text,
                                opening.tag_name(),
                                false, /*includeTrivia*/
                            );
                            self.parse_error_at_range(
                                closing_element.tag_name().loc(),
                                diag::Expected_corresponding_JSX_closing_tag_for_0,
                                args![text],
                            );
                        }
                    }
                }
                let element = self
                    .factory
                    .new_jsx_element(opening, children, closing_element);
                result = self.finish_node(element, pos);
                set_node_parent(closing_element, result); // force reset parent pointers from possibly discarded parse result
            }
            SyntaxKind::JsxOpeningFragment => {
                let children = self.parse_jsx_children(opening);
                let closing_fragment = self.parse_jsx_closing_fragment(in_expression_context);
                let fragment = self
                    .factory
                    .new_jsx_fragment(opening, children, closing_fragment);
                result = self.finish_node(fragment, pos);
            }
            SyntaxKind::JsxSelfClosingElement => {
                // Nothing else to do for self-closing elements
                result = opening;
            }
            _ => panic!("Unhandled case in parseJsxElementOrSelfClosingElementOrFragment"),
        }
        // If the user writes the invalid code '<div></div><div></div>' in an expression context (i.e. not wrapped in
        // an enclosing tag), we'll naively try to parse   ^ this as a 'less than' operator and the remainder of the tag
        // as garbage, which will cause the formatter to badly mangle the JSX. Perform a speculative parse of a JSX
        // element if we see a < token so that we can wrap it in a synthetic binary expression so the formatter
        // does less damage and we can report a better error.
        // Since JSX elements are invalid < operands anyway, this lookahead parse will only occur in error scenarios
        // of one sort or another.
        // If we are in a unary context, we can't do this recovery; the binary expression we return here is not
        // a valid UnaryExpression and will cause problems later.
        if !must_be_unary && in_expression_context && self.token == SyntaxKind::LessThanToken {
            let mut top_bad_pos = top_invalid_node_position;
            if top_bad_pos < 0 {
                top_bad_pos = result.pos();
            }
            let invalid_element = self.parse_jsx_element_or_self_closing_element_or_fragment(
                true, /*inExpressionContext*/
                top_bad_pos,
                Node::NIL,
                false,
            );
            let operator_token = self.factory.new_token(SyntaxKind::CommaToken);
            set_node_loc(
                operator_token,
                TextRange::new(invalid_element.pos(), invalid_element.pos()),
            );
            self.parse_error_at(
                skip_trivia(self.source_text, top_bad_pos),
                invalid_element.end(),
                diag::JSX_expressions_must_have_one_parent_element,
                args![],
            );
            let binary = self.factory.new_binary_expression(
                ModifierList::NIL, /*modifiers*/
                result,
                Node::NIL, /*typeNode*/
                operator_token,
                invalid_element,
            );
            result = self.finish_node(binary, pos);
        }
        result
    }

    // Go: parser.go:4873 parseJsxChildren
    pub fn parse_jsx_children(&mut self, opening_tag: Node) -> NodeList {
        let pos = self.node_pos();
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << (ParsingContext::JsxChildren as i32);
        let mut list: Vec<Node> = Vec::new();
        loop {
            let current_token = self
                .scanner
                .re_scan_jsx_token(true /*allowMultilineJsxText*/);
            let child = self.parse_jsx_child(opening_tag, current_token);
            if child.is_nil() {
                break;
            }
            list.push(child);
            if is_jsx_opening_element(opening_tag)
                && child.kind() == SyntaxKind::JsxElement
                && !tag_names_are_equivalent(
                    child.opening_element().tag_name(),
                    child.closing_element().tag_name(),
                )
                && tag_names_are_equivalent(
                    opening_tag.tag_name(),
                    child.closing_element().tag_name(),
                )
            {
                // stop after parsing a mismatched child like <div>...(<span></div>) in order to reattach the </div> higher
                break;
            }
        }
        self.parsing_contexts = save_parsing_contexts;
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &list)
    }

    // Go: parser.go:4896 parseJsxChild
    pub fn parse_jsx_child(&mut self, opening_tag: Node, token: SyntaxKind) -> Node {
        match token {
            SyntaxKind::EndOfFile => {
                // If we hit EOF, issue the error at the tag that lacks the closing element
                // rather than at the end of the file (which is useless)
                if is_jsx_opening_fragment(opening_tag) {
                    self.parse_error_at_range(
                        opening_tag.loc(),
                        diag::JSX_fragment_has_no_corresponding_closing_tag,
                        args![],
                    );
                } else {
                    // We want the error span to cover only 'Foo.Bar' in < Foo.Bar >
                    // or to cover only 'Foo' in < Foo >
                    let tag = opening_tag.tag_name();
                    let start = skip_trivia(self.source_text, tag.pos()).min(tag.end());
                    let text = get_text_of_node_from_source_text(
                        self.source_text,
                        opening_tag.tag_name(),
                        false, /*includeTrivia*/
                    );
                    self.parse_error_at(
                        start,
                        tag.end(),
                        diag::JSX_element_0_has_no_corresponding_closing_tag,
                        args![text],
                    );
                }
                Node::NIL
            }
            SyntaxKind::LessThanSlashToken | SyntaxKind::ConflictMarkerTrivia => Node::NIL,
            SyntaxKind::JsxText | SyntaxKind::JsxTextAllWhiteSpaces => self.parse_jsx_text(),
            SyntaxKind::OpenBraceToken => {
                self.parse_jsx_expression(false /*inExpressionContext*/)
            }
            SyntaxKind::LessThanToken => self
                .parse_jsx_element_or_self_closing_element_or_fragment(
                    false, /*inExpressionContext*/
                    -1,    /*topInvalidNodePosition*/
                    opening_tag,
                    false,
                ),
            _ => panic!("Unhandled case in parseJsxChild"),
        }
    }

    // Go: parser.go:4924 parseJsxText
    pub fn parse_jsx_text(&mut self) -> Node {
        let pos = self.node_pos();
        let text = self.scanner.token_value();
        let result = self
            .factory
            .new_jsx_text(text, self.token == SyntaxKind::JsxTextAllWhiteSpaces);
        self.scan_jsx_text();
        self.finish_node(result, pos)
    }

    // Go: parser.go:4931 parseJsxExpression
    pub fn parse_jsx_expression(&mut self, in_expression_context: bool) -> Node {
        let pos = self.node_pos();
        if !self.parse_expected(SyntaxKind::OpenBraceToken) {
            return Node::NIL;
        }
        let mut dot_dot_dot_token = Node::NIL;
        let mut expression = Node::NIL;
        if self.token != SyntaxKind::CloseBraceToken {
            if !in_expression_context {
                dot_dot_dot_token = self.parse_optional_token(SyntaxKind::DotDotDotToken);
            }
            // Only an AssignmentExpression is valid here per the JSX spec,
            // but we can unambiguously parse a comma sequence and provide
            // a better error message in grammar checking.
            expression = self.parse_expression();
        }
        if in_expression_context {
            self.parse_expected(SyntaxKind::CloseBraceToken);
        } else if self.parse_expected_without_advancing(SyntaxKind::CloseBraceToken) {
            self.scan_jsx_text();
        }
        let node = self
            .factory
            .new_jsx_expression(dot_dot_dot_token, expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:4955 scanJsxText
    pub fn scan_jsx_text(&mut self) -> SyntaxKind {
        self.token = self.scanner.scan_jsx_token();
        self.token
    }

    // Go: parser.go:4960 scanJsxIdentifier
    pub fn scan_jsx_identifier(&mut self) -> SyntaxKind {
        self.token = self.scanner.scan_jsx_identifier();
        self.token
    }

    // Go: parser.go:4965 scanJsxAttributeValue
    pub fn scan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.token = self.scanner.scan_jsx_attribute_value();
        self.token
    }

    // Go: parser.go:4970 parseJsxClosingElement
    pub fn parse_jsx_closing_element(&mut self, open: Node, in_expression_context: bool) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanSlashToken);
        let tag_name = self.parse_jsx_element_name();
        if self.parse_expected_with_diagnostic(
            SyntaxKind::GreaterThanToken,
            None,  /*diagnosticMessage*/
            false, /*shouldAdvance*/
        ) {
            // manually advance the scanner in order to look for jsx text inside jsx
            if in_expression_context || !tag_names_are_equivalent(open.tag_name(), tag_name) {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        let node = self.factory.new_jsx_closing_element(tag_name);
        self.finish_node(node, pos)
    }

    // Go: parser.go:4985 parseJsxOpeningOrSelfClosingElementOrOpeningFragment
    pub fn parse_jsx_opening_or_self_closing_element_or_opening_fragment(
        &mut self,
        in_expression_context: bool,
    ) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanToken);
        if self.token == SyntaxKind::GreaterThanToken {
            // See below for explanation of scanJsxText
            self.scan_jsx_text();
            let node = self.factory.new_jsx_opening_fragment();
            return self.finish_node(node, pos);
        }
        let tag_name = self.parse_jsx_element_name();
        let mut type_arguments = NodeList::NIL;
        if !self.context_flags.intersects(NodeFlags::JAVA_SCRIPT_FILE) {
            type_arguments = self.parse_type_arguments();
        }
        let attributes = self.parse_jsx_attributes();
        let result;
        if self.token == SyntaxKind::GreaterThanToken {
            // Closing tag, so scan the immediately-following text with the JSX scanning instead
            // of regular scanning to avoid treating illegal characters (e.g. '#') as immediate
            // scanning errors
            self.scan_jsx_text();
            result = self
                .factory
                .new_jsx_opening_element(tag_name, type_arguments, attributes);
        } else {
            self.parse_expected(SyntaxKind::SlashToken);
            if self.parse_expected_without_advancing(SyntaxKind::GreaterThanToken) {
                if in_expression_context {
                    self.next_token();
                } else {
                    self.scan_jsx_text();
                }
            }
            result =
                self.factory
                    .new_jsx_self_closing_element(tag_name, type_arguments, attributes);
        }
        self.finish_node(result, pos)
    }

    // Go: parser.go:5020 parseJsxElementName
    pub fn parse_jsx_element_name(&mut self) -> Node {
        let pos = self.node_pos();
        // JsxElement can have name in the form of
        //      propertyAccessExpression
        //      primaryExpression in the form of an identifier and "this" keyword
        // We can't just simply use parseLeftHandSideExpressionOrHigher because then we will start consider class,function etc as a keyword
        // We only want to consider "this" as a primaryExpression
        let initial_expression = self.parse_jsx_tag_name();
        if is_jsx_namespaced_name(initial_expression) {
            return initial_expression; // `a:b.c` is invalid syntax, don't even look for the `.` if we parse `a:b`, and let `parseAttribute` report "unexpected :" instead.
        }
        let mut expression = initial_expression;
        while self.parse_optional(SyntaxKind::DotToken) {
            let name = self.parse_right_side_of_dot(
                true,  /*allowIdentifierNames*/
                false, /*allowPrivateIdentifiers*/
                false, /*allowUnicodeEscapeSequenceInIdentifierName*/
            );
            let node = self.factory.new_property_access_expression(
                expression,
                Node::NIL,
                name,
                NodeFlags::NONE,
            );
            expression = self.finish_node(node, pos);
        }
        expression
    }

    // Go: parser.go:5038 parseJsxTagName
    pub fn parse_jsx_tag_name(&mut self) -> Node {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let is_this = self.token == SyntaxKind::ThisKeyword;
        let tag_name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(SyntaxKind::ColonToken) {
            self.scan_jsx_identifier();
            let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
            let node = self.factory.new_jsx_namespaced_name(tag_name, name);
            return self.finish_node(node, pos);
        }
        if is_this {
            let result = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
            return self.finish_node(result, pos);
        }
        tag_name
    }

    // Go: parser.go:5054 parseJsxAttributes
    pub fn parse_jsx_attributes(&mut self) -> Node {
        let pos = self.node_pos();
        let properties =
            self.parse_list(ParsingContext::JsxAttributes, Parser::parse_jsx_attribute);
        let node = self.factory.new_jsx_attributes(properties);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5059 parseJsxAttribute
    pub fn parse_jsx_attribute(&mut self) -> Node {
        if self.token == SyntaxKind::OpenBraceToken {
            return self.parse_jsx_spread_attribute();
        }
        let pos = self.node_pos();
        let name = self.parse_jsx_attribute_name();
        let initializer = self.parse_jsx_attribute_value();
        let node = self.factory.new_jsx_attribute(name, initializer);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5067 parseJsxSpreadAttribute
    pub fn parse_jsx_spread_attribute(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBraceToken);
        self.parse_expected(SyntaxKind::DotDotDotToken);
        let expression = self.parse_expression();
        self.parse_expected(SyntaxKind::CloseBraceToken);
        let node = self.factory.new_jsx_spread_attribute(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5076 parseJsxAttributeName
    pub fn parse_jsx_attribute_name(&mut self) -> Node {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let attr_name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(SyntaxKind::ColonToken) {
            self.scan_jsx_identifier();
            let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
            let node = self.factory.new_jsx_namespaced_name(attr_name, name);
            return self.finish_node(node, pos);
        }
        attr_name
    }

    // Go: parser.go:5087 parseJsxAttributeValue
    pub fn parse_jsx_attribute_value(&mut self) -> Node {
        if self.token == SyntaxKind::EqualsToken {
            if self.scan_jsx_attribute_value() == SyntaxKind::StringLiteral {
                return self.parse_literal_expression();
            }
            if self.token == SyntaxKind::OpenBraceToken {
                return self.parse_jsx_expression(true /*inExpressionContext*/);
            }
            if self.token == SyntaxKind::LessThanToken {
                // An attribute value must be a single JsxAttributeValue, so don't allow the sibling-element
                // recovery to wrap it in a synthetic binary expression.
                return self.parse_jsx_element_or_self_closing_element_or_fragment(
                    true,      /*inExpressionContext*/
                    -1,        /*topInvalidNodePosition*/
                    Node::NIL, /*openingTag*/
                    true,      /*mustBeUnary*/
                );
            }
            self.parse_error_at_current_token(diag::X_or_JSX_element_expected, args![]);
        }
        Node::NIL
    }

    // Go: parser.go:5105 parseJsxClosingFragment
    pub fn parse_jsx_closing_fragment(&mut self, in_expression_context: bool) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanSlashToken);
        if self.parse_expected_with_diagnostic(
            SyntaxKind::GreaterThanToken,
            Some(diag::Expected_corresponding_closing_tag_for_JSX_fragment),
            false, /*shouldAdvance*/
        ) {
            // manually advance the scanner in order to look for jsx text inside jsx
            if in_expression_context {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        let node = self.factory.new_jsx_closing_fragment();
        self.finish_node(node, pos)
    }

    // Go: parser.go:5119 parseSimpleUnaryExpression
    pub fn parse_simple_unary_expression(&mut self) -> Node {
        match { self.token } {
            SyntaxKind::PlusToken
            | SyntaxKind::MinusToken
            | SyntaxKind::TildeToken
            | SyntaxKind::ExclamationToken => self.parse_prefix_unary_expression(),
            SyntaxKind::DeleteKeyword => self.parse_delete_expression(),
            SyntaxKind::TypeOfKeyword => self.parse_type_of_expression(),
            SyntaxKind::VoidKeyword => self.parse_void_expression(),
            SyntaxKind::LessThanToken => {
                // Just like in parseUpdateExpression, we need to avoid parsing type assertions when
                // in JSX and we see an expression like "+ <foo> bar".
                if self.language_variant == LanguageVariant::JSX {
                    return self.parse_jsx_element_or_self_closing_element_or_fragment(
                        true,      /*inExpressionContext*/
                        -1,        /*topInvalidNodePosition*/
                        Node::NIL, /*openingTag*/
                        true,      /*mustBeUnary*/
                    );
                }
                // // This is modified UnaryExpression grammar in TypeScript
                // //  UnaryExpression (modified):
                // //      < type > UnaryExpression
                self.parse_type_assertion()
            }
            // PORT: Go falls through from the await case to the default case.
            SyntaxKind::AwaitKeyword if self.is_await_expression() => self.parse_await_expression(),
            _ => self.parse_update_expression(),
        }
    }

    // Go: parser.go:5149 parsePrefixUnaryExpression
    pub fn parse_prefix_unary_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let operator = self.token;
        self.next_token();
        let operand = self.parse_simple_unary_expression();
        let node = self.factory.new_prefix_unary_expression(operator, operand);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5156 parseDeleteExpression
    pub fn parse_delete_expression(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_delete_expression(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5162 parseTypeOfExpression
    pub fn parse_type_of_expression(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_type_of_expression(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5168 parseVoidExpression
    pub fn parse_void_expression(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_void_expression(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5174 isAwaitExpression
    pub fn is_await_expression(&mut self) -> bool {
        if self.token == SyntaxKind::AwaitKeyword {
            if self.in_await_context() {
                return true;
            }
            // here we are using similar heuristics as 'isYieldExpression'
            return self
                .look_ahead(Parser::next_token_is_identifier_or_keyword_or_literal_on_same_line);
        }
        false
    }

    // Go: parser.go:5185 parseAwaitExpression
    pub fn parse_await_expression(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_await_expression(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5191 parseTypeAssertion
    pub fn parse_type_assertion(&mut self) -> Node {
        debug_assert!(
            self.language_variant != LanguageVariant::JSX,
            "Type assertions should never be parsed in JSX; they should be parsed as comparisons or JSX elements/fragments."
        );
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanToken);
        let type_node = self.parse_type();
        self.parse_expected(SyntaxKind::GreaterThanToken);
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_type_assertion(type_node, expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5222 parseLeftHandSideExpressionOrHigher
    pub fn parse_left_hand_side_expression_or_higher(&mut self) -> Node {
        // Original Ecma:
        // LeftHandSideExpression: See 11.2
        //      NewExpression
        //      CallExpression
        //
        // Our simplification:
        //
        // LeftHandSideExpression: See 11.2
        //      MemberExpression
        //      CallExpression
        //
        // See comment in parseMemberExpressionOrHigher on how we replaced NewExpression with
        // MemberExpression to make our lives easier.
        //
        // to best understand the below code, it's important to see how CallExpression expands
        // out into its own productions:
        //
        // CallExpression:
        //      MemberExpression Arguments
        //      CallExpression Arguments
        //      CallExpression[Expression]
        //      CallExpression.IdentifierName
        //      import (AssignmentExpression)
        //      super Arguments
        //      super.IdentifierName
        //
        // Because of the recursion in these calls, we need to bottom out first. There are three
        // bottom out states we can run into: 1) We see 'super' which must start either of
        // the last two CallExpression productions. 2) We see 'import' which must start import call.
        // 3)we have a MemberExpression which either completes the LeftHandSideExpression,
        // or starts the beginning of the first four CallExpression productions.
        let pos = self.node_pos();
        let expression;
        if self.token == SyntaxKind::ImportKeyword {
            if self.look_ahead(Parser::next_token_is_open_paren_or_less_than) {
                // We don't want to eagerly consume all import keyword as import call expression so we look ahead to find "("
                // For example:
                //      var foo3 = require("subfolder
                //      import * as foo1 from "module-from-node
                // We want this import to be a statement rather than import call expression
                self.source_flags = self.source_flags | NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                expression = self.parse_keyword_expression();
            } else if self.look_ahead(Parser::next_token_is_dot) {
                // This is an 'import.*' metaproperty (i.e. 'import.meta')
                self.next_token(); // advance past the 'import'
                self.next_token(); // advance past the dot
                let name = self.parse_import_meta_property_name();
                let node = self
                    .factory
                    .new_meta_property(SyntaxKind::ImportKeyword, name);
                expression = self.finish_node(node, pos);
                if is_import_phase_meta_property(expression) {
                    if self.token == SyntaxKind::OpenParenToken
                        || self.token == SyntaxKind::LessThanToken
                    {
                        self.source_flags =
                            self.source_flags | NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                    }
                } else {
                    self.source_flags =
                        self.source_flags | NodeFlags::POSSIBLY_CONTAINS_IMPORT_META;
                }
            } else {
                expression = self.parse_member_expression_or_higher();
            }
        } else if self.token == SyntaxKind::SuperKeyword {
            expression = self.parse_super_expression();
        } else {
            expression = self.parse_member_expression_or_higher();
        }
        // Now, we *may* be complete.  However, we might have consumed the start of a
        // CallExpression or OptionalExpression.  As such, we need to consume the rest
        // of it here to be complete.
        self.parse_call_expression_rest(pos, expression)
    }

    // Go: parser.go:5291 nextTokenIsDot
    pub fn next_token_is_dot(&mut self) -> bool {
        self.next_token() == SyntaxKind::DotToken
    }

    // Go: parser.go:5295 parseImportMetaPropertyName (ts#63915)
    pub fn parse_import_meta_property_name(&mut self) -> Node {
        if matches!(
            self.token,
            SyntaxKind::DeferKeyword | SyntaxKind::SourceKeyword
        ) && self.current_import_phase_modifier() == SyntaxKind::Unknown
        {
            self.parse_error_at_current_token(
                diag::Keywords_cannot_contain_escape_characters,
                args![],
            );
        }
        self.parse_identifier_name()
    }

    // Go: parser.go:5274 parseSuperExpression
    pub fn parse_super_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let mut expression = self.parse_keyword_expression();
        if self.token == SyntaxKind::LessThanToken {
            let start_pos = self.node_pos();
            let type_arguments = self.try_parse_type_arguments_in_expression();
            if type_arguments.is_some() {
                let end = self.node_pos();
                self.parse_error_at(
                    start_pos,
                    end,
                    diag::X_super_may_not_use_type_arguments,
                    args![],
                );
                if !self.is_template_start_of_tagged_template() {
                    let node = self
                        .factory
                        .new_expression_with_type_arguments(expression, type_arguments);
                    expression = self.finish_node(node, pos);
                }
            }
        }
        if self.token == SyntaxKind::OpenParenToken
            || self.token == SyntaxKind::DotToken
            || self.token == SyntaxKind::OpenBracketToken
        {
            return expression;
        }
        // If we have seen "super" it must be followed by '(' or '.'.
        // If it wasn't then just try to parse out a '.' and report an error.
        self.parse_error_at_current_token(
            diag::X_super_must_be_followed_by_an_argument_list_or_member_access,
            args![],
        );
        // private names will never work with `super` (`super.#foo`), but that's a semantic error, not syntactic
        let name = self.parse_right_side_of_dot(
            true, /*allowIdentifierNames*/
            true, /*allowPrivateIdentifiers*/
            true, /*allowUnicodeEscapeSequenceInIdentifierName*/
        );
        let node = self.factory.new_property_access_expression(
            expression,
            Node::NIL, /*questionDotToken*/
            name,
            NodeFlags::NONE,
        );
        self.finish_node(node, pos)
    }

    // Go: parser.go:5297 isTemplateStartOfTaggedTemplate
    pub fn is_template_start_of_tagged_template(&self) -> bool {
        self.token == SyntaxKind::NoSubstitutionTemplateLiteral
            || self.token == SyntaxKind::TemplateHead
    }

    // Go: parser.go:5301 tryParseTypeArgumentsInExpression
    pub fn try_parse_type_arguments_in_expression(&mut self) -> NodeList {
        // TypeArguments must not be parsed in JavaScript files to avoid ambiguity with binary operators.
        // Check the cheap preconditions before saving the parser state: unless the current token is `<`
        // (or `<<`, which reScanLessThanToken would split), there is nothing to speculatively parse and
        // the mark/rewind would be a no-op.
        if self.context_flags.intersects(NodeFlags::JAVA_SCRIPT_FILE)
            || (self.token != SyntaxKind::LessThanToken
                && self.token != SyntaxKind::LessThanLessThanToken)
        {
            return NodeList::NIL;
        }
        let state = self.mark();
        if self.re_scan_less_than_token() == SyntaxKind::LessThanToken {
            self.next_token();
            let type_arguments =
                self.parse_delimited_list(ParsingContext::TypeArguments, Parser::parse_type);
            // If it doesn't have the closing `>` then it's definitely not an type argument list.
            if self.re_scan_greater_than_token() == SyntaxKind::GreaterThanToken {
                self.next_token();
                // We successfully parsed a type argument list. The next token determines whether we want to
                // treat it as such. If the type argument list is followed by `(` or a template literal, as in
                // `f<number>(42)`, we favor the type argument interpretation even though JavaScript would view
                // it as a relational expression.
                if self.can_follow_type_arguments_in_expression() {
                    return type_arguments;
                }
            }
        }
        self.rewind(state);
        NodeList::NIL
    }

    // Go: parser.go:5329 canFollowTypeArgumentsInExpression
    pub fn can_follow_type_arguments_in_expression(&mut self) -> bool {
        match self.token {
            // These tokens can follow a type argument list in a call expression:
            // foo<x>(
            // foo<T> `...`
            // foo<T> `...${100}...`
            SyntaxKind::OpenParenToken
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead => return true,
            // A type argument list followed by `<` never makes sense, and a type argument list followed
            // by `>` is ambiguous with a (re-scanned) `>>` operator, so we disqualify both. Also, in
            // this context, `+` and `-` are unary operators, not binary operators.
            SyntaxKind::LessThanToken
            | SyntaxKind::GreaterThanToken
            | SyntaxKind::PlusToken
            | SyntaxKind::MinusToken => return false,
            _ => {}
        }
        // We favor the type argument list interpretation when it is immediately followed by
        // a line break, a binary operator, or something that can't start an expression.
        self.has_preceding_line_break()
            || self.is_binary_operator()
            || !self.is_start_of_expression()
    }

    // Go: parser.go:5348 parseMemberExpressionOrHigher
    pub fn parse_member_expression_or_higher(&mut self) -> Node {
        // Note: to make our lives simpler, we decompose the NewExpression productions and
        // place ObjectCreationExpression and FunctionExpression into PrimaryExpression.
        // like so:
        //
        //   PrimaryExpression : See 11.1
        //      this
        //      Identifier
        //      Literal
        //      ArrayLiteral
        //      ObjectLiteral
        //      (Expression)
        //      FunctionExpression
        //      new MemberExpression Arguments?
        //
        //   MemberExpression : See 11.2
        //      PrimaryExpression
        //      MemberExpression[Expression]
        //      MemberExpression.IdentifierName
        //
        //   CallExpression : See 11.2
        //      MemberExpression
        //      CallExpression Arguments
        //      CallExpression[Expression]
        //      CallExpression.IdentifierName
        //
        // Technically this is ambiguous.  i.e. CallExpression defines:
        //
        //   CallExpression:
        //      CallExpression Arguments
        //
        // If you see: "new Foo()"
        //
        // Then that could be treated as a single ObjectCreationExpression, or it could be
        // treated as the invocation of "new Foo".  We disambiguate that in code (to match
        // the original grammar) by making sure that if we see an ObjectCreationExpression
        // we always consume arguments if they are there. So we treat "new Foo()" as an
        // object creation only, and not at all as an invocation.  Another way to think
        // about this is that for every "new" that we see, we will consume an argument list if
        // it is there as part of the *associated* object creation node.  Any additional
        // argument lists we see, will become invocation expressions.
        //
        // Because there are no other places in the grammar now that refer to FunctionExpression
        // or ObjectCreationExpression, it is safe to push down into the PrimaryExpression
        // production.
        //
        // Because CallExpression and MemberExpression are left recursive, we need to bottom out
        // of the recursion immediately.  So we parse out a primary expression to start with.
        let pos = self.node_pos();
        let expression = self.parse_primary_expression();
        self.parse_member_expression_rest(pos, expression, true /*allowOptionalChain*/)
    }

    // Go: parser.go:5401 parseMemberExpressionRest
    pub fn parse_member_expression_rest(
        &mut self,
        pos: i32,
        mut expression: Node,
        allow_optional_chain: bool,
    ) -> Node {
        loop {
            let mut question_dot_token = Node::NIL;
            let is_property_access;
            if allow_optional_chain && self.is_start_of_optional_property_or_element_access_chain()
            {
                question_dot_token = self.parse_expected_token(SyntaxKind::QuestionDotToken);
                is_property_access = token_is_identifier_or_keyword(self.token);
            } else {
                is_property_access = self.parse_optional(SyntaxKind::DotToken);
            }
            if is_property_access {
                expression =
                    self.parse_property_access_expression_rest(pos, expression, question_dot_token);
                continue;
            }
            // when in the [Decorator] context, we do not parse ElementAccess as it could be part of a ComputedPropertyName
            if (question_dot_token.is_some() || !self.in_decorator_context())
                && self.parse_optional(SyntaxKind::OpenBracketToken)
            {
                expression =
                    self.parse_element_access_expression_rest(pos, expression, question_dot_token);
                continue;
            }
            if self.is_template_start_of_tagged_template() {
                // Absorb type arguments into TemplateExpression when preceding expression is ExpressionWithTypeArguments
                if question_dot_token.is_nil() && is_expression_with_type_arguments(expression) {
                    let original_expression = expression.expression();
                    let original_type_arguments = expression.type_argument_list();
                    expression = self.parse_tagged_template_rest(
                        pos,
                        original_expression,
                        question_dot_token,
                        original_type_arguments,
                    );
                    self.unparse_expression_with_type_arguments(
                        original_expression,
                        original_type_arguments,
                        expression,
                    );
                } else {
                    expression = self.parse_tagged_template_rest(
                        pos,
                        expression,
                        question_dot_token,
                        NodeList::NIL, /*typeArguments*/
                    );
                }
                continue;
            }
            if question_dot_token.is_nil() {
                if self.token == SyntaxKind::ExclamationToken && !self.has_preceding_line_break() {
                    self.next_token();
                    let node = self
                        .factory
                        .new_non_null_expression(expression, NodeFlags::NONE);
                    let node = self.finish_node(node, pos);
                    expression = self.check_js_syntax(node);
                    continue;
                }
                let type_arguments = self.try_parse_type_arguments_in_expression();
                if type_arguments.is_some() {
                    let node = self
                        .factory
                        .new_expression_with_type_arguments(expression, type_arguments);
                    expression = self.finish_node(node, pos);
                    continue;
                }
            }
            return expression;
        }
    }

    // Go: parser.go:5447 isStartOfOptionalPropertyOrElementAccessChain
    pub fn is_start_of_optional_property_or_element_access_chain(&mut self) -> bool {
        self.token == SyntaxKind::QuestionDotToken
            && self
                .look_ahead(Parser::next_token_is_identifier_or_keyword_or_open_bracket_or_template)
    }

    // Go: parser.go:5451 nextTokenIsIdentifierOrKeywordOrOpenBracketOrTemplate
    pub fn next_token_is_identifier_or_keyword_or_open_bracket_or_template(&mut self) -> bool {
        self.next_token();
        token_is_identifier_or_keyword(self.token)
            || self.token == SyntaxKind::OpenBracketToken
            || self.is_template_start_of_tagged_template()
    }

    // Go: parser.go:5456 parsePropertyAccessExpressionRest
    pub fn parse_property_access_expression_rest(
        &mut self,
        pos: i32,
        expression: Node,
        question_dot_token: Node,
    ) -> Node {
        let name = self.parse_right_side_of_dot(
            true, /*allowIdentifierNames*/
            true, /*allowPrivateIdentifiers*/
            true, /*allowUnicodeEscapeSequenceInIdentifierName*/
        );
        let is_optional_chain =
            question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
        let property_access = self.factory.new_property_access_expression(
            expression,
            question_dot_token,
            name,
            if is_optional_chain {
                NodeFlags::OPTIONAL_CHAIN
            } else {
                NodeFlags::NONE
            },
        );
        if is_optional_chain && is_private_identifier(name) {
            let loc = self.skip_range_trivia(name.loc());
            self.parse_error_at_range(
                loc,
                diag::An_optional_chain_cannot_contain_private_identifiers,
                args![],
            );
        }
        if is_expression_with_type_arguments(expression) {
            let type_arguments = expression.type_argument_list();
            if type_arguments.is_some() {
                let loc = TextRange::new(
                    type_arguments.pos() - 1,
                    skip_trivia(self.source_text, type_arguments.end()) + 1,
                );
                self.parse_error_at_range(
                    loc,
                    diag::An_instantiation_expression_cannot_be_followed_by_a_property_access,
                    args![],
                );
            }
        }
        self.finish_node(property_access, pos)
    }

    // Go: parser.go:5473 tryReparseOptionalChain
    pub fn try_reparse_optional_chain(&mut self, mut node: Node) -> bool {
        if node.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
            return true;
        }
        // check for an optional chain in a non-null expression
        if is_non_null_expression(node) {
            let mut expr = node.expression();
            while is_non_null_expression(expr)
                && !expr.flags().intersects(NodeFlags::OPTIONAL_CHAIN)
            {
                expr = expr.expression();
            }
            if expr.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
                // this is part of an optional chain. Walk down from `node` to `expression` and set the flag.
                while is_non_null_expression(node) {
                    set_node_flags(node, node.flags() | NodeFlags::OPTIONAL_CHAIN);
                    node = node.expression();
                }
                return true;
            }
        }
        false
    }

    // Go: parser.go:5495 parseElementAccessExpressionRest
    pub fn parse_element_access_expression_rest(
        &mut self,
        pos: i32,
        expression: Node,
        question_dot_token: Node,
    ) -> Node {
        let mut argument_expression = self.create_missing_identifier();
        if self.token == SyntaxKind::CloseBracketToken {
            let node_pos = self.node_pos();
            self.parse_error_at(
                node_pos,
                node_pos,
                diag::An_element_access_expression_should_take_an_argument,
                args![],
            );
        } else {
            argument_expression = self.parse_expression_allow_in();
        }
        self.parse_expected(SyntaxKind::CloseBracketToken);
        let is_optional_chain =
            question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
        let node = self.factory.new_element_access_expression(
            expression,
            question_dot_token,
            argument_expression,
            if is_optional_chain {
                NodeFlags::OPTIONAL_CHAIN
            } else {
                NodeFlags::NONE
            },
        );
        self.finish_node(node, pos)
    }

    // Go: parser.go:5507 parseCallExpressionRest
    pub fn parse_call_expression_rest(&mut self, pos: i32, mut expression: Node) -> Node {
        loop {
            expression = self
                .parse_member_expression_rest(pos, expression, true /*allowOptionalChain*/);
            let mut type_arguments = NodeList::NIL;
            let question_dot_token = self.parse_optional_token(SyntaxKind::QuestionDotToken);
            if question_dot_token.is_some() {
                type_arguments = self.try_parse_type_arguments_in_expression();
                if self.is_template_start_of_tagged_template() {
                    expression = self.parse_tagged_template_rest(
                        pos,
                        expression,
                        question_dot_token,
                        type_arguments,
                    );
                    continue;
                }
            }
            if type_arguments.is_some() || self.token == SyntaxKind::OpenParenToken {
                // Absorb type arguments into CallExpression when preceding expression is ExpressionWithTypeArguments
                if question_dot_token.is_nil()
                    && expression.kind() == SyntaxKind::ExpressionWithTypeArguments
                {
                    type_arguments = expression.type_argument_list();
                    expression = expression.expression();
                }
                let inner = expression;
                let argument_list = self.parse_argument_list();
                let is_optional_chain =
                    question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
                let node = self.factory.new_call_expression(
                    expression,
                    question_dot_token,
                    type_arguments,
                    argument_list,
                    if is_optional_chain {
                        NodeFlags::OPTIONAL_CHAIN
                    } else {
                        NodeFlags::NONE
                    },
                );
                let node = self.finish_node(node, pos);
                expression = self.check_js_syntax(node);
                self.unparse_expression_with_type_arguments(inner, type_arguments, expression);
                continue;
            }
            if question_dot_token.is_some() {
                // We parsed `?.` but then failed to parse anything, so report a missing identifier here.
                self.parse_error_at_current_token(diag::Identifier_expected, args![]);
                let name = self.create_missing_identifier();
                let node = self.factory.new_property_access_expression(
                    expression,
                    question_dot_token,
                    name,
                    NodeFlags::OPTIONAL_CHAIN,
                );
                expression = self.finish_node(node, pos);
            }
            break;
        }
        expression
    }

    // Go: parser.go:5543 parseArgumentList
    pub fn parse_argument_list(&mut self) -> NodeList {
        self.parse_expected(SyntaxKind::OpenParenToken);
        let result = self.parse_delimited_list(
            ParsingContext::ArgumentExpressions,
            Parser::parse_argument_expression,
        );
        self.parse_expected(SyntaxKind::CloseParenToken);
        result
    }

    // Go: parser.go:5550 parseArgumentExpression
    pub fn parse_argument_expression(&mut self) -> Node {
        self.do_in_context(
            NodeFlags::DISALLOW_IN_CONTEXT | NodeFlags::DECORATOR_CONTEXT,
            false,
            Parser::parse_argument_or_array_literal_element,
        )
    }

    // Go: parser.go:5554 parseArgumentOrArrayLiteralElement
    pub fn parse_argument_or_array_literal_element(&mut self) -> Node {
        match self.token {
            SyntaxKind::DotDotDotToken => self.parse_spread_element(),
            SyntaxKind::CommaToken => {
                let node = self.factory.new_omitted_expression();
                let pos = self.node_pos();
                self.finish_node(node, pos)
            }
            _ => self.parse_assignment_expression_or_higher(),
        }
    }

    // Go: parser.go:5564 parseSpreadElement
    pub fn parse_spread_element(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::DotDotDotToken);
        let expression = self.parse_assignment_expression_or_higher();
        let node = self.factory.new_spread_element(expression);
        self.finish_node(node, pos)
    }

    // Go: parser.go:5571 parseTaggedTemplateRest
    pub fn parse_tagged_template_rest(
        &mut self,
        pos: i32,
        tag: Node,
        question_dot_token: Node,
        type_arguments: NodeList,
    ) -> Node {
        let template;
        if self.token == SyntaxKind::NoSubstitutionTemplateLiteral {
            self.re_scan_template_token(true /*isTaggedTemplate*/);
            template = self.parse_literal_expression();
        } else {
            template = self.parse_template_expression(true /*isTaggedTemplate*/);
        }
        let is_optional_chain =
            question_dot_token.is_some() || tag.flags().intersects(NodeFlags::OPTIONAL_CHAIN);
        let node = self.factory.new_tagged_template_expression(
            tag,
            question_dot_token,
            type_arguments,
            template,
            if is_optional_chain {
                NodeFlags::OPTIONAL_CHAIN
            } else {
                NodeFlags::NONE
            },
        );
        let node = self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
}

// Go: parser.go:4503 typeHasArrowFunctionBlockingParseError
// If true, we should abort parsing an error function.
pub fn type_has_arrow_function_blocking_parse_error(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::TypeReference => node_is_missing(node.type_name()),
        SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
            is_missing_node_list(node.parameter_list())
                || type_has_arrow_function_blocking_parse_error(node.type_())
        }
        SyntaxKind::ParenthesizedType => type_has_arrow_function_blocking_parse_error(node.type_()),
        _ => false,
    }
}

// Go: parser.go:4635 shouldConsumeBinaryOperator
// shouldConsumeBinaryOperator reports whether an operator binds before the operator represented by currentPrecedence.
// At equal precedence, only the right-associative exponentiation operator binds first.
#[must_use]
pub fn should_consume_binary_operator(
    operator: SyntaxKind,
    operator_precedence: OperatorPrecedence,
    current_precedence: OperatorPrecedence,
) -> bool {
    if operator_precedence > current_precedence {
        return true;
    }
    operator_precedence == current_precedence && operator == SyntaxKind::AsteriskAsteriskToken
}
