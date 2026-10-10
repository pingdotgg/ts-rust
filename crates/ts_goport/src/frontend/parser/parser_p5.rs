//! Port of Go `parser/parser.go` lines 5534 to 6822: templates, primary
//! expressions, literals, identifiers, node finishing, lookahead helpers,
//! context flags, pragmas and the JavaScript syntax checks.
//!
//! Unit U8 of the frontend plan (`/tmp/port/frontend/plan.md`). The `Parser`
//! struct and its fields are in `parser_p1.rs` (U4).

use crate::frontend::prelude::*;

impl<'a> Parser<'a> {
    // Go: parser/parser.go:5583 parseTemplateExpression
    pub fn parse_template_expression(&mut self, is_tagged_template: bool) -> Node {
        let pos = self.node_pos();
        let head = self.parse_template_head(is_tagged_template);
        let spans = self.parse_template_spans(is_tagged_template);
        let node = self.factory.new_template_expression(head, spans);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:5588 parseTemplateSpans
    pub fn parse_template_spans(&mut self, is_tagged_template: bool) -> NodeList {
        let pos = self.node_pos();
        let mut list: Vec<Node> = Vec::new();
        loop {
            let span = self.parse_template_span(is_tagged_template);
            list.push(span);
            if span.literal().kind() != SyntaxKind::TemplateMiddle {
                break;
            }
        }
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &list)
    }

    // Go: parser/parser.go:5601 parseTemplateSpan
    pub fn parse_template_span(&mut self, is_tagged_template: bool) -> Node {
        let pos = self.node_pos();
        let expression = self.parse_expression_allow_in();
        let literal = self.parse_literal_of_template_span(is_tagged_template);
        let node = self.factory.new_template_span(expression, literal);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:5608 parsePrimaryExpression
    // PORT: Go `fallthrough` from NoSubstitutionTemplateLiteral into the
    // literal case is one arm with a kind check. Go `break` out of the switch
    // leaves the match and reaches the final identifier parse.
    pub fn parse_primary_expression(&mut self) -> Node {
        match self.token {
            SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral => {
                if self.token == SyntaxKind::NoSubstitutionTemplateLiteral
                    && self
                        .scanner
                        .token_flags()
                        .intersects(TokenFlags::IS_INVALID)
                {
                    self.re_scan_template_token(false /*isTaggedTemplate*/);
                }
                return self.parse_literal_expression();
            }
            SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword => {
                return self.parse_keyword_expression();
            }
            SyntaxKind::OpenParenToken => {
                return self.parse_parenthesized_expression();
            }
            SyntaxKind::OpenBracketToken => {
                return self.parse_array_literal_expression();
            }
            SyntaxKind::OpenBraceToken => {
                return self.parse_object_literal_expression();
            }
            SyntaxKind::AsyncKeyword => {
                // Async arrow functions are parsed earlier in parseAssignmentExpressionOrHigher.
                // If we encounter `async [no LineTerminator here] function` then this is an async
                // function; otherwise, its an identifier.
                if self.look_ahead(Parser::next_token_is_function_keyword_on_same_line) {
                    return self.parse_function_expression();
                }
            }
            SyntaxKind::AtToken => {
                return self.parse_decorated_expression();
            }
            SyntaxKind::ClassKeyword => {
                return self.parse_class_expression();
            }
            SyntaxKind::FunctionKeyword => {
                return self.parse_function_expression();
            }
            SyntaxKind::NewKeyword => {
                return self.parse_new_expression_or_new_dot_target();
            }
            SyntaxKind::SlashToken | SyntaxKind::SlashEqualsToken => {
                if self.re_scan_slash_token() == SyntaxKind::RegularExpressionLiteral {
                    return self.parse_literal_expression();
                }
            }
            SyntaxKind::TemplateHead => {
                return self.parse_template_expression(false /*isTaggedTemplate*/);
            }
            SyntaxKind::PrivateIdentifier => {
                return self.parse_private_identifier();
            }
            _ => {}
        }
        self.parse_identifier_with_diagnostic(Some(diag::Expression_expected), None)
    }

    // Go: parser/parser.go:5653 parseParenthesizedExpression
    pub fn parse_parenthesized_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(SyntaxKind::CloseParenToken);
        let node = self.factory.new_parenthesized_expression(expression);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:5664 parseArrayLiteralExpression
    pub fn parse_array_literal_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let open_bracket_position = self.scanner.token_start();
        let open_bracket_parsed = self.parse_expected(SyntaxKind::OpenBracketToken);
        let multi_line = self.has_preceding_line_break();
        let elements = self.parse_delimited_list(
            ParsingContext::ArrayLiteralMembers,
            Parser::parse_argument_or_array_literal_element,
        );
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenBracketToken,
            SyntaxKind::CloseBracketToken,
            open_bracket_parsed,
            open_bracket_position,
        );
        let node = self
            .factory
            .new_array_literal_expression(elements, multi_line);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:5674 parseObjectLiteralExpression
    pub fn parse_object_literal_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let open_brace_position = self.scanner.token_start();
        let open_brace_parsed = self.parse_expected(SyntaxKind::OpenBraceToken);
        let multi_line = self.has_preceding_line_break();
        let properties = self.parse_delimited_list(
            ParsingContext::ObjectLiteralMembers,
            Parser::parse_object_literal_element,
        );
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            open_brace_parsed,
            open_brace_position,
        );
        let node = self
            .factory
            .new_object_literal_expression(properties, multi_line);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:5684 parseObjectLiteralElement
    pub fn parse_object_literal_element(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.parse_optional(SyntaxKind::DotDotDotToken) {
            let expression = self.parse_assignment_expression_or_higher();
            let node = self.factory.new_spread_assignment(expression);
            let result = self.finish_node(node, pos);
            self.with_js_doc(result, jsdoc);
            return result;
        }
        let modifiers = self.parse_modifiers_ex(
            true,  /*allowDecorators*/
            false, /*permitConstAsModifier*/
            false, /*stopOnStartOfClassStaticBlock*/
        );
        if self.parse_contextual_modifier(SyntaxKind::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::GetAccessor,
                ParseFlags::NONE,
            );
        }
        if self.parse_contextual_modifier(SyntaxKind::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::SetAccessor,
                ParseFlags::NONE,
            );
        }
        let asterisk_token = self.parse_optional_token(SyntaxKind::AsteriskToken);
        let token_is_identifier = self.is_identifier();
        let name = self.parse_property_name();
        // Disallowing of optional property assignments and definite assignment assertion happens in the grammar checker.
        let mut postfix_token = self.parse_optional_token(SyntaxKind::QuestionToken);
        // Decorators, Modifiers, questionToken, and exclamationToken are not supported by property assignments and are reported in the grammar checker
        if postfix_token.is_nil() {
            postfix_token = self.parse_optional_token(SyntaxKind::ExclamationToken);
        }
        if asterisk_token.is_some()
            || self.token == SyntaxKind::OpenParenToken
            || self.token == SyntaxKind::LessThanToken
        {
            return self.parse_method_declaration(
                pos,
                jsdoc,
                modifiers,
                asterisk_token,
                name,
                postfix_token,
                None, /*diagnosticMessage*/
            );
        }
        // check if it is short-hand property assignment or normal property assignment
        // NOTE: if token is EqualsToken it is interpreted as CoverInitializedName production
        // CoverInitializedName[Yield] :
        //     IdentifierReference[?Yield] Initializer[In, ?Yield]
        // this is necessary because ObjectLiteral productions are also used to cover grammar for ObjectAssignmentPattern
        let node;
        let is_shorthand_property_assignment =
            token_is_identifier && self.token != SyntaxKind::ColonToken;
        if is_shorthand_property_assignment {
            let equals_token = self.parse_optional_token(SyntaxKind::EqualsToken);
            let mut initializer = Node::NIL;
            if equals_token.is_some() {
                initializer = self.do_in_context(
                    NodeFlags::DISALLOW_IN_CONTEXT,
                    false,
                    Parser::parse_assignment_expression_or_higher,
                );
            }
            node = self.factory.new_shorthand_property_assignment(
                modifiers,
                name,
                postfix_token,
                Node::NIL, /*typeNode*/
                equals_token,
                initializer,
            );
        } else {
            self.parse_expected(SyntaxKind::ColonToken);
            let initializer = self.do_in_context(
                NodeFlags::DISALLOW_IN_CONTEXT,
                false,
                Parser::parse_assignment_expression_or_higher,
            );
            node = self.factory.new_property_assignment(
                modifiers,
                name,
                postfix_token,
                Node::NIL, /*typeNode*/
                initializer,
            );
        }
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }

    // Go: parser/parser.go:5736 parseFunctionExpression
    pub fn parse_function_expression(&mut self) -> Node {
        // GeneratorExpression:
        //      function* BindingIdentifier [Yield][opt](FormalParameters[Yield]){ GeneratorBody }
        //
        // FunctionExpression:
        //      function BindingIdentifier[opt](FormalParameters){ FunctionBody }
        let save_contex_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DECORATOR_CONTEXT, false);
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        self.parse_expected(SyntaxKind::FunctionKeyword);
        let asterisk_token = self.parse_optional_token(SyntaxKind::AsteriskToken);
        let is_generator = asterisk_token.is_some();
        let is_async = modifier_list_has_async(modifiers);
        let signature_flags = (if is_generator {
            ParseFlags::YIELD
        } else {
            ParseFlags::NONE
        }) | (if is_async {
            ParseFlags::AWAIT
        } else {
            ParseFlags::NONE
        });
        let name = if is_generator && is_async {
            self.do_in_context(
                NodeFlags::YIELD_CONTEXT | NodeFlags::AWAIT_CONTEXT,
                true,
                Parser::parse_optional_binding_identifier,
            )
        } else if is_generator {
            self.do_in_context(
                NodeFlags::YIELD_CONTEXT,
                true,
                Parser::parse_optional_binding_identifier,
            )
        } else if is_async {
            self.do_in_context(
                NodeFlags::AWAIT_CONTEXT,
                true,
                Parser::parse_optional_binding_identifier,
            )
        } else {
            self.parse_optional_binding_identifier()
        };
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(signature_flags);
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
        let body = self.parse_function_block(signature_flags, None /*diagnosticMessage*/);
        self.context_flags = save_contex_flags;
        let result = self.factory.new_function_expression(
            modifiers,
            asterisk_token,
            name,
            type_parameters,
            parameters,
            return_type,
            Node::NIL, /*fullSignature*/
            body,
        );
        self.finish_node(result, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:5775 parseOptionalBindingIdentifier
    pub fn parse_optional_binding_identifier(&mut self) -> Node {
        if self.is_binding_identifier() {
            return self.parse_binding_identifier();
        }
        Node::NIL
    }

    // Go: parser/parser.go:5782 parseDecoratedExpression
    pub fn parse_decorated_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(
            true,  /*allowDecorators*/
            false, /*permitConstAsModifier*/
            false, /*stopOnStartOfClassStaticBlock*/
        );
        if self.token == SyntaxKind::ClassKeyword {
            return self.parse_class_declaration_or_expression(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::ClassExpression,
            );
        }
        let error_pos = self.node_pos();
        self.parse_error_at(error_pos, error_pos, diag::Expression_expected, args![]);
        let node = self.factory.new_missing_declaration(modifiers);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:5793 unparseExpressionWithTypeArguments
    pub fn unparse_expression_with_type_arguments(
        &mut self,
        expression: Node,
        type_arguments: NodeList,
        result: Node,
    ) {
        // force overwrite the `.Parent` of the expression and type arguments to erase the fact that they may have originally been parsed as an ExpressionWithTypeArguments and be parented to such
        if expression.is_some() {
            set_node_parent(expression, result);
        }
        if !type_arguments.is_nil() {
            for a in type_arguments.nodes().iter() {
                set_node_parent(a, result);
            }
        }
    }

    // Go: parser/parser.go:5805 parseNewExpressionOrNewDotTarget
    pub fn parse_new_expression_or_new_dot_target(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::NewKeyword);
        if self.parse_optional(SyntaxKind::DotToken) {
            let name = self.parse_identifier_name();
            let node = self.factory.new_meta_property(SyntaxKind::NewKeyword, name);
            return self.finish_node(node, pos);
        }
        let expression_pos = self.node_pos();
        let primary = self.parse_primary_expression();
        let mut expression = self.parse_member_expression_rest(
            expression_pos,
            primary,
            false, /*allowOptionalChain*/
        );
        let mut type_arguments = NodeList::NIL;
        // Absorb type arguments into NewExpression when preceding expression is ExpressionWithTypeArguments
        if expression.kind() == SyntaxKind::ExpressionWithTypeArguments {
            type_arguments = expression.type_argument_list();
            expression = expression.expression();
        }
        if self.token == SyntaxKind::QuestionDotToken {
            let text = get_text_of_node_from_source_text(
                self.source_text,
                expression,
                false, /*includeTrivia*/
            );
            self.parse_error_at_current_token(
                diag::Invalid_optional_chain_from_new_expression_Did_you_mean_to_call_0,
                args![text],
            );
        }
        let mut argument_list = NodeList::NIL;
        if self.token == SyntaxKind::OpenParenToken {
            argument_list = self.parse_argument_list();
        }
        let node = self
            .factory
            .new_new_expression(expression, type_arguments, argument_list);
        let finished = self.finish_node(node, pos);
        let result = self.check_js_syntax(finished);
        self.unparse_expression_with_type_arguments(expression, type_arguments, result);
        result
    }

    // Go: parser/parser.go:5832 parseKeywordExpression
    pub fn parse_keyword_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let result = self.factory.new_keyword_expression(self.token);
        self.next_token();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:5839 parseLiteralExpression
    pub fn parse_literal_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let text = self.scanner.token_value();
        let token_flags = self.scanner.token_flags();
        let result = match self.token {
            SyntaxKind::StringLiteral => self.factory.new_string_literal(text, token_flags),
            SyntaxKind::NumericLiteral => self.factory.new_numeric_literal(text, token_flags),
            SyntaxKind::BigIntLiteral => self.factory.new_big_int_literal(text, token_flags),
            SyntaxKind::RegularExpressionLiteral => self
                .factory
                .new_regular_expression_literal(text, token_flags),
            SyntaxKind::NoSubstitutionTemplateLiteral => self
                .factory
                .new_no_substitution_template_literal(text, token_flags),
            _ => panic!("Unhandled case in parseLiteralExpression"),
        };
        self.next_token();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:5862 parseIdentifierNameErrorOnUnicodeEscapeSequence
    pub fn parse_identifier_name_error_on_unicode_escape_sequence(&mut self) -> Node {
        if self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape() {
            self.parse_error_at_current_token(
                diag::Unicode_escape_sequence_cannot_appear_here,
                args![],
            );
        }
        self.create_identifier(token_is_identifier_or_keyword(self.token))
    }

    // Go: parser/parser.go:5869 parseBindingIdentifier
    pub fn parse_binding_identifier(&mut self) -> Node {
        self.parse_binding_identifier_with_diagnostic(None)
    }

    // Go: parser/parser.go:5873 parseBindingIdentifierWithDiagnostic
    pub fn parse_binding_identifier_with_diagnostic(
        &mut self,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> Node {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let is_binding_identifier = self.is_binding_identifier();
        let id = self.create_identifier_with_diagnostic(
            is_binding_identifier,
            None, /*diagnosticMessage*/
            private_identifier_diagnostic_message,
        );
        self.statement_has_await_identifier = save_has_await_identifier;
        id
    }

    // Go: parser/parser.go:5880 parseIdentifierName
    pub fn parse_identifier_name(&mut self) -> Node {
        self.parse_identifier_name_with_diagnostic(None)
    }

    // Go: parser/parser.go:5884 parseIdentifierNameWithDiagnostic
    pub fn parse_identifier_name_with_diagnostic(
        &mut self,
        diagnostic_message: Option<&'static Message>,
    ) -> Node {
        self.create_identifier_with_diagnostic(
            token_is_identifier_or_keyword(self.token),
            diagnostic_message,
            None,
        )
    }

    // Go: parser/parser.go:5888 parseIdentifier
    pub fn parse_identifier(&mut self) -> Node {
        self.parse_identifier_with_diagnostic(None, None)
    }

    // Go: parser/parser.go:5892 parseIdentifierWithDiagnostic
    pub fn parse_identifier_with_diagnostic(
        &mut self,
        diagnostic_message: Option<&'static Message>,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> Node {
        let is_identifier = self.is_identifier();
        self.create_identifier_with_diagnostic(
            is_identifier,
            diagnostic_message,
            private_identifier_diagnostic_message,
        )
    }

    // Go: parser/parser.go:5896 createIdentifier
    pub fn create_identifier(&mut self, is_identifier: bool) -> Node {
        self.create_identifier_with_diagnostic(is_identifier, None, None)
    }

    // Go: parser/parser.go:5900 createIdentifierWithDiagnostic
    pub fn create_identifier_with_diagnostic(
        &mut self,
        is_identifier: bool,
        diagnostic_message: Option<&'static Message>,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> Node {
        if is_identifier {
            let pos = if self.scanner.has_preceding_js_doc_leading_asterisks() {
                self.scanner.token_start()
            } else {
                self.node_pos()
            };
            let text = self.scanner.token_value();
            self.next_token_without_check();
            let node = self.new_identifier(text);
            return self.finish_node(node, pos);
        }
        if self.token == SyntaxKind::PrivateIdentifier {
            if let Some(message) = private_identifier_diagnostic_message {
                self.parse_error_at_current_token(message, args![]);
            } else {
                self.parse_error_at_current_token(
                    diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                    args![],
                );
            }
            return self.create_identifier(true /*isIdentifier*/);
        }
        // Only for end of file because the error gets reported incorrectly on embedded script tags.
        let report_at_current_position = self.token == SyntaxKind::EndOfFile;
        if let Some(message) = diagnostic_message {
            if report_at_current_position {
                let pos = self.scanner.token_full_start();
                self.parse_error_at(pos, pos, message, args![]);
            } else {
                self.parse_error_at_current_token(message, args![]);
            }
        } else if is_reserved_word(self.token) {
            let token_text = self.scanner.token_text().to_string();
            if report_at_current_position {
                let pos = self.scanner.token_full_start();
                self.parse_error_at(
                    pos,
                    pos,
                    diag::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here,
                    args![token_text],
                );
            } else {
                self.parse_error_at_current_token(
                    diag::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here,
                    args![token_text],
                );
            }
        } else if report_at_current_position {
            let pos = self.scanner.token_full_start();
            self.parse_error_at(pos, pos, diag::Identifier_expected, args![]);
        } else {
            self.parse_error_at_current_token(diag::Identifier_expected, args![]);
        }
        self.create_missing_identifier()
    }

    // Go: parser/parser.go:5947 newNodeList
    // PORT: Go sets `list.Loc` after `NewNodeList`. Rust lists are immutable
    // once created, so the factory takes the range at creation.
    pub fn new_node_list(&mut self, loc: TextRange, nodes: &[Node]) -> NodeList {
        let loc = self.jsdoc_tail_range(loc);
        self.factory.new_node_list_with_loc(nodes, loc)
    }

    // Go: parser/parser.go:5953 newModifierList
    // PORT: see new_node_list.
    pub fn new_modifier_list(&mut self, loc: TextRange, nodes: &[Node]) -> ModifierList {
        let loc = self.jsdoc_tail_range(loc);
        self.factory.new_modifier_list_with_loc(nodes, loc)
    }

    /// `loc` with the positions in the cut tail of a JSDoc text mapped to
    /// the file (see `parse_js_doc_comment`).
    #[inline]
    fn jsdoc_tail_range(&self, loc: TextRange) -> TextRange {
        if loc.end() >= self.jsdoc_tail_first {
            TextRange::new(
                self.jsdoc_tail_pos(loc.pos()),
                self.jsdoc_tail_pos(loc.end()),
            )
        } else {
            loc
        }
    }

    // Go: parser/parser.go:5959 finishNode
    pub fn finish_node(&mut self, node: Node, pos: i32) -> Node {
        let end = self.node_pos();
        self.finish_node_with_end(node, pos, end)
    }

    // Go: parser/parser.go:5963 finishNodeWithEnd
    pub fn finish_node_with_end(&mut self, node: Node, pos: i32, end: i32) -> Node {
        let loc = self.jsdoc_tail_range(TextRange::new(pos, end));
        let mut flags = self.context_flags;
        if self.has_parse_error() {
            flags |= NodeFlags::THIS_NODE_HAS_ERROR;
            self.set_has_parse_error(false);
        }
        // PORT: one store write for the Go `Loc` and `Flags` writes. A node
        // of the parse store has no binder flags yet, so `Flags |= flags`
        // equals Go `node.Flags = node.Flags | flags`.
        if !finish_store_node(node, loc, flags) {
            set_node_loc(node, loc);
            set_node_flags(node, node.flags() | flags);
        }
        self.override_parent_in_immediate_children(node);
        node
    }

    // Go: parser/parser.go:5974 overrideParentInImmediateChildren
    // PORT: Go calls the `p.setParentFromContext` closure field (set in
    // initializeClosures: `n.Parent = p.currentParent; return false`). The
    // closure body is in the two walks below, because the visitor cannot
    // borrow the parser while `for_each_child` runs.
    // PERF: R2-5. The same visit writes the binder child links of `node`
    // (`StoreChildLinks`), while its data is hot, so the binder can walk
    // the children without loading the node data (`bind_each_child`).
    // PERF: R3-2. A node of the parse store whose children are all nodes of
    // that store (nearly every node) takes one store borrow for the whole
    // visit (`set_parent_in_store_children`). Any other node takes the
    // generic walk. Debug builds run the generic walk after the fast one
    // and check that it writes the same parents and links.
    pub fn override_parent_in_immediate_children(&mut self, node: Node) {
        self.current_parent = node;
        let parent = self.current_parent;
        if set_parent_in_store_children(parent) {
            #[cfg(debug_assertions)]
            {
                let fast = debug_store_child_link_state(parent);
                set_parent_in_children_generic(node, parent);
                debug_assert_eq!(
                    fast,
                    debug_store_child_link_state(parent),
                    "R3-2: the store walk differs from the generic walk"
                );
            }
        } else {
            set_parent_in_children_generic(node, parent);
        }
        self.current_parent = Node::NIL;
    }

    // Go: parser/parser.go:5980 nextTokenIsSlash
    pub fn next_token_is_slash(&mut self) -> bool {
        self.next_token() == SyntaxKind::SlashToken
    }

    // Go: parser/parser.go:5984 scanTypeMemberStart
    pub fn scan_type_member_start(&mut self) -> bool {
        // Return true if we have the start of a signature member
        if self.token == SyntaxKind::OpenParenToken
            || self.token == SyntaxKind::LessThanToken
            || self.token == SyntaxKind::GetKeyword
            || self.token == SyntaxKind::SetKeyword
        {
            return true;
        }
        let mut id_token = false;
        // Eat up all modifiers, but hold on to the last one in case it is actually an identifier
        while is_modifier_kind(self.token) {
            id_token = true;
            self.next_token();
        }
        // Index signatures and computed property names are type members
        if self.token == SyntaxKind::OpenBracketToken {
            return true;
        }
        // Try to get the first property-like token following all modifiers
        if self.is_literal_property_name() {
            id_token = true;
            self.next_token();
        }
        // If we were able to get any potential identifier, check that it is
        // the start of a member declaration
        if id_token {
            return self.token == SyntaxKind::OpenParenToken
                || self.token == SyntaxKind::LessThanToken
                || self.token == SyntaxKind::QuestionToken
                || self.token == SyntaxKind::ColonToken
                || self.token == SyntaxKind::CommaToken
                || self.can_parse_semicolon();
        }
        false
    }

    // Go: parser/parser.go:6012 scanClassMemberStart
    pub fn scan_class_member_start(&mut self) -> bool {
        let mut id_token = SyntaxKind::Unknown;
        if self.token == SyntaxKind::AtToken {
            return true;
        }
        // Eat up all modifiers, but hold on to the last one in case it is actually an identifier.
        while is_modifier_kind(self.token) {
            id_token = self.token;
            // If the idToken is a class modifier (protected, private, public, and static), it is
            // certain that we are starting to parse class member. This allows better error recovery
            // Example:
            //      public foo() ...     // true
            //      public @dec blah ... // true; we will then report an error later
            //      export public ...    // true; we will then report an error later
            if is_class_member_modifier(id_token) {
                return true;
            }
            self.next_token();
        }
        if self.token == SyntaxKind::AsteriskToken {
            return true;
        }
        // Try to get the first property-like token following all modifiers.
        // This can either be an identifier or the 'get' or 'set' keywords.
        if self.is_literal_property_name() {
            id_token = self.token;
            self.next_token();
        }
        // Index signatures and computed properties are class members; we can parse.
        if self.token == SyntaxKind::OpenBracketToken {
            return true;
        }
        // If we were able to get any potential identifier...
        if id_token != SyntaxKind::Unknown {
            // If we have a non-keyword identifier, or if we have an accessor, then it's safe to parse.
            if !is_keyword(id_token)
                || id_token == SyntaxKind::SetKeyword
                || id_token == SyntaxKind::GetKeyword
            {
                return true;
            }
            // If it *is* a keyword, but not an accessor, check a little farther along
            // to see if it should actually be parsed as a class member.
            match self.token {
                SyntaxKind::OpenParenToken // Method declaration
                | SyntaxKind::LessThanToken // Generic Method declaration
                | SyntaxKind::ExclamationToken // Non-null assertion on property name
                | SyntaxKind::ColonToken // Type Annotation for declaration
                | SyntaxKind::EqualsToken // Initializer for declaration
                | SyntaxKind::QuestionToken => {
                    // Not valid, but permitted so that it gets caught later on.
                    return true;
                }
                _ => {}
            }
            // Covers
            //  - Semicolons     (declaration termination)
            //  - Closing braces (end-of-class, must be declaration)
            //  - End-of-files   (not valid, but permitted so that it gets caught later on)
            //  - Line-breaks    (enabling *automatic semicolon insertion*)
            return self.can_parse_semicolon();
        }
        false
    }

    // Go: parser/parser.go:6071 canParseSemicolon
    pub fn can_parse_semicolon(&self) -> bool {
        // If there's a real semicolon, then we can always parse it out.
        // We can parse out an optional semicolon in ASI cases in the following cases.
        self.token == SyntaxKind::SemicolonToken
            || self.token == SyntaxKind::CloseBraceToken
            || self.token == SyntaxKind::EndOfFile
            || self.has_preceding_line_break()
    }

    // Go: parser/parser.go:6077 tryParseSemicolon
    pub fn try_parse_semicolon(&mut self) -> bool {
        if !self.can_parse_semicolon() {
            return false;
        }
        if self.token == SyntaxKind::SemicolonToken {
            // consume the semicolon if it was explicitly provided.
            self.next_token();
        }
        true
    }

    // Go: parser/parser.go:6088 parseSemicolon
    pub fn parse_semicolon(&mut self) -> bool {
        self.try_parse_semicolon() || self.parse_expected(SyntaxKind::SemicolonToken)
    }

    // Go: parser/parser.go:6092 isLiteralPropertyName
    pub fn is_literal_property_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token)
            || self.token == SyntaxKind::StringLiteral
            || self.token == SyntaxKind::NumericLiteral
            || self.token == SyntaxKind::BigIntLiteral
    }

    // Go: parser/parser.go:6127 isStartOfStatement
    pub fn is_start_of_statement(&mut self) -> bool {
        match self.token {
            // 'catch' and 'finally' do not actually indicate that the code is part of a statement,
            // however, we say they are here so that we may gracefully parse them and error later.
            SyntaxKind::AtToken
            | SyntaxKind::SemicolonToken
            | SyntaxKind::OpenBraceToken
            | SyntaxKind::VarKeyword
            | SyntaxKind::LetKeyword
            | SyntaxKind::UsingKeyword
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::ClassKeyword
            | SyntaxKind::EnumKeyword
            | SyntaxKind::IfKeyword
            | SyntaxKind::DoKeyword
            | SyntaxKind::WhileKeyword
            | SyntaxKind::ForKeyword
            | SyntaxKind::ContinueKeyword
            | SyntaxKind::BreakKeyword
            | SyntaxKind::ReturnKeyword
            | SyntaxKind::WithKeyword
            | SyntaxKind::SwitchKeyword
            | SyntaxKind::ThrowKeyword
            | SyntaxKind::TryKeyword
            | SyntaxKind::DebuggerKeyword
            | SyntaxKind::CatchKeyword
            | SyntaxKind::FinallyKeyword => true,
            SyntaxKind::ImportKeyword => {
                self.is_start_of_declaration()
                    || self.is_next_token_open_paren_or_less_than_or_dot()
            }
            SyntaxKind::ConstKeyword | SyntaxKind::ExportKeyword => self.is_start_of_declaration(),
            SyntaxKind::AsyncKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::InterfaceKeyword
            | SyntaxKind::ModuleKeyword
            | SyntaxKind::NamespaceKeyword
            | SyntaxKind::TypeKeyword
            | SyntaxKind::GlobalKeyword
            | SyntaxKind::DeferKeyword
            | SyntaxKind::SourceKeyword => {
                // When these don't start a declaration, they're an identifier in an expression statement
                true
            }
            SyntaxKind::AccessorKeyword
            | SyntaxKind::PublicKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::StaticKeyword
            | SyntaxKind::ReadonlyKeyword => {
                // When these don't start a declaration, they may be the start of a class member if an identifier
                // immediately follows. Otherwise they're an identifier in an expression statement.
                self.is_start_of_declaration()
                    || !self.look_ahead(Parser::next_token_is_identifier_or_keyword_on_same_line)
            }
            _ => self.is_start_of_expression(),
        }
    }

    // Go: parser/parser.go:6125 isStartOfDeclaration
    pub fn is_start_of_declaration(&mut self) -> bool {
        self.look_ahead(Parser::scan_start_of_declaration)
    }

    // Go: parser/parser.go:6160 scanStartOfDeclaration
    pub fn scan_start_of_declaration(&mut self) -> bool {
        loop {
            match self.token {
                SyntaxKind::VarKeyword
                | SyntaxKind::LetKeyword
                | SyntaxKind::ConstKeyword
                | SyntaxKind::FunctionKeyword
                | SyntaxKind::ClassKeyword
                | SyntaxKind::EnumKeyword => return true,
                SyntaxKind::UsingKeyword => return self.is_using_declaration(),
                SyntaxKind::AwaitKeyword => return self.is_await_using_declaration(),
                // 'declare', 'module', 'namespace', 'interface'* and 'type' are all legal JavaScript identifiers;
                // however, an identifier cannot be followed by another identifier on the same line. This is what we
                // count on to parse out the respective declarations. For instance, we exploit this to say that
                //
                //    namespace n
                //
                // can be none other than the beginning of a namespace declaration, but need to respect that JavaScript sees
                //
                //    namespace
                //    n
                //
                // as the identifier 'namespace' on one line followed by the identifier 'n' on another.
                // We need to look one token ahead to see if it permissible to try parsing a declaration.
                //
                // *Note*: 'interface' is actually a strict mode reserved word. So while
                //
                //   "use strict"
                //   interface
                //   I {}
                //
                // could be legal, it would add complexity for very little gain.
                SyntaxKind::InterfaceKeyword
                | SyntaxKind::TypeKeyword
                | SyntaxKind::DeferKeyword
                | SyntaxKind::SourceKeyword => {
                    return self.next_token_is_identifier_on_same_line();
                }
                SyntaxKind::ModuleKeyword | SyntaxKind::NamespaceKeyword => {
                    return self.next_token_is_identifier_or_string_literal_on_same_line();
                }
                SyntaxKind::AbstractKeyword
                | SyntaxKind::AccessorKeyword
                | SyntaxKind::AsyncKeyword
                | SyntaxKind::DeclareKeyword
                | SyntaxKind::PrivateKeyword
                | SyntaxKind::ProtectedKeyword
                | SyntaxKind::PublicKeyword
                | SyntaxKind::ReadonlyKeyword => {
                    let previous_token = self.token;
                    self.next_token();
                    // ASI takes effect for this modifier.
                    if self.has_preceding_line_break() {
                        return false;
                    }
                    if previous_token == SyntaxKind::DeclareKeyword
                        && self.token == SyntaxKind::TypeKeyword
                    {
                        // If we see 'declare type', then commit to parsing a type alias. parseTypeAliasDeclaration will
                        // report Line_break_not_permitted_here if needed.
                        return true;
                    }
                    continue;
                }
                SyntaxKind::GlobalKeyword => {
                    self.next_token();
                    return self.token == SyntaxKind::OpenBraceToken
                        || self.token == SyntaxKind::Identifier
                        || self.token == SyntaxKind::ExportKeyword;
                }
                SyntaxKind::ImportKeyword => {
                    self.next_token();
                    return self.token == SyntaxKind::DeferKeyword
                        || self.token == SyntaxKind::SourceKeyword
                        || self.token == SyntaxKind::StringLiteral
                        || self.token == SyntaxKind::AsteriskToken
                        || self.token == SyntaxKind::OpenBraceToken
                        || token_is_identifier_or_keyword(self.token);
                }
                SyntaxKind::ExportKeyword => {
                    self.next_token();
                    if self.token == SyntaxKind::EqualsToken
                        || self.token == SyntaxKind::AsteriskToken
                        || self.token == SyntaxKind::OpenBraceToken
                        || self.token == SyntaxKind::DefaultKeyword
                        || self.token == SyntaxKind::AsKeyword
                        || self.token == SyntaxKind::AtToken
                    {
                        return true;
                    }
                    if self.token == SyntaxKind::TypeKeyword {
                        self.next_token();
                        return self.token == SyntaxKind::AsteriskToken
                            || self.token == SyntaxKind::OpenBraceToken
                            || self.is_identifier() && !self.has_preceding_line_break();
                    }
                    continue;
                }
                SyntaxKind::StaticKeyword => {
                    self.next_token();
                    continue;
                }
                _ => {}
            }
            return false;
        }
    }

    // Go: parser/parser.go:6203 isStartOfExpression
    pub fn is_start_of_expression(&mut self) -> bool {
        if self.is_start_of_left_hand_side_expression() {
            return true;
        }
        match self.token {
            SyntaxKind::PlusToken
            | SyntaxKind::MinusToken
            | SyntaxKind::TildeToken
            | SyntaxKind::ExclamationToken
            | SyntaxKind::DeleteKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::PlusPlusToken
            | SyntaxKind::MinusMinusToken
            | SyntaxKind::LessThanToken
            | SyntaxKind::AwaitKeyword
            | SyntaxKind::YieldKeyword
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::AtToken => {
                // Yield/await always starts an expression.  Either it is an identifier (in which case
                // it is definitely an expression).  Or it's a keyword (either because we're in
                // a generator or async function, or in strict mode (or both)) and it started a yield or await expression.
                return true;
            }
            _ => {}
        }
        // Error tolerance.  If we see the start of some binary operator, we consider
        // that the start of an expression.  That way we'll parse out a missing identifier,
        // give a good message about an identifier being missing, and then consume the
        // rest of the binary expression.
        if self.is_binary_operator() {
            return true;
        }
        self.is_identifier()
    }

    // Go: parser/parser.go:6226 isStartOfLeftHandSideExpression
    pub fn is_start_of_left_hand_side_expression(&mut self) -> bool {
        match self.token {
            SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead
            | SyntaxKind::OpenParenToken
            | SyntaxKind::OpenBracketToken
            | SyntaxKind::OpenBraceToken
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::ClassKeyword
            | SyntaxKind::NewKeyword
            | SyntaxKind::SlashToken
            | SyntaxKind::SlashEqualsToken
            | SyntaxKind::Identifier => return true,
            SyntaxKind::ImportKeyword => {
                return self.is_next_token_open_paren_or_less_than_or_dot();
            }
            _ => {}
        }
        self.is_identifier()
    }

    // Go: parser/parser.go:6239 isStartOfType
    pub fn is_start_of_type(&mut self, in_start_of_parameter: bool) -> bool {
        match self.token {
            SyntaxKind::AnyKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::BigIntKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::UniqueKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::UndefinedKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::ThisKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::OpenBraceToken
            | SyntaxKind::OpenBracketToken
            | SyntaxKind::LessThanToken
            | SyntaxKind::BarToken
            | SyntaxKind::AmpersandToken
            | SyntaxKind::NewKeyword
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::ObjectKeyword
            | SyntaxKind::AsteriskToken
            | SyntaxKind::QuestionToken
            | SyntaxKind::ExclamationToken
            | SyntaxKind::DotDotDotToken
            | SyntaxKind::InferKeyword
            | SyntaxKind::ImportKeyword
            | SyntaxKind::AssertsKeyword
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead => return true,
            SyntaxKind::FunctionKeyword => return !in_start_of_parameter,
            SyntaxKind::MinusToken => {
                return !in_start_of_parameter
                    && self.look_ahead(Parser::next_token_is_numeric_or_big_int_literal);
            }
            SyntaxKind::OpenParenToken => {
                // Only consider '(' the start of a type if followed by ')', '...', an identifier, a modifier,
                // or something that starts a type. We don't want to consider things like '(1)' a type.
                return !in_start_of_parameter
                    && self.look_ahead(Parser::next_is_parenthesized_or_function_type);
            }
            _ => {}
        }
        self.is_identifier()
    }

    // Go: parser/parser.go:6262 nextTokenIsNumericOrBigIntLiteral
    pub fn next_token_is_numeric_or_big_int_literal(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::NumericLiteral || self.token == SyntaxKind::BigIntLiteral
    }

    // Go: parser/parser.go:6267 nextIsParenthesizedOrFunctionType
    pub fn next_is_parenthesized_or_function_type(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::CloseParenToken
            || self.is_start_of_parameter(false /*isJSDocParameter*/)
            || self.is_start_of_type(false /*inStartOfParameter*/)
    }

    // Go: parser/parser.go:6272 isStartOfParameter
    pub fn is_start_of_parameter(&mut self, is_js_doc_parameter: bool) -> bool {
        self.token == SyntaxKind::DotDotDotToken
            || self.is_binding_identifier_or_private_identifier_or_pattern()
            || is_modifier_kind(self.token)
            || self.token == SyntaxKind::AtToken
            || self.is_start_of_type(!is_js_doc_parameter /*inStartOfParameter*/)
    }

    // Go: parser/parser.go:6280 isBindingIdentifierOrPrivateIdentifierOrPattern
    pub fn is_binding_identifier_or_private_identifier_or_pattern(&self) -> bool {
        self.token == SyntaxKind::OpenBraceToken
            || self.token == SyntaxKind::OpenBracketToken
            || self.token == SyntaxKind::PrivateIdentifier
            || self.is_binding_identifier()
    }

    // Go: parser/parser.go:6284 isNextTokenOpenParenOrLessThanOrDot
    pub fn is_next_token_open_paren_or_less_than_or_dot(&mut self) -> bool {
        self.look_ahead(Parser::next_token_is_open_paren_or_less_than_or_dot)
    }

    // Go: parser/parser.go:6288 nextTokenIsOpenParenOrLessThanOrDot
    pub fn next_token_is_open_paren_or_less_than_or_dot(&mut self) -> bool {
        matches!(
            self.next_token(),
            SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken | SyntaxKind::DotToken
        )
    }

    // Go: parser/parser.go:6296 nextTokenIsIdentifierOnSameLine
    pub fn next_token_is_identifier_on_same_line(&mut self) -> bool {
        self.next_token();
        self.is_identifier() && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:6301 nextTokenIsIdentifierOrStringLiteralOnSameLine
    pub fn next_token_is_identifier_or_string_literal_on_same_line(&mut self) -> bool {
        self.next_token();
        (self.is_identifier() || self.token == SyntaxKind::StringLiteral)
            && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:6307 isIdentifier
    // Ignore strict mode flag because we will report an error in type checker instead.
    pub fn is_identifier(&self) -> bool {
        if self.token == SyntaxKind::Identifier {
            return true;
        }
        // If we have a 'yield' keyword, and we're in the [yield] context, then 'yield' is
        // considered a keyword and is not an identifier.
        // If we have a 'await' keyword, and we're in the [Await] context, then 'await' is
        // considered a keyword and is not an identifier.
        if self.token == SyntaxKind::YieldKeyword && self.in_yield_context()
            || self.token == SyntaxKind::AwaitKeyword && self.in_await_context()
        {
            return false;
        }
        (self.token as u16) > (SyntaxKind::LAST_RESERVED_WORD as u16)
    }

    // Go: parser/parser.go:6321 isBindingIdentifier
    pub fn is_binding_identifier(&self) -> bool {
        // `let await`/`let yield` in [Yield] or [Await] are allowed here and disallowed in the binder.
        self.token == SyntaxKind::Identifier
            || (self.token as u16) > (SyntaxKind::LAST_RESERVED_WORD as u16)
    }

    // Go: parser/parser.go:6326 isImportAttributeName
    pub fn is_import_attribute_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == SyntaxKind::StringLiteral
    }

    // Go: parser/parser.go:6330 isBinaryOperator
    pub fn is_binary_operator(&self) -> bool {
        if self.in_disallow_in_context() && self.token == SyntaxKind::InKeyword {
            return false;
        }
        get_binary_operator_precedence(self.token) != OperatorPrecedence::INVALID
    }

    // Go: parser/parser.go:6337 isValidHeritageClauseObjectLiteral
    pub fn is_valid_heritage_clause_object_literal(&mut self) -> bool {
        self.look_ahead(Parser::next_is_valid_heritage_clause_object_literal)
    }

    // Go: parser/parser.go:6341 nextIsValidHeritageClauseObjectLiteral
    pub fn next_is_valid_heritage_clause_object_literal(&mut self) -> bool {
        if self.next_token() == SyntaxKind::CloseBraceToken {
            // if we see "extends {}" then only treat the {} as what we're extending (and not
            // the class body) if we have:
            //
            //      extends {} {
            //      extends {},
            //      extends {} extends
            //      extends {} implements
            let next = self.next_token();
            return next == SyntaxKind::CommaToken
                || next == SyntaxKind::OpenBraceToken
                || next == SyntaxKind::ExtendsKeyword
                || next == SyntaxKind::ImplementsKeyword;
        }
        true
    }

    // Go: parser/parser.go:6356 isHeritageClause
    pub fn is_heritage_clause(&self) -> bool {
        self.token == SyntaxKind::ExtendsKeyword || self.token == SyntaxKind::ImplementsKeyword
    }

    // Go: parser/parser.go:6360 isHeritageClauseExtendsOrImplementsKeyword
    pub fn is_heritage_clause_extends_or_implements_keyword(&mut self) -> bool {
        self.is_heritage_clause() && self.look_ahead(Parser::next_is_start_of_expression)
    }

    // Go: parser/parser.go:6364 nextIsStartOfExpression
    pub fn next_is_start_of_expression(&mut self) -> bool {
        self.next_token();
        self.is_start_of_expression()
    }

    // Go: parser/parser.go:6369 isUsingDeclaration
    pub fn is_using_declaration(&mut self) -> bool {
        // 'using' always starts a lexical declaration if followed by an identifier. We also eagerly parse
        // |ObjectBindingPattern| so that we can report a grammar error during check. We don't parse out
        // |ArrayBindingPattern| since it potentially conflicts with element access (i.e., `using[x]`).
        self.look_ahead(|p: &mut Parser<'a>| {
            p.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
                false, /*disallowOf*/
            )
        })
    }

    // Go: parser/parser.go:6378 nextTokenIsEqualsOrSemicolonOrColonToken
    pub fn next_token_is_equals_or_semicolon_or_colon_token(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::EqualsToken
            || self.token == SyntaxKind::SemicolonToken
            || self.token == SyntaxKind::ColonToken
    }

    // Go: parser/parser.go:6383 nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLine
    pub fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
        &mut self,
        disallow_of: bool,
    ) -> bool {
        self.next_token();
        if disallow_of && self.token == SyntaxKind::OfKeyword {
            return self.look_ahead(Parser::next_token_is_equals_or_semicolon_or_colon_token);
        }
        (self.is_binding_identifier() || self.token == SyntaxKind::OpenBraceToken)
            && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:6391 nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLineDisallowOf
    pub fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of(
        &mut self,
    ) -> bool {
        self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
            true, /*disallowOf*/
        )
    }

    // Go: parser/parser.go:6395 isAwaitUsingDeclaration
    pub fn is_await_using_declaration(&mut self) -> bool {
        self.look_ahead(Parser::next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line)
    }

    // Go: parser/parser.go:6399 nextIsUsingKeywordThenBindingIdentifierOrStartOfObjectDestructuringOnSameLine
    pub fn next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line(
        &mut self,
    ) -> bool {
        self.next_token() == SyntaxKind::UsingKeyword
            && self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
                false, /*disallowOf*/
            )
    }

    // Go: parser/parser.go:6403 nextTokenIsTokenStringLiteral
    pub fn next_token_is_token_string_literal(&mut self) -> bool {
        self.next_token() == SyntaxKind::StringLiteral
    }

    // Go: parser/parser.go:6407 setContextFlags
    pub fn set_context_flags(&mut self, flags: NodeFlags, value: bool) {
        if value {
            self.context_flags = self.context_flags | flags;
        } else {
            self.context_flags = self.context_flags.without(flags);
        }
    }

    // Go: parser/parser.go:6413 (p *Parser) doInContext (ts#63902: was a free generic function)
    pub fn do_in_context<T>(
        &mut self,
        flags: NodeFlags,
        value: bool,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let save_context_flags = self.context_flags;
        self.set_context_flags(flags, value);
        let result = f(self);
        self.context_flags = save_context_flags;
        result
    }

    // Go: parser/parser.go:6423 inYieldContext
    pub fn in_yield_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::YIELD_CONTEXT)
    }

    // Go: parser/parser.go:6427 inDisallowInContext
    pub fn in_disallow_in_context(&self) -> bool {
        self.context_flags
            .intersects(NodeFlags::DISALLOW_IN_CONTEXT)
    }

    // Go: parser/parser.go:6431 inDisallowConditionalTypesContext
    pub fn in_disallow_conditional_types_context(&self) -> bool {
        self.context_flags
            .intersects(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT)
    }

    // Go: parser/parser.go:6435 inDecoratorContext
    pub fn in_decorator_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::DECORATOR_CONTEXT)
    }

    // Go: parser/parser.go:6439 inAwaitContext
    pub fn in_await_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::AWAIT_CONTEXT)
    }

    // Go: parser/parser.go:6443 skipRangeTrivia
    pub fn skip_range_trivia(&self, text_range: TextRange) -> TextRange {
        TextRange::new(
            skip_trivia(self.source_text, text_range.pos()),
            text_range.end(),
        )
    }
}

/// The body of `override_parent_in_immediate_children` through the node
/// reads (`StoreChildLinks`): the path for a node that
/// `set_parent_in_store_children` does not take.
// PORT: Go calls the `p.setParentFromContext` closure field (set in
// initializeClosures: `n.Parent = p.currentParent; return false`).
fn set_parent_in_children_generic(node: Node, parent: Node) {
    let mut links = StoreChildLinks::new(parent);
    node.for_each_child(|n| {
        if !links.set_parent(n) {
            set_node_parent(n, parent);
        }
        false
    });
}

// Go: parser/parser.go:6447 isReservedWord
pub fn is_reserved_word(token: SyntaxKind) -> bool {
    (SyntaxKind::FIRST_RESERVED_WORD as u16) <= (token as u16)
        && (token as u16) <= (SyntaxKind::LAST_RESERVED_WORD as u16)
}

// Go: parser/parser.go:6451 attachFileToDiagnostics
// PORT: Go sets the file on shared diagnostic pointers and returns the same
// slice. Rust diagnostics are owned, so the vector is taken and returned.
pub fn attach_file_to_diagnostics(mut diagnostics: Vec<Diagnostic>, file: Node) -> Vec<Diagnostic> {
    for d in &mut diagnostics {
        d.set_file(file);
        for r in &mut d.related_information {
            r.set_file(file);
        }
    }
    diagnostics
}

// Go: parser/parser.go:6461 getCommentPragmas
pub fn get_comment_pragmas(f: &NodeFactory, source_text: &str) -> Vec<Pragma> {
    let mut pragmas = Vec::new();
    for comment_range in get_leading_comment_ranges(f, source_text, 0) {
        let comment = &source_text[comment_range.pos() as usize..comment_range.end() as usize];
        pragmas.extend(extract_pragmas(comment_range, comment));
    }
    pragmas
}

/// Go `ast.Pragma{CommentRange: commentRange, Name: name, Args: args}`.
// PORT: the existing Rust `Pragma` (program.rs) stores the comment range as
// `range` and `kind`. It has no `has_trailing_new_line`; no Go reader of
// `Pragma` uses that field.
fn new_pragma(
    comment_range: CommentRange,
    name: String,
    args: IndexMap<String, PragmaArgument>,
) -> Pragma {
    Pragma {
        name,
        args,
        range: comment_range.text_range,
        kind: comment_range.kind,
    }
}

// Go: parser/parser.go:6469 extractPragmas
pub fn extract_pragmas(comment_range: CommentRange, text: &str) -> Vec<Pragma> {
    if comment_range.kind == SyntaxKind::SingleLineCommentTrivia {
        let mut pos: i32 = 2;
        let triple_slash = match_(text, pos, "/");
        if triple_slash {
            pos += 1;
        }
        pos = skip_blanks(text, pos);
        if triple_slash && match_(text, pos, "<") {
            let tag_name = extract_name(text, pos + 1);
            if tag_name != "reference" {
                return Vec::new();
            }
            pos += 10;
            let mut args: IndexMap<String, PragmaArgument> = IndexMap::new();
            loop {
                pos = skip_blanks(text, pos);
                if match_(text, pos, "/>") {
                    break;
                }
                let arg_name = extract_name(text, pos);
                if arg_name.is_empty() {
                    break;
                }
                pos = skip_blanks(text, pos + arg_name.len() as i32);
                if !match_(text, pos, "=") {
                    break;
                }
                pos = skip_blanks(text, pos + 1);
                let Some(value) = extract_quoted_string(text, pos) else {
                    break;
                };
                let value_len = value.len() as i32;
                args.insert(
                    arg_name.clone(),
                    PragmaArgument {
                        name: arg_name,
                        value: value.to_string(),
                        range: TextRange::new(
                            comment_range.pos() + pos + 1,
                            comment_range.pos() + pos + 1 + value_len,
                        ),
                    },
                );
                pos += value_len + 2;
            }
            return vec![new_pragma(comment_range, "reference".to_string(), args)];
        }
        if match_(text, pos, "@") {
            pos += 1;
            let pragma_name = extract_name(text, pos);
            if !(pragma_name == "ts-check" || pragma_name == "ts-nocheck") {
                return Vec::new();
            }
            return vec![new_pragma(comment_range, pragma_name, IndexMap::new())];
        }
    }
    if comment_range.kind == SyntaxKind::MultiLineCommentTrivia {
        let text = text.strip_suffix("*/").unwrap_or(text);
        let mut pos: i32 = 2;
        let mut pragmas = Vec::new();
        loop {
            pos = skip_to(text, pos, "@");
            if pos < 0 {
                break;
            }
            // Mirrors the /@(\S+)(\s+(?:\S.*)?)?$/gm pragma regex used by TypeScript: the '@'
            // must be immediately followed by a non-whitespace pragma name, and the remainder
            // of the line is consumed as that pragma's arguments. As a consequence, only the
            // first '@'-token on a line is considered, so an unrelated '@token' earlier on the
            // line (e.g. an email address) prevents a later '@jsx' on the same line from being
            // treated as a pragma.
            let name_pos = pos + 1;
            let name_end = skip_non_blanks(text, name_pos);
            if name_end == name_pos {
                pos += 1;
                continue;
            }
            let line_end = line_end_pos(text, pos);
            let pragma_name = go_to_lower(&text[name_pos as usize..name_end as usize]);
            if pragma_name == "jsx"
                || pragma_name == "jsxfrag"
                || pragma_name == "jsximportsource"
                || pragma_name == "jsxruntime"
            {
                let start = skip_blanks(text, name_end);
                let arg_end = skip_non_blanks(text, start);
                if arg_end != start {
                    let mut args: IndexMap<String, PragmaArgument> = IndexMap::with_capacity(1);
                    args.insert(
                        "factory".to_string(),
                        PragmaArgument {
                            name: "factory".to_string(),
                            value: text[start as usize..arg_end as usize].to_string(),
                            range: TextRange::new(
                                comment_range.pos() + start,
                                comment_range.pos() + arg_end,
                            ),
                        },
                    );
                    pragmas.push(new_pragma(comment_range, pragma_name, args));
                }
            }
            pos = line_end;
        }
        return pragmas;
    }
    Vec::new()
}

/// Go `strings.ToLower`.
// PORT: Go maps each rune with `unicode.ToLower`, a one-rune mapping. Rust
// `str::to_lowercase` uses multi-rune and context rules (U+0130, final
// sigma). Map each char to its first lowercase char, and U+0130 to 'i' as Go
// does.
fn go_to_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\u{130}' {
                'i'
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect()
}

// Go: parser/parser.go:6573 match
fn match_(text: &str, pos: i32, s: &str) -> bool {
    text.as_bytes()[pos as usize..].starts_with(s.as_bytes())
}

// Go: parser/parser.go:6577 skipBlanks
fn skip_blanks(text: &str, mut pos: i32) -> i32 {
    let bytes = text.as_bytes();
    while (pos as usize) < bytes.len()
        && (bytes[pos as usize] == b' ' || bytes[pos as usize] == b'\t')
    {
        pos += 1;
    }
    pos
}

// Go: parser/parser.go:6584 skipNonBlanks
fn skip_non_blanks(text: &str, mut pos: i32) -> i32 {
    let bytes = text.as_bytes();
    while (pos as usize) < bytes.len()
        && !matches!(bytes[pos as usize], b' ' | b'\t' | b'\r' | b'\n')
    {
        pos += 1;
    }
    pos
}

// Go: parser/parser.go:6591 skipTo
fn skip_to(text: &str, pos: i32, s: &str) -> i32 {
    if pos as usize >= text.len() {
        return -1;
    }
    match text[pos as usize..].find(s) {
        Some(i) => pos + i as i32,
        None => -1,
    }
}

// Go: parser/parser.go:6602 lineEndPos
fn line_end_pos(text: &str, mut pos: i32) -> i32 {
    while (pos as usize) < text.len() {
        let ch = text[pos as usize..].chars().next().unwrap_or('\0');
        if is_line_break(ch) {
            return pos;
        }
        pos += ch.len_utf8() as i32;
    }
    text.len() as i32
}

// Go: parser/parser.go:6613 extractName
fn extract_name(text: &str, mut pos: i32) -> String {
    let bytes = text.as_bytes();
    let start = pos;
    while (pos as usize) < bytes.len()
        && (bytes[pos as usize].is_ascii_alphabetic() || bytes[pos as usize] == b'-')
    {
        pos += 1;
    }
    text[start as usize..pos as usize].to_ascii_lowercase()
}

// Go: parser/parser.go:6621 extractQuotedString
// PORT: Go `(string, bool)` is `Option<&str>`.
fn extract_quoted_string(text: &str, mut pos: i32) -> Option<&str> {
    let bytes = text.as_bytes();
    if pos as usize == bytes.len() {
        return None;
    }
    let quote = bytes[pos as usize];
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    pos += 1;
    let start = pos;
    while (pos as usize) < bytes.len() && bytes[pos as usize] != quote {
        pos += 1;
    }
    if pos as usize == bytes.len() {
        return None;
    }
    Some(&text[start as usize..pos as usize])
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:6640 processPragmasIntoFields
    // PORT: Go `context *ast.SourceFile` is the U4 `ParsedSourceFile` field
    // struct. Go `[]*ast.FileReference` is `Vec<FileReference>` and the nil
    // `CheckJsDirective` is `None`.
    pub fn process_pragmas_into_fields(&mut self, context: &mut ParsedSourceFile) {
        context.check_js_directive = None;
        context.referenced_files = Vec::new();
        context.type_reference_directives = Vec::new();
        context.lib_reference_directives = Vec::new();
        // context.AmdDependencies = nil
        for pragma in &context.pragmas {
            match pragma.name.as_str() {
                "reference" => {
                    let types = pragma.args.get("types");
                    let lib = pragma.args.get("lib");
                    let path = pragma.args.get("path");
                    let resolution_mode = pragma.args.get("resolution-mode");
                    let preserve = pragma.args.get("preserve");
                    let no_default_lib = pragma.args.get("no-default-lib");
                    let preserve_value = preserve.is_some_and(|p| p.value == "true");
                    if no_default_lib.is_some_and(|a| a.value == "true") {
                        // Ignored.
                    } else if let Some(types) = types {
                        let mut parsed = ResolutionMode::default();
                        if let Some(resolution_mode) = resolution_mode {
                            parsed = self.parse_resolution_mode(
                                &resolution_mode.value,
                                resolution_mode.range.pos(),
                                resolution_mode.range.end(),
                            );
                        }
                        context.type_reference_directives.push(FileReference {
                            range: types.range,
                            file_name: types.value.clone(),
                            resolution_mode: parsed,
                            preserve: preserve_value,
                        });
                    } else if let Some(lib) = lib {
                        context.lib_reference_directives.push(FileReference {
                            range: lib.range,
                            file_name: lib.value.clone(),
                            preserve: preserve_value,
                            ..Default::default()
                        });
                    } else if let Some(path) = path {
                        context.referenced_files.push(FileReference {
                            range: path.range,
                            file_name: path.value.clone(),
                            preserve: preserve_value,
                            ..Default::default()
                        });
                    } else {
                        self.parse_error_at_range(
                            pragma.range,
                            diag::Invalid_reference_directive_syntax,
                            args![],
                        );
                    }
                }
                "ts-check" | "ts-nocheck" => {
                    // _last_ of either nocheck or check in a file is the "winner"
                    let replace = match &context.check_js_directive {
                        None => true,
                        Some(directive) => pragma.range.pos() > directive.range.pos(),
                    };
                    if replace {
                        // PORT: Go stores the whole CommentRange; the Rust
                        // `CheckJsDirective.range` is its TextRange.
                        context.check_js_directive = Some(CheckJsDirective {
                            enabled: pragma.name == "ts-check",
                            range: pragma.range,
                        });
                    }
                }
                "jsx" | "jsxfrag" | "jsximportsource" | "jsxruntime" => {
                    // Nothing to do here
                }
                _ => panic!("Unhandled pragma kind: {}", pragma.name),
            }
        }
    }

    // Go: parser/parser.go:6700 parseResolutionMode
    pub fn parse_resolution_mode(&mut self, mode: &str, pos: i32, end: i32) -> ResolutionMode {
        if mode == "import" {
            return ModuleKind::ES_NEXT;
        }
        if mode == "require" {
            return ModuleKind::COMMON_JS;
        }
        self.parse_error_at(
            pos,
            end,
            diag::X_resolution_mode_should_be_either_require_or_import,
            args![],
        );
        ResolutionMode::default()
    }

    // Go: parser/parser.go:6713 jsErrorAtRange
    pub fn js_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static Message,
        args: Vec<String>,
    ) {
        let range = TextRange::new(skip_trivia(self.source_text, loc.pos()), loc.end());
        self.js_diagnostics
            .push(new_diagnostic(Node::NIL, range, message, args));
    }

    // Go: parser/parser.go:6717 checkJSDecoratorSyntax
    pub fn check_js_decorator_syntax(&mut self, node: Node) {
        let modifiers = node.modifier_nodes();
        if modifiers.is_empty() {
            return;
        }

        if can_have_illegal_decorators(node) {
            for modifier in modifiers.iter() {
                if is_decorator(modifier) {
                    self.js_error_at_range(
                        modifier.loc(),
                        diag::Decorators_are_not_valid_here,
                        args![],
                    );
                    break;
                }
            }
        } else if can_have_decorators(node) {
            let decorator_index = find_index(modifiers, is_decorator);
            if decorator_index >= 0 && is_class_declaration(node) {
                let export_index = find_index(modifiers, is_export_modifier);
                if export_index >= 0 {
                    let default_index =
                        find_index(modifiers, |m| m.kind() == SyntaxKind::DefaultKeyword);
                    if decorator_index > export_index
                        && default_index >= 0
                        && decorator_index < default_index
                    {
                        // Decorator between `export` and `default`
                        let loc = modifiers.get(decorator_index as usize).loc();
                        self.js_error_at_range(loc, diag::Decorators_are_not_valid_here, args![]);
                    } else if decorator_index < export_index {
                        // Find a trailing decorator after the export keyword
                        let mut trailing_decorator_index: i32 = -1;
                        for i in export_index as usize..modifiers.len() {
                            if is_decorator(modifiers.get(i)) {
                                trailing_decorator_index = i as i32;
                                break;
                            }
                        }
                        if trailing_decorator_index >= 0 {
                            let trailing_loc =
                                modifiers.get(trailing_decorator_index as usize).loc();
                            let decorator_loc = modifiers.get(decorator_index as usize).loc();
                            let mut diag = new_diagnostic(
                                Node::NIL,
                                TextRange::new(skip_trivia(self.source_text, trailing_loc.pos()), trailing_loc.end()),
                                diag::Decorators_may_not_appear_after_export_or_export_default_if_they_also_appear_before_export,
                                args![],
                            );
                            diag.add_related_info(Some(new_diagnostic(
                                Node::NIL,
                                TextRange::new(
                                    skip_trivia(self.source_text, decorator_loc.pos()),
                                    decorator_loc.end(),
                                ),
                                diag::Decorator_used_before_export_here,
                                args![],
                            )));
                            self.js_diagnostics.push(diag);
                        }
                    }
                }
            }
        }
    }

    // Go: parser/parser.go:6771 checkJSSyntax
    // PORT: Go `fallthrough` from the Parameter, PropertyDeclaration and
    // MethodDeclaration case (first switch) and from the class and function
    // case (second switch) is written as a shared follow-up block.
    pub fn check_js_syntax(&mut self, node: Node) -> Node {
        if !node.flags().intersects(NodeFlags::JAVA_SCRIPT_FILE)
            || node
                .flags()
                .intersects(NodeFlags::JS_DOC | NodeFlags::REPARSED)
        {
            return node;
        }
        let kind = node.kind();
        let mut check_signature_or_type = false;
        match kind {
            SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodDeclaration => {
                let token = node.question_token();
                if token.is_some()
                    && !token.flags().intersects(NodeFlags::REPARSED)
                    && is_question_token(token)
                {
                    self.js_error_at_range(
                        token.loc(),
                        diag::The_0_modifier_can_only_be_used_in_TypeScript_files,
                        args!["?"],
                    );
                }
                check_signature_or_type = true;
            }
            SyntaxKind::MethodSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ArrowFunction
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::IndexSignature => {
                check_signature_or_type = true;
            }
            SyntaxKind::ImportDeclaration => {
                let clause = node.import_clause();
                if clause.is_some() && clause.is_type_only() {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        args!["import type"],
                    );
                }
            }
            SyntaxKind::ExportDeclaration => {
                if node.is_type_only() {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        args!["export type"],
                    );
                }
            }
            SyntaxKind::ImportSpecifier => {
                if node.is_type_only() {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        args!["import...type"],
                    );
                }
            }
            SyntaxKind::ExportSpecifier => {
                if node.is_type_only() {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                        args!["export...type"],
                    );
                }
            }
            SyntaxKind::ImportEqualsDeclaration => {
                self.js_error_at_range(
                    node.loc(),
                    diag::X_import_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
            SyntaxKind::ExportAssignment => {
                if node.is_export_equals() {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_export_can_only_be_used_in_TypeScript_files,
                        args![],
                    );
                }
            }
            SyntaxKind::HeritageClause => {
                if node.token() == SyntaxKind::ImplementsKeyword {
                    self.js_error_at_range(
                        node.loc(),
                        diag::X_implements_clauses_can_only_be_used_in_TypeScript_files,
                        args![],
                    );
                }
            }
            SyntaxKind::InterfaceDeclaration => {
                self.js_error_at_range(
                    node.name().loc(),
                    diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                    args!["interface"],
                );
            }
            SyntaxKind::ModuleDeclaration => {
                self.js_error_at_range(
                    node.name().loc(),
                    diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                    args![token_to_string(node.keyword())],
                );
            }
            SyntaxKind::TypeAliasDeclaration => {
                self.js_error_at_range(
                    node.name().loc(),
                    diag::Type_aliases_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
            SyntaxKind::EnumDeclaration => {
                self.js_error_at_range(
                    node.name().loc(),
                    diag::X_0_declarations_can_only_be_used_in_TypeScript_files,
                    args!["enum"],
                );
            }
            SyntaxKind::NonNullExpression => {
                self.js_error_at_range(
                    node.loc(),
                    diag::Non_null_assertions_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
            SyntaxKind::AsExpression => {
                self.js_error_at_range(
                    node.type_().loc(),
                    diag::Type_assertion_expressions_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
            SyntaxKind::SatisfiesExpression => {
                self.js_error_at_range(
                    node.type_().loc(),
                    diag::Type_satisfaction_expressions_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
            _ => {}
        }
        if check_signature_or_type {
            let t = node.type_();
            if is_function_like(node) && node.body().is_nil() {
                self.js_error_at_range(
                    node.loc(),
                    diag::Signature_declarations_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            } else if t.is_some() && !t.flags().intersects(NodeFlags::REPARSED) {
                self.js_error_at_range(
                    t.loc(),
                    diag::Type_annotations_can_only_be_used_in_TypeScript_files,
                    args![],
                );
            }
        }
        // Check decorator placement in JS files
        self.check_js_decorator_syntax(node);
        // Check absence of type parameters, type arguments and non-JavaScript modifiers
        let mut check_modifiers = false;
        match kind {
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ArrowFunction => {
                let list = node.type_parameter_list();
                if !list.is_nil()
                    && list
                        .nodes()
                        .iter()
                        .any(|n| !n.flags().intersects(NodeFlags::REPARSED))
                {
                    self.js_error_at_range(
                        list.loc(),
                        diag::Type_parameter_declarations_can_only_be_used_in_TypeScript_files,
                        args![],
                    );
                }
                check_modifiers = true;
            }
            SyntaxKind::VariableStatement | SyntaxKind::PropertyDeclaration => {
                check_modifiers = true;
            }
            SyntaxKind::Parameter => {
                if node.modifier_nodes().iter().any(is_modifier) {
                    self.js_error_at_range(
                        node.modifiers().loc(),
                        diag::Parameter_modifiers_can_only_be_used_in_TypeScript_files,
                        args![],
                    );
                }
            }
            SyntaxKind::CallExpression
            | SyntaxKind::NewExpression
            | SyntaxKind::ExpressionWithTypeArguments
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxOpeningElement
            | SyntaxKind::TaggedTemplateExpression => {
                let list = node.type_argument_list();
                if !list.is_nil()
                    && list
                        .nodes()
                        .iter()
                        .any(|n| !n.flags().intersects(NodeFlags::REPARSED))
                {
                    self.js_error_at_range(
                        list.loc(),
                        diag::Type_arguments_can_only_be_used_in_TypeScript_files,
                        args![],
                    );
                }
            }
            _ => {}
        }
        if check_modifiers {
            for modifier in node.modifier_nodes().iter() {
                if !modifier.flags().intersects(NodeFlags::REPARSED)
                    && modifier.kind() != SyntaxKind::Decorator
                    && !modifier_to_flag(modifier.kind()).intersects(ModifierFlags::JAVA_SCRIPT)
                {
                    self.js_error_at_range(
                        modifier.loc(),
                        diag::The_0_modifier_can_only_be_used_in_TypeScript_files,
                        args![token_to_string(modifier.kind())],
                    );
                }
            }
        }
        node
    }
}

/// Go `core.FindIndex` over modifier nodes.
fn find_index(nodes: NodeSlice, mut f: impl FnMut(Node) -> bool) -> i32 {
    nodes.iter().position(|n| f(n)).map_or(-1, |i| i as i32)
}
