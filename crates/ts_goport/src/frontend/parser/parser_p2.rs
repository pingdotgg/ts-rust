//! Port of typescript-go `internal/parser/parser.go` lines 1409-2826:
//! statements (switch, throw, try, labeled), variable declarations, binding
//! patterns, functions, classes and class members, interfaces, type aliases,
//! enums, modules, imports, exports and the start of type parsing.
//!
//! PORT: all `factory.NewX` calls hoist their arguments into locals first.
//! `self.factory` is borrowed immutably by the call, and a nested
//! `self.parse_x()` needs `&mut self`. Go evaluates the arguments left to
//! right before the call, so the order of parse calls is the same.
//!
//! PORT: Go `*diagnostics.Message` parameters that can be nil are
//! `Option<&'static Message>`. Go `nodeSliceArena` allocations are plain
//! slices passed to `new_node_list` / `new_modifier_list`.

use crate::frontend::prelude::*;

impl<'a> Parser<'a> {
    // Go: parser/parser.go:1407 parseDefaultClause
    pub fn parse_default_clause(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::DefaultKeyword);
        self.parse_expected(SyntaxKind::ColonToken);
        let statements = self.parse_list(
            ParsingContext::SwitchClauseStatements,
            Parser::parse_statement,
        );
        let node = self.factory.new_case_or_default_clause(
            SyntaxKind::DefaultClause,
            Node::NIL, /*expression*/
            statements,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1418 parseCaseOrDefaultClause
    pub fn parse_case_or_default_clause(&mut self) -> Node {
        if self.token == SyntaxKind::CaseKeyword {
            return self.parse_case_clause();
        }
        self.parse_default_clause()
    }

    // Go: parser/parser.go:1425 parseCaseBlock
    pub fn parse_case_block(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::OpenBraceToken);
        let clauses = self.parse_list(
            ParsingContext::SwitchClauses,
            Parser::parse_case_or_default_clause,
        );
        self.parse_expected(SyntaxKind::CloseBraceToken);
        let node = self.factory.new_case_block(clauses);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1436 parseSwitchStatement
    pub fn parse_switch_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::SwitchKeyword);
        self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(SyntaxKind::CloseParenToken);
        let case_block = self.parse_case_block();
        let node = self.factory.new_switch_statement(expression, case_block);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1449 parseThrowStatement
    pub fn parse_throw_statement(&mut self) -> Node {
        // ThrowStatement[Yield] :
        //      throw [no LineTerminator here]Expression[In, ?Yield];
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::ThrowKeyword);
        // Because of automatic semicolon insertion, we need to report error if this
        // throw could be terminated with a semicolon.  Note: we can't call 'parseExpression'
        // directly as that might consume an expression on the following line.
        // Instead, we create a "missing" identifier, but don't report an error. The actual error
        // will be reported in the grammar walker.
        let expression = if !self.has_preceding_line_break() {
            self.parse_expression_allow_in()
        } else {
            self.create_missing_identifier()
        };
        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let node = self.factory.new_throw_statement(expression);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1475 parseTryStatement
    // TODO: Review for error recovery
    pub fn parse_try_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::TryKeyword);
        let try_block = self.parse_block(false /*ignoreMissingOpenBrace*/, None);
        let mut catch_clause = Node::NIL;
        if self.token == SyntaxKind::CatchKeyword {
            catch_clause = self.parse_catch_clause();
        }
        // If we don't have a catch clause, then we must have a finally clause.  Try to parse
        // one out no matter what.
        let mut finally_block = Node::NIL;
        if catch_clause.is_nil() || self.token == SyntaxKind::FinallyKeyword {
            self.parse_expected_with_diagnostic(
                SyntaxKind::FinallyKeyword,
                Some(diag::X_catch_or_finally_expected),
                true, /*shouldAdvance*/
            );
            finally_block = self.parse_block(false /*ignoreMissingOpenBrace*/, None);
        }
        let node = self
            .factory
            .new_try_statement(try_block, catch_clause, finally_block);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1496 parseCatchClause
    pub fn parse_catch_clause(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::CatchKeyword);
        let mut variable_declaration = Node::NIL;
        if self.parse_optional(SyntaxKind::OpenParenToken) {
            variable_declaration = self.parse_variable_declaration();
            self.parse_expected(SyntaxKind::CloseParenToken);
        }
        let block = self.parse_block(false /*ignoreMissingOpenBrace*/, None);
        let node = self.factory.new_catch_clause(variable_declaration, block);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1509 parseDebuggerStatement
    pub fn parse_debugger_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::DebuggerKeyword);
        self.parse_semicolon();
        let node = self.factory.new_debugger_statement();
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1519 parseExpressionOrLabeledStatement
    pub fn parse_expression_or_labeled_statement(&mut self) -> Node {
        // Avoiding having to do the lookahead for a labeled statement by just trying to parse
        // out an expression, seeing if it is identifier and then seeing if it is followed by
        // a colon.
        let pos = self.node_pos();
        let mut jsdoc = self.jsdoc_scanner_info();
        let has_paren = self.token == SyntaxKind::OpenParenToken;
        let expression = self.parse_expression();

        if expression.kind() == SyntaxKind::Identifier
            && self.parse_optional(SyntaxKind::ColonToken)
        {
            let statement = self.parse_statement();
            let node = self.factory.new_labeled_statement(expression, statement);
            let result = self.finish_node(node, pos);
            self.with_js_doc(result, jsdoc);
            return result;
        }

        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let node = self.factory.new_expression_statement(expression);
        let result = self.finish_node(node, pos);
        if has_paren {
            jsdoc &= !JSDOC_SCANNER_INFO_HAS_JS_DOC;
        }
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1545 parseVariableStatement
    pub fn parse_variable_statement(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let declaration_list =
            self.parse_variable_declaration_list(false /*inForStatementInitializer*/);
        self.parse_semicolon();
        let node = self
            .factory
            .new_variable_statement(modifiers, declaration_list);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:1554 parseVariableDeclarationList
    pub fn parse_variable_declaration_list(&mut self, in_for_statement_initializer: bool) -> Node {
        let pos = self.node_pos();
        let mut flags = NodeFlags::default();
        match self.token {
            SyntaxKind::VarKeyword => flags = NodeFlags::NONE,
            SyntaxKind::LetKeyword => flags = NodeFlags::LET,
            SyntaxKind::ConstKeyword => flags = NodeFlags::CONST,
            SyntaxKind::UsingKeyword => flags = NodeFlags::USING,
            SyntaxKind::AwaitKeyword => {
                // PORT: Go `break` leaves the switch with flags unset.
                if self.is_await_using_declaration() {
                    flags = NodeFlags::AWAIT_USING;
                    self.next_token();
                }
            }
            _ => panic!("Unhandled case in parseVariableDeclarationList"),
        }
        self.next_token();
        // The user may have written the following:
        //
        //    for (let of X) { }
        //
        // In this case, we want to parse an empty declaration list, and then parse 'of'
        // as a keyword. The reason this is not automatic is that 'of' is a valid identifier.
        // So we need to look ahead to determine if 'of' should be treated as a keyword in
        // this context.
        // The checker will then give an error that there is an empty declaration list.
        let declarations;
        if self.token == SyntaxKind::OfKeyword
            && self.look_ahead(Parser::next_is_identifier_and_close_paren)
        {
            declarations = self.create_missing_list();
        } else {
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, in_for_statement_initializer);
            let parse_element: fn(&mut Parser<'a>) -> Node = if in_for_statement_initializer {
                Parser::parse_variable_declaration
            } else {
                Parser::parse_variable_declaration_allow_exclamation
            };
            declarations =
                self.parse_delimited_list(ParsingContext::VariableDeclarations, parse_element);
            self.context_flags = save_context_flags;
        }
        let node = self
            .factory
            .new_variable_declaration_list(declarations, flags);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1598 nextIsIdentifierAndCloseParen
    pub fn next_is_identifier_and_close_paren(&mut self) -> bool {
        self.next_token_is_identifier() && self.next_token() == SyntaxKind::CloseParenToken
    }

    // Go: parser/parser.go:1602 nextTokenIsIdentifier
    pub fn next_token_is_identifier(&mut self) -> bool {
        self.next_token();
        self.is_identifier()
    }

    // Go: parser/parser.go:1607 parseVariableDeclaration
    pub fn parse_variable_declaration(&mut self) -> Node {
        self.parse_variable_declaration_worker(false /*allowExclamation*/)
    }

    // Go: parser/parser.go:1611 parseVariableDeclarationAllowExclamation
    pub fn parse_variable_declaration_allow_exclamation(&mut self) -> Node {
        self.parse_variable_declaration_worker(true /*allowExclamation*/)
    }

    // Go: parser/parser.go:1615 parseVariableDeclarationWorker
    pub fn parse_variable_declaration_worker(&mut self, allow_exclamation: bool) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            diag::Private_identifiers_are_not_allowed_in_variable_declarations,
        ));
        let mut exclamation_token = Node::NIL;
        if allow_exclamation
            && name.kind() == SyntaxKind::Identifier
            && self.token == SyntaxKind::ExclamationToken
            && !self.has_preceding_line_break()
        {
            exclamation_token = self.parse_token_node();
        }
        let type_node = self.parse_type_annotation();
        let mut initializer = Node::NIL;
        if self.token != SyntaxKind::InKeyword && self.token != SyntaxKind::OfKeyword {
            initializer = self.parse_initializer();
        }
        let node =
            self.factory
                .new_variable_declaration(name, exclamation_token, type_node, initializer);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:1634 parseIdentifierOrPattern
    pub fn parse_identifier_or_pattern(&mut self) -> Node {
        self.parse_identifier_or_pattern_with_diagnostic(None)
    }

    // Go: parser/parser.go:1638 parseIdentifierOrPatternWithDiagnostic
    pub fn parse_identifier_or_pattern_with_diagnostic(
        &mut self,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> Node {
        if self.token == SyntaxKind::OpenBracketToken {
            return self.parse_array_binding_pattern();
        }
        if self.token == SyntaxKind::OpenBraceToken {
            return self.parse_object_binding_pattern();
        }
        self.parse_binding_identifier_with_diagnostic(private_identifier_diagnostic_message)
    }

    // Go: parser/parser.go:1648 parseArrayBindingPattern
    pub fn parse_array_binding_pattern(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBracketToken);
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(
            ParsingContext::ArrayBindingElements,
            Parser::parse_array_binding_element,
        );
        self.context_flags = save_context_flags;
        self.parse_expected(SyntaxKind::CloseBracketToken);
        let node = self
            .factory
            .new_binding_pattern(SyntaxKind::ArrayBindingPattern, elements);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1659 parseArrayBindingElement
    pub fn parse_array_binding_element(&mut self) -> Node {
        let pos = self.node_pos();
        let mut dot_dot_dot_token = Node::NIL;
        let mut name = Node::NIL;
        let mut initializer = Node::NIL;
        if self.token != SyntaxKind::CommaToken {
            // These are all nil for a missing element
            dot_dot_dot_token = self.parse_optional_token(SyntaxKind::DotDotDotToken);
            name = self.parse_identifier_or_pattern();
            initializer = self.parse_initializer();
        }
        let node = self.factory.new_binding_element(
            dot_dot_dot_token,
            Node::NIL, /*propertyName*/
            name,
            initializer,
        );
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1673 parseObjectBindingPattern
    pub fn parse_object_binding_pattern(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBraceToken);
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(
            ParsingContext::ObjectBindingElements,
            Parser::parse_object_binding_element,
        );
        self.context_flags = save_context_flags;
        self.parse_expected(SyntaxKind::CloseBraceToken);
        let node = self
            .factory
            .new_binding_pattern(SyntaxKind::ObjectBindingPattern, elements);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1684 parseObjectBindingElement
    pub fn parse_object_binding_element(&mut self) -> Node {
        let pos = self.node_pos();
        let dot_dot_dot_token = self.parse_optional_token(SyntaxKind::DotDotDotToken);
        let token_is_identifier = self.is_binding_identifier();
        let mut property_name = self.parse_property_name();
        let name;
        if token_is_identifier && self.token != SyntaxKind::ColonToken {
            name = property_name;
            property_name = Node::NIL;
        } else {
            self.parse_expected(SyntaxKind::ColonToken);
            name = self.parse_identifier_or_pattern();
        }
        let initializer = self.parse_initializer();
        let node =
            self.factory
                .new_binding_element(dot_dot_dot_token, property_name, name, initializer);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1701 parseInitializer
    pub fn parse_initializer(&mut self) -> Node {
        if self.parse_optional(SyntaxKind::EqualsToken) {
            return self.parse_assignment_expression_or_higher();
        }
        Node::NIL
    }

    // Go: parser/parser.go:1708 parseTypeAnnotation
    pub fn parse_type_annotation(&mut self) -> Node {
        if self.parse_optional(SyntaxKind::ColonToken) {
            return self.parse_type();
        }
        Node::NIL
    }

    // Go: parser/parser.go:1715 parseFunctionDeclaration
    pub fn parse_function_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected(SyntaxKind::FunctionKeyword);
        let asterisk_token = self.parse_optional_token(SyntaxKind::AsteriskToken);
        // We don't parse the name here in await context, instead we will report a grammar error in the checker.
        let mut name = Node::NIL;
        if modifiers.is_nil()
            || !modifiers
                .modifier_flags()
                .intersects(ModifierFlags::DEFAULT)
            || self.is_binding_identifier()
        {
            name = self.parse_binding_identifier();
        }
        let signature_flags = (if asterisk_token.is_some() {
            ParseFlags::YIELD
        } else {
            ParseFlags::NONE
        }) | (if modifiers.is_some()
            && modifiers.modifier_flags().intersects(ModifierFlags::ASYNC)
        {
            ParseFlags::AWAIT
        } else {
            ParseFlags::NONE
        });
        let type_parameters = self.parse_type_parameters();
        let save_context_flags = self.context_flags;
        if modifiers.is_some() && modifiers.modifier_flags().intersects(ModifierFlags::EXPORT) {
            self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        }
        let parameters = self.parse_parameters(signature_flags);
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
        let body =
            self.parse_function_block_or_semicolon(signature_flags, Some(diag::X_or_expected));
        self.context_flags = save_context_flags;
        let node = self.factory.new_function_declaration(
            modifiers,
            asterisk_token,
            name,
            type_parameters,
            parameters,
            return_type,
            Node::NIL, /*fullSignature*/
            body,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:1739 parseClassDeclaration
    pub fn parse_class_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_class_declaration_or_expression(
            pos,
            jsdoc,
            modifiers,
            SyntaxKind::ClassDeclaration,
        )
    }

    // Go: parser/parser.go:1743 parseClassExpression
    pub fn parse_class_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_class_declaration_or_expression(
            pos,
            jsdoc,
            ModifierList::NIL, /*modifiers*/
            SyntaxKind::ClassExpression,
        )
    }

    // Go: parser/parser.go:1747 parseClassDeclarationOrExpression
    pub fn parse_class_declaration_or_expression(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        kind: SyntaxKind,
    ) -> Node {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.parse_expected(SyntaxKind::ClassKeyword);
        // We don't parse the name here in await context, instead we will report a grammar error in the checker.
        let name = self.parse_name_of_class_declaration_or_expression();
        let type_parameters = self.parse_type_parameters();
        if modifiers.is_some()
            && self.parsing_contexts & (1 << (ParsingContext::SourceElements as i32)) != 0
            && self.parsing_contexts
                & ((1 << (ParsingContext::BlockStatements as i32))
                    | (1 << (ParsingContext::SwitchClauseStatements as i32)))
                == 0
            && modifiers.nodes().iter().any(is_export_modifier)
        {
            self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true /*value*/);
        }
        let heritage_clauses = self.parse_heritage_clauses(false /*isInterface*/);
        let members;
        if self.parse_expected(SyntaxKind::OpenBraceToken) {
            // ClassTail[Yield,Await] : (Modified) See 14.5
            //      ClassHeritage[?Yield,?Await]opt { ClassBody[?Yield,?Await]opt }
            members = self.parse_list(ParsingContext::ClassMembers, Parser::parse_class_element);
            self.parse_expected(SyntaxKind::CloseBraceToken);
        } else {
            members = self.create_missing_list();
        }
        self.context_flags = save_context_flags;
        if modifiers.is_some()
            && modifiers_to_flags(&modifiers.nodes().to_vec()).intersects(ModifierFlags::AMBIENT)
        {
            self.statement_has_await_identifier = save_has_await_identifier;
        }
        let result = if kind == SyntaxKind::ClassDeclaration {
            self.factory.new_class_declaration(
                modifiers,
                name,
                type_parameters,
                heritage_clauses,
                members,
            )
        } else {
            self.factory.new_class_expression(
                modifiers,
                name,
                type_parameters,
                heritage_clauses,
                members,
            )
        };
        self.finish_node(result, pos);
        self.with_js_doc(result, jsdoc);
        if result.flags().intersects(NodeFlags::JAVA_SCRIPT_FILE) {
            self.check_js_syntax(result);
            if heritage_clauses.is_some() {
                for clause in heritage_clauses.nodes().iter() {
                    if clause.token() == SyntaxKind::ExtendsKeyword {
                        for expr in clause.types().nodes().iter() {
                            self.check_js_syntax(expr);
                        }
                    }
                }
            }
        }
        result
    }

    // Go: parser/parser.go:1797 parseNameOfClassDeclarationOrExpression
    pub fn parse_name_of_class_declaration_or_expression(&mut self) -> Node {
        // implements is a future reserved word so
        // 'class implements' might mean either
        // - class expression with omitted name, 'implements' starts heritage clause
        // - class with name 'implements'
        // 'isImplementsClause' helps to disambiguate between these two cases
        if self.is_binding_identifier() && !self.is_implements_clause() {
            let save_has_await_identifier = self.statement_has_await_identifier;
            let is_binding_identifier = self.is_binding_identifier();
            let id = self.create_identifier(is_binding_identifier);
            self.statement_has_await_identifier = save_has_await_identifier;
            return id;
        }
        Node::NIL
    }

    // Go: parser/parser.go:1812 isImplementsClause
    pub fn is_implements_clause(&mut self) -> bool {
        self.token == SyntaxKind::ImplementsKeyword
            && self.look_ahead(Parser::next_token_is_identifier_or_keyword)
    }
}

// Go: parser/parser.go:1816 isExportModifier
pub fn is_export_modifier(modifier: Node) -> bool {
    modifier.kind() == SyntaxKind::ExportKeyword
}

// Go: parser/parser.go:1820 isAsyncModifier
pub fn is_async_modifier(modifier: Node) -> bool {
    modifier.kind() == SyntaxKind::AsyncKeyword
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:1824 parseHeritageClauses
    pub fn parse_heritage_clauses(&mut self, is_interface: bool) -> NodeList {
        // ClassTail[Yield,Await] : (Modified) See 14.5
        //      ClassHeritage[?Yield,?Await]opt { ClassBody[?Yield,?Await]opt }
        if self.is_heritage_clause() {
            return self.parse_list(ParsingContext::HeritageClauses, |p: &mut Parser<'a>| {
                p.parse_heritage_clause(is_interface)
            });
        }
        NodeList::NIL
    }

    // Go: parser/parser.go:1835 parseHeritageClause
    pub fn parse_heritage_clause(&mut self, is_interface: bool) -> Node {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        let mut parse_element: fn(&mut Parser<'a>) -> Node =
            Parser::parse_expression_with_type_arguments;
        if is_type_heritage_clause(is_interface, kind) {
            parse_element = Parser::parse_type_heritage_clause_element;
        }
        let types = self.parse_delimited_list(ParsingContext::HeritageClauseElement, parse_element);
        let node = self.factory.new_heritage_clause(kind, types);
        let node = self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
}

// Go: parser/parser.go:1847 isTypeHeritageClause
pub fn is_type_heritage_clause(is_interface: bool, token: SyntaxKind) -> bool {
    is_interface && token == SyntaxKind::ExtendsKeyword
        || !is_interface && token == SyntaxKind::ImplementsKeyword
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:1852 parseTypeHeritageClauseElement
    /// Go returns an `*ast.HeritageClauseElement`: a TypeReference, or the
    /// ExpressionWithTypeArguments when its expression is not an entity name.
    pub fn parse_type_heritage_clause_element(&mut self) -> Node {
        let pos = self.node_pos();
        let expression_with_type_arguments = self.parse_expression_with_type_arguments();
        if !is_valid_heritage_type_reference_expression(expression_with_type_arguments.expression())
        {
            return expression_with_type_arguments;
        }
        let type_name = self.convert_entity_name_expression_to_entity_name(
            expression_with_type_arguments.expression(),
        );
        let type_arguments = expression_with_type_arguments.type_argument_list();
        let result = self
            .factory
            .new_type_reference_node(type_name, type_arguments);
        self.finish_node(result, pos)
    }
}

// Go: parser/parser.go:1862 isValidHeritageTypeReferenceExpression
pub fn is_valid_heritage_type_reference_expression(node: Node) -> bool {
    if is_identifier(node) {
        return node_is_present(node);
    }
    is_property_access_expression(node)
        && !is_optional_chain(node)
        && node_is_present(node.name())
        && is_valid_heritage_type_reference_expression(node.expression())
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:1872 convertEntityNameExpressionToEntityName
    pub fn convert_entity_name_expression_to_entity_name(&mut self, node: Node) -> Node {
        if is_identifier(node) {
            return node;
        }
        let left = self.convert_entity_name_expression_to_entity_name(node.expression());
        let right = node.name();
        let result = self.factory.new_qualified_name(left, right);
        self.finish_node_with_end(result, node.pos(), node.end())
    }

    // Go: parser/parser.go:1884 parseExpressionWithTypeArguments
    pub fn parse_expression_with_type_arguments(&mut self) -> Node {
        let pos = self.node_pos();
        let expression = self.parse_left_hand_side_expression_or_higher();
        if is_expression_with_type_arguments(expression) {
            return expression;
        }
        let type_arguments = self.parse_type_arguments();
        let node = self
            .factory
            .new_expression_with_type_arguments(expression, type_arguments);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:1894 parseClassElement
    pub fn parse_class_element(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.token == SyntaxKind::SemicolonToken {
            self.next_token();
            let node = self.factory.new_semicolon_class_element();
            let result = self.finish_node(node, pos);
            self.with_js_doc(result, jsdoc);
            return result;
        }
        let modifiers = self.parse_modifiers_ex(
            true, /*allowDecorators*/
            true, /*permitConstAsModifier*/
            true, /*stopOnStartOfClassStaticBlock*/
        );
        if self.token == SyntaxKind::StaticKeyword
            && self.look_ahead(Parser::next_token_is_open_brace)
        {
            return self.parse_class_static_block_declaration(pos, jsdoc, modifiers);
        }
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
        if self.token == SyntaxKind::ConstructorKeyword || self.token == SyntaxKind::StringLiteral {
            let constructor_declaration =
                self.try_parse_constructor_declaration(pos, jsdoc, modifiers);
            if constructor_declaration.is_some() {
                return constructor_declaration;
            }
        }
        if self.is_index_signature() {
            let node = self.parse_index_signature_declaration(pos, jsdoc, modifiers);
            return self.check_js_syntax(node);
        }
        // It is very important that we check this *after* checking indexers because
        // the [ token can start an index signature or a computed property name
        if token_is_identifier_or_keyword(self.token)
            || self.token == SyntaxKind::StringLiteral
            || self.token == SyntaxKind::NumericLiteral
            || self.token == SyntaxKind::BigIntLiteral
            || self.token == SyntaxKind::AsteriskToken
            || self.token == SyntaxKind::OpenBracketToken
        {
            let is_ambient =
                modifiers.is_some() && modifiers.nodes().iter().any(is_declare_modifier);
            if is_ambient {
                for m in modifiers.nodes().iter() {
                    set_node_flags(m, m.flags() | NodeFlags::AMBIENT);
                }
                let save_context_flags = self.context_flags;
                self.set_context_flags(NodeFlags::AMBIENT, true);
                let result = self.parse_property_or_method_declaration(pos, jsdoc, modifiers);
                self.context_flags = save_context_flags;
                return result;
            } else {
                return self.parse_property_or_method_declaration(pos, jsdoc, modifiers);
            }
        }
        if modifiers.is_some() {
            // treat this as a property declaration with a missing name.
            let node_pos = self.node_pos();
            self.parse_error_at(node_pos, node_pos, diag::Declaration_expected, args![]);
            let name = self.create_missing_identifier();
            return self.parse_property_declaration(
                pos,
                jsdoc,
                modifiers,
                name,
                Node::NIL, /*questionToken*/
            );
        }
        // 'isClassMemberStart' should have hinted not to attempt parsing.
        panic!("Should not have attempted to parse class member declaration.");
    }

    // Go: parser/parser.go:1949 parseClassStaticBlockDeclaration
    pub fn parse_class_static_block_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected_token(SyntaxKind::StaticKeyword);
        let body = self.parse_class_static_block_body();
        let node = self
            .factory
            .new_class_static_block_declaration(modifiers, body);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:1957 parseClassStaticBlockBody
    pub fn parse_class_static_block_body(&mut self) -> Node {
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, false);
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let body = self.parse_block(
            false, /*ignoreMissingOpenBrace*/
            None,  /*diagnosticMessage*/
        );
        self.context_flags = save_context_flags;
        body
    }

    // Go: parser/parser.go:1966 tryParseConstructorDeclaration
    pub fn try_parse_constructor_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let state = self.mark();
        if self.token == SyntaxKind::ConstructorKeyword
            || self.token == SyntaxKind::StringLiteral
                && self.scanner.token_value() == "constructor"
                && self.look_ahead(Parser::next_token_is_open_paren)
        {
            self.next_token();
            let type_parameters = self.parse_type_parameters();
            let parameters = self.parse_parameters(ParseFlags::NONE);
            let return_type = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
            let body =
                self.parse_function_block_or_semicolon(ParseFlags::NONE, Some(diag::X_or_expected));
            let node = self.factory.new_constructor_declaration(
                modifiers,
                type_parameters,
                parameters,
                return_type,
                Node::NIL, /*fullSignature*/
                body,
            );
            let result = self.finish_node(node, pos);
            self.with_js_doc(result, jsdoc);
            self.check_js_syntax(result);
            return result;
        }
        self.rewind(state);
        Node::NIL
    }

    // Go: parser/parser.go:1983 nextTokenIsOpenParen
    pub fn next_token_is_open_paren(&mut self) -> bool {
        self.next_token() == SyntaxKind::OpenParenToken
    }

    // Go: parser/parser.go:1987 parsePropertyOrMethodDeclaration
    pub fn parse_property_or_method_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let asterisk_token = self.parse_optional_token(SyntaxKind::AsteriskToken);
        let name = self.parse_property_name();
        // Note: this is not legal as per the grammar.  But we allow it in the parser and
        // report an error in the grammar checker.
        let question_token = self.parse_optional_token(SyntaxKind::QuestionToken);
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
                question_token,
                Some(diag::X_or_expected),
            );
        }
        self.parse_property_declaration(pos, jsdoc, modifiers, name, question_token)
    }

    // Go: parser/parser.go:1999 parseMethodDeclaration
    #[allow(clippy::too_many_arguments)]
    pub fn parse_method_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        asterisk_token: Node,
        name: Node,
        question_token: Node,
        diagnostic_message: Option<&'static Message>,
    ) -> Node {
        let signature_flags = (if asterisk_token.is_some() {
            ParseFlags::YIELD
        } else {
            ParseFlags::NONE
        }) | (if modifier_list_has_async(modifiers) {
            ParseFlags::AWAIT
        } else {
            ParseFlags::NONE
        });
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(signature_flags);
        let type_node = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
        let body = self.parse_function_block_or_semicolon(signature_flags, diagnostic_message);
        let node = self.factory.new_method_declaration(
            modifiers,
            asterisk_token,
            name,
            question_token,
            type_parameters,
            parameters,
            type_node,
            Node::NIL, /*fullSignature*/
            body,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }
}

// Go: parser/parser.go:2011 modifierListHasAsync
pub fn modifier_list_has_async(modifiers: ModifierList) -> bool {
    modifiers.is_some() && modifiers.nodes().iter().any(is_async_modifier)
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:2015 parsePropertyDeclaration
    pub fn parse_property_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        name: Node,
        question_token: Node,
    ) -> Node {
        let mut postfix_token = question_token;
        if postfix_token.is_nil() && !self.has_preceding_line_break() {
            postfix_token = self.parse_optional_token(SyntaxKind::ExclamationToken);
        }
        let type_node = self.parse_type_annotation();
        let initializer = self.do_in_context(
            NodeFlags::YIELD_CONTEXT | NodeFlags::AWAIT_CONTEXT | NodeFlags::DISALLOW_IN_CONTEXT,
            false,
            Parser::parse_initializer,
        );
        self.parse_semicolon_after_property_name(name, type_node, initializer);
        let node = self.factory.new_property_declaration(
            modifiers,
            name,
            postfix_token,
            type_node,
            initializer,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2029 parseSemicolonAfterPropertyName
    pub fn parse_semicolon_after_property_name(
        &mut self,
        name: Node,
        type_node: Node,
        initializer: Node,
    ) {
        if self.token == SyntaxKind::AtToken && !self.has_preceding_line_break() {
            self.parse_error_at_current_token(
                diag::Decorators_must_precede_the_name_and_all_keywords_of_property_declarations,
                args![],
            );
            return;
        }
        if self.token == SyntaxKind::OpenParenToken {
            self.parse_error_at_current_token(
                diag::Cannot_start_a_function_call_in_a_type_annotation,
                args![],
            );
            self.next_token();
            return;
        }
        if type_node.is_some() && !self.can_parse_semicolon() {
            if initializer.is_some() {
                self.parse_error_at_current_token(
                    diag::X_0_expected,
                    args![token_to_string(SyntaxKind::SemicolonToken)],
                );
            } else {
                self.parse_error_at_current_token(diag::Expected_for_property_initializer, args![]);
            }
            return;
        }
        if self.try_parse_semicolon() {
            return;
        }
        if initializer.is_some() {
            self.parse_error_at_current_token(
                diag::X_0_expected,
                args![token_to_string(SyntaxKind::SemicolonToken)],
            );
            return;
        }
        self.parse_error_for_missing_semicolon_after(name);
    }

    // Go: parser/parser.go:2057 parseErrorForMissingSemicolonAfter
    pub fn parse_error_for_missing_semicolon_after(&mut self, node: Node) {
        // Tagged template literals are sometimes used in places where only simple strings are allowed, i.e.:
        //   module `M1` {
        //   ^^^^^^^^^^^ This block is parsed as a template literal like module`M1`.
        if node.kind() == SyntaxKind::TaggedTemplateExpression {
            let range = self.skip_range_trivia(node.template().loc());
            self.parse_error_at_range(
                range,
                diag::Module_declaration_names_may_only_use_or_quoted_strings,
                args![],
            );
            return;
        }
        // Otherwise, if this isn't a well-known keyword-like identifier, give the generic fallback message.
        let mut expression_text = "";
        if node.kind() == SyntaxKind::Identifier {
            expression_text = node.text();
        }
        if expression_text.is_empty() {
            self.parse_error_at_current_token(
                diag::X_0_expected,
                args![token_to_string(SyntaxKind::SemicolonToken)],
            );
            return;
        }
        let pos = skip_trivia(self.source_text, node.pos());
        // Some known keywords are likely signs of syntax being used improperly.
        match expression_text {
            "const" | "let" | "var" => {
                self.parse_error_at(
                    pos,
                    node.end(),
                    diag::Variable_declaration_not_allowed_at_this_location,
                    args![],
                );
                return;
            }
            "declare" => {
                // If a declared node failed to parse, it would have emitted a diagnostic already.
                return;
            }
            "interface" => {
                self.parse_error_for_invalid_name(
                    diag::Interface_name_cannot_be_0,
                    diag::Interface_must_be_given_a_name,
                    SyntaxKind::OpenBraceToken,
                );
                return;
            }
            "is" => {
                let token_start = self.scanner.token_start();
                self.parse_error_at(
                    pos,
                    token_start,
                    diag::A_type_predicate_is_only_allowed_in_return_type_position_for_functions_and_methods,
                    args![],
                );
                return;
            }
            "module" | "namespace" => {
                self.parse_error_for_invalid_name(
                    diag::Namespace_name_cannot_be_0,
                    diag::Namespace_must_be_given_a_name,
                    SyntaxKind::OpenBraceToken,
                );
                return;
            }
            "type" => {
                self.parse_error_for_invalid_name(
                    diag::Type_alias_name_cannot_be_0,
                    diag::Type_alias_must_be_given_a_name,
                    SyntaxKind::EqualsToken,
                );
                return;
            }
            _ => {}
        }
        // The user alternatively might have misspelled or forgotten to add a space after a common keyword.
        let mut suggestion = get_spelling_suggestion_for_strings(
            expression_text,
            viable_keyword_suggestions().iter().cloned(),
        );
        if suggestion.is_empty() {
            suggestion = get_space_suggestion(expression_text);
        }
        if !suggestion.is_empty() {
            self.parse_error_at(
                pos,
                node.end(),
                diag::Unknown_keyword_or_identifier_Did_you_mean_0,
                args![suggestion],
            );
            return;
        }
        // Unknown tokens are handled with their own errors in the scanner
        if self.token == SyntaxKind::Unknown {
            return;
        }
        // Otherwise, we know this some kind of unknown word, not just a missing expected semicolon.
        self.parse_error_at(
            pos,
            node.end(),
            diag::Unexpected_keyword_or_identifier,
            args![],
        );
    }
}

// Go: parser/parser.go:2113 getSpaceSuggestion
pub fn get_space_suggestion(expression_text: &str) -> String {
    for keyword in viable_keyword_suggestions().iter() {
        if expression_text.len() > keyword.len() + 2
            && expression_text.starts_with(keyword.as_str())
        {
            return format!("{} {}", keyword, &expression_text[keyword.len()..]);
        }
    }
    String::new()
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:2122 parseErrorForInvalidName
    pub fn parse_error_for_invalid_name(
        &mut self,
        name_diagnostic: &'static Message,
        blank_diagnostic: &'static Message,
        token_if_blank_name: SyntaxKind,
    ) {
        if self.token == token_if_blank_name {
            self.parse_error_at_current_token(blank_diagnostic, args![]);
        } else {
            let token_value = self.scanner.token_value().to_string();
            self.parse_error_at_current_token(name_diagnostic, args![token_value]);
        }
    }

    // Go: parser/parser.go:2130 parseInterfaceDeclaration
    pub fn parse_interface_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected(SyntaxKind::InterfaceKeyword);
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        let heritage_clauses = self.parse_heritage_clauses(true /*isInterface*/);
        let members = self.parse_object_type_members();
        let node = self.factory.new_interface_declaration(
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            members,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2142 parseTypeAliasDeclaration
    pub fn parse_type_alias_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected(SyntaxKind::TypeKeyword);
        if self.has_preceding_line_break() {
            self.parse_error_at_current_token(diag::Line_break_not_permitted_here, args![]);
        }
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        self.parse_expected(SyntaxKind::EqualsToken);
        let type_node = if self.token == SyntaxKind::IntrinsicKeyword
            && self.look_ahead(Parser::next_is_not_dot)
        {
            self.parse_keyword_type_node()
        } else {
            self.parse_type()
        };
        self.parse_semicolon();
        let node =
            self.factory
                .new_type_alias_declaration(modifiers, name, type_parameters, type_node);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2163 nextIsNotDot
    pub fn next_is_not_dot(&mut self) -> bool {
        self.next_token() != SyntaxKind::DotToken
    }

    // Go: parser/parser.go:2171 parseEnumMember
    // In an ambient declaration, the grammar only allows integer literals as initializers.
    // In a non-ambient declaration, the grammar allows uninitialized members only in a
    // ConstantEnumMemberSection, which starts at the beginning of an enum declaration
    // or any time an integer literal initializer is encountered.
    pub fn parse_enum_member(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_property_name();
        let initializer = self.do_in_context(
            NodeFlags::DISALLOW_IN_CONTEXT,
            false,
            Parser::parse_initializer,
        );
        let node = self.factory.new_enum_member(name, initializer);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:2181 parseEnumDeclaration
    pub fn parse_enum_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.parse_expected(SyntaxKind::EnumKeyword);
        let name = self.parse_identifier();
        let members;
        if self.parse_expected(SyntaxKind::OpenBraceToken) {
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::YIELD_CONTEXT | NodeFlags::AWAIT_CONTEXT, false);
            members =
                self.parse_delimited_list(ParsingContext::EnumMembers, Parser::parse_enum_member);
            self.context_flags = save_context_flags;
            self.parse_expected(SyntaxKind::CloseBraceToken);
        } else {
            members = self.create_missing_list();
        }
        let node = self.factory.new_enum_declaration(modifiers, name, members);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // Go: parser/parser.go:2202 parseModuleDeclaration
    pub fn parse_module_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let mut keyword = SyntaxKind::ModuleKeyword;
        if self.token == SyntaxKind::GlobalKeyword {
            // global augmentation
            return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
        } else if self.parse_optional(SyntaxKind::NamespaceKeyword) {
            keyword = SyntaxKind::NamespaceKeyword;
        } else {
            self.parse_expected(SyntaxKind::ModuleKeyword);
            if self.token == SyntaxKind::StringLiteral {
                return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
            }
        }
        self.parse_module_or_namespace_declaration(
            pos, jsdoc, modifiers, false, /*nested*/
            keyword,
        )
    }

    // Go: parser/parser.go:2218 parseAmbientExternalModuleDeclaration
    pub fn parse_ambient_external_module_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let name;
        let mut keyword = SyntaxKind::ModuleKeyword;
        let save_has_await_identifier = self.statement_has_await_identifier;
        if self.token == SyntaxKind::GlobalKeyword {
            // parse 'global' as name of global scope augmentation
            name = self.parse_identifier();
            keyword = SyntaxKind::GlobalKeyword;
        } else {
            // parse string literal
            name = self.parse_literal_expression();
        }
        let mut attributes = Node::NIL;
        if keyword == SyntaxKind::ModuleKeyword && self.parse_optional(SyntaxKind::WithKeyword) {
            attributes = self.parse_type_literal();
        }
        let mut body = Node::NIL;
        if self.token == SyntaxKind::OpenBraceToken {
            body = self.parse_module_block();
        } else {
            self.parse_semicolon();
        }
        let node = self
            .factory
            .new_module_declaration(modifiers, keyword, name, attributes, body);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // Go: parser/parser.go:2246 parseModuleBlock
    pub fn parse_module_block(&mut self) -> Node {
        let pos = self.node_pos();
        let statements;
        if self.parse_expected(SyntaxKind::OpenBraceToken) {
            statements = self.parse_list(ParsingContext::BlockStatements, Parser::parse_statement);
            self.parse_expected(SyntaxKind::CloseBraceToken);
        } else {
            statements = self.create_missing_list();
        }
        let node = self.factory.new_module_block(statements);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2258 parseModuleOrNamespaceDeclaration
    pub fn parse_module_or_namespace_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        nested: bool,
        keyword: SyntaxKind,
    ) -> Node {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let name = if nested {
            self.parse_identifier_name()
        } else {
            self.parse_identifier()
        };
        let body;
        if self.parse_optional(SyntaxKind::DotToken) {
            let implicit_export = self.factory.new_modifier(SyntaxKind::ExportKeyword);
            let node_pos = self.node_pos();
            set_node_loc(implicit_export, TextRange::new(node_pos, node_pos));
            set_node_flags(implicit_export, NodeFlags::REPARSED);
            // PORT: Go `p.nodeSliceArena.NewSlice1(implicitExport)` is a one-element slice.
            let implicit_modifiers =
                self.new_modifier_list(implicit_export.loc(), &[implicit_export]);
            let inner_pos = self.node_pos();
            body = self.parse_module_or_namespace_declaration(
                inner_pos,
                JsdocScannerInfo::default(), /*jsdoc*/
                implicit_modifiers,
                true, /*nested*/
                keyword,
            );
        } else {
            body = self.parse_module_block();
        }
        let node = self
            .factory
            .new_module_declaration(modifiers, keyword, name, Node::NIL, body);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // Go: parser/parser.go:2282 parseImportDeclarationOrImportEqualsDeclaration
    pub fn parse_import_declaration_or_import_equals_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected(SyntaxKind::ImportKeyword);
        let after_import_pos = self.node_pos();
        // We don't parse the identifier here in await context, instead we will report a grammar error in the checker.
        let save_has_await_identifier = self.statement_has_await_identifier;
        let phase_modifier_candidate = self.current_import_phase_modifier();
        let mut identifier = Node::NIL;
        if self.is_identifier() {
            identifier = self.parse_identifier();
        }
        let mut phase_modifier = SyntaxKind::Unknown;
        if identifier.is_some()
            && identifier.text() == "type"
            && (self.token != SyntaxKind::FromKeyword
                || self.is_identifier()
                    && self.look_ahead(Parser::next_token_is_from_keyword_or_equals_token))
            && (self.is_identifier()
                || self.token_after_import_definitely_produces_import_declaration())
        {
            phase_modifier = SyntaxKind::TypeKeyword;
            identifier = Node::NIL;
            if self.is_identifier() {
                identifier = self.parse_identifier();
            }
        } else if identifier.is_some()
            && phase_modifier_candidate != SyntaxKind::Unknown
            && self.should_parse_import_phase_modifier()
        {
            phase_modifier = phase_modifier_candidate;
            identifier = Node::NIL;
            if self.is_identifier() {
                identifier = self.parse_identifier();
            }
        }
        if identifier.is_some()
            && self.token_after_imported_identifier_allows_import_equals_declaration()
            && phase_modifier != SyntaxKind::DeferKeyword
            && phase_modifier != SyntaxKind::SourceKeyword
        {
            let node = self.parse_import_equals_declaration(
                pos,
                jsdoc,
                modifiers,
                identifier,
                phase_modifier == SyntaxKind::TypeKeyword,
            );
            let import_equals = self.check_js_syntax(node);
            self.statement_has_await_identifier = save_has_await_identifier; // Import= declaration is always parsed in an Await context, no need to reparse
            return import_equals;
        }
        let import_clause = self.try_parse_import_clause(
            identifier,
            after_import_pos,
            phase_modifier,
            false, /*skipJSDocLeadingAsterisks*/
        );
        self.statement_has_await_identifier = save_has_await_identifier; // import clause is always parsed in an Await context
        let module_specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();
        self.parse_semicolon();
        let node = self.factory.new_import_declaration(
            modifiers,
            import_clause,
            module_specifier,
            attributes,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2324 nextTokenIsFromKeywordOrEqualsToken
    pub fn next_token_is_from_keyword_or_equals_token(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::FromKeyword || self.token == SyntaxKind::EqualsToken
    }

    // Go: parser/parser.go:2329 shouldParseImportPhaseModifier (ts#63915)
    pub fn should_parse_import_phase_modifier(&mut self) -> bool {
        match self.token {
            SyntaxKind::CommaToken | SyntaxKind::EqualsToken => return false,
            SyntaxKind::FromKeyword => {
                if self.look_ahead(Parser::next_token_is_token_string_literal) {
                    return false;
                }
            }
            _ => {}
        }
        true
    }

    // Go: parser/parser.go:2341 currentImportPhaseModifier (ts#63915)
    // The token text is the raw text, so an escaped `d\u0065fer` or
    // `s\u006furce` is not a phase modifier.
    pub fn current_import_phase_modifier(&self) -> SyntaxKind {
        match self.scanner.token_text() {
            "defer" => SyntaxKind::DeferKeyword,
            "source" => SyntaxKind::SourceKeyword,
            _ => SyntaxKind::Unknown,
        }
    }

    // Go: parser/parser.go:2352 tokenAfterImportDefinitelyProducesImportDeclaration
    pub fn token_after_import_definitely_produces_import_declaration(&self) -> bool {
        self.token == SyntaxKind::AsteriskToken || self.token == SyntaxKind::OpenBraceToken
    }

    // Go: parser/parser.go:2356 tokenAfterImportedIdentifierAllowsImportEqualsDeclaration (ts#63915; was tokenAfterImportedIdentifierDefinitelyProducesImportDeclaration)
    pub fn token_after_imported_identifier_allows_import_equals_declaration(&self) -> bool {
        !matches!(self.token, SyntaxKind::CommaToken | SyntaxKind::FromKeyword)
    }

    // Go: parser/parser.go:2347 parseImportEqualsDeclaration
    pub fn parse_import_equals_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        identifier: Node,
        is_type_only: bool,
    ) -> Node {
        self.parse_expected(SyntaxKind::EqualsToken);
        let module_reference = self.parse_module_reference();
        self.parse_semicolon();
        let node = self.factory.new_import_equals_declaration(
            modifiers,
            is_type_only,
            identifier,
            module_reference,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:2356 parseModuleReference
    pub fn parse_module_reference(&mut self) -> Node {
        if self.token == SyntaxKind::RequireKeyword
            && self.look_ahead(Parser::next_token_is_open_paren)
        {
            return self.parse_external_module_reference();
        }
        self.parse_entity_name(
            false, /*allowReservedWords*/
            false, /*allowPrivateName*/
            None,  /*diagnosticMessage*/
        )
    }

    // Go: parser/parser.go:2363 parseExternalModuleReference
    pub fn parse_external_module_reference(&mut self) -> Node {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::RequireKeyword);
        self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_module_specifier();
        self.parse_expected(SyntaxKind::CloseParenToken);
        let node = self.factory.new_external_module_reference(expression);
        let result = self.finish_node(node, pos);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // Go: parser/parser.go:2375 parseModuleSpecifier
    pub fn parse_module_specifier(&mut self) -> Node {
        if self.token == SyntaxKind::StringLiteral {
            return self.parse_literal_expression();
        }
        // We allow arbitrary expressions here, even though the grammar only allows string
        // literals.  We check to ensure that it is only a string literal later in the grammar
        // check pass.
        self.parse_expression()
    }

    // Go: parser/parser.go:2403 tryParseImportClause
    pub fn try_parse_import_clause(
        &mut self,
        identifier: Node,
        pos: i32,
        phase_modifier: SyntaxKind,
        skip_js_doc_leading_asterisks: bool,
    ) -> Node {
        // ImportDeclaration:
        //  import ImportClause from ModuleSpecifier ;
        //  import ModuleSpecifier;
        if identifier.is_some()
            || self.token == SyntaxKind::AsteriskToken
            || self.token == SyntaxKind::OpenBraceToken
        {
            let import_clause = self.parse_import_clause(
                identifier,
                pos,
                phase_modifier,
                skip_js_doc_leading_asterisks,
            );
            self.parse_expected(SyntaxKind::FromKeyword);
            return import_clause;
        }
        if phase_modifier == SyntaxKind::DeferKeyword || phase_modifier == SyntaxKind::SourceKeyword
        {
            let node = self.factory.new_import_clause(
                phase_modifier,
                Node::NIL, /*name*/
                Node::NIL, /*namedBindings*/
            );
            return self.finish_node(node, pos);
        }
        Node::NIL
    }

    // Go: parser/parser.go:2397 parseImportClause
    pub fn parse_import_clause(
        &mut self,
        identifier: Node,
        pos: i32,
        phase_modifier: SyntaxKind,
        skip_js_doc_leading_asterisks: bool,
    ) -> Node {
        // ImportClause:
        //  ImportedDefaultBinding
        //  NameSpaceImport
        //  NamedImports
        //  ImportedDefaultBinding, NameSpaceImport
        //  ImportedDefaultBinding, NamedImports
        // If there was no default import or if there is comma token after default import
        // parse namespace or named imports
        let mut named_bindings = Node::NIL;
        let save_has_await_identifier = self.statement_has_await_identifier;
        if identifier.is_nil() || self.parse_optional(SyntaxKind::CommaToken) {
            if skip_js_doc_leading_asterisks {
                self.scanner.set_skip_js_doc_leading_asterisks(true);
            }
            if self.token == SyntaxKind::AsteriskToken {
                named_bindings = self.parse_namespace_import();
            } else {
                named_bindings = self.parse_named_imports();
            }
            if skip_js_doc_leading_asterisks {
                self.scanner.set_skip_js_doc_leading_asterisks(false);
            }
        }
        let node = self
            .factory
            .new_import_clause(phase_modifier, identifier, named_bindings);
        let result = self.finish_node(node, pos);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // Go: parser/parser.go:2426 parseNamespaceImport
    pub fn parse_namespace_import(&mut self) -> Node {
        // NameSpaceImport:
        //  * as ImportedBinding
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::AsteriskToken);
        self.parse_expected(SyntaxKind::AsKeyword);
        let name = self.parse_identifier();
        let node = self.factory.new_namespace_import(name);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2436 parseNamedImports
    pub fn parse_named_imports(&mut self) -> Node {
        let pos = self.node_pos();
        // NamedImports:
        //  { }
        //  { ImportsList }
        //  { ImportsList, }
        let imports = self.parse_bracketed_list(
            ParsingContext::ImportOrExportSpecifiers,
            Parser::parse_import_specifier,
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
        );
        let node = self.factory.new_named_imports(imports);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2446 parseImportSpecifier
    pub fn parse_import_specifier(&mut self) -> Node {
        let pos = self.node_pos();
        let (is_type_only, property_name, name) =
            self.parse_import_or_export_specifier(SyntaxKind::ImportSpecifier);
        let identifier_name;
        if name.kind() == SyntaxKind::Identifier {
            identifier_name = name;
        } else {
            let range = self.skip_range_trivia(name.loc());
            self.parse_error_at_range(range, diag::Identifier_expected, args![]);
            identifier_name = self.new_identifier("");
            self.finish_node(identifier_name, name.pos());
        }
        let node = self
            .factory
            .new_import_specifier(is_type_only, property_name, identifier_name);
        let node = self.finish_node(node, pos);
        self.check_js_syntax(node)
    }

    // Go: parser/parser.go:2461 parseImportOrExportSpecifier
    pub fn parse_import_or_export_specifier(&mut self, kind: SyntaxKind) -> (bool, Node, Node) {
        // ImportSpecifier:
        //   BindingIdentifier
        //   ModuleExportName as BindingIdentifier
        // ExportSpecifier:
        //   ModuleExportName
        //   ModuleExportName as ModuleExportName
        // let checkIdentifierIsKeyword = isKeyword(token()) && !isIdentifier();
        // let checkIdentifierStart = scanner.getTokenStart();
        // let checkIdentifierEnd = scanner.getTokenEnd();
        let mut is_type_only = false;
        let mut property_name = Node::NIL;
        let mut can_parse_as_keyword = true;
        let disallow_keywords = kind == SyntaxKind::ImportSpecifier;
        let (mut name, mut name_ok) = self.parse_module_export_name(disallow_keywords);
        if name.kind() == SyntaxKind::Identifier && name.text() == "type" {
            // If the first token of an import specifier is 'type', there are a lot of possibilities,
            // especially if we see 'as' afterwards:
            //
            // import { type } from "mod";          - isTypeOnly: false,   name: type
            // import { type as } from "mod";       - isTypeOnly: true,    name: as
            // import { type as as } from "mod";    - isTypeOnly: false,   name: as,    propertyName: type
            // import { type as as as } from "mod"; - isTypeOnly: true,    name: as,    propertyName: as
            if self.token == SyntaxKind::AsKeyword {
                // { type as ...? }
                let first_as = self.parse_identifier_name();
                if self.token == SyntaxKind::AsKeyword {
                    // { type as as ...? }
                    let second_as = self.parse_identifier_name();
                    if self.can_parse_module_export_name() {
                        // { type as as something }
                        // { type as as "something" }
                        is_type_only = true;
                        property_name = first_as;
                        (name, name_ok) = self.parse_module_export_name(disallow_keywords);
                        can_parse_as_keyword = false;
                    } else {
                        // { type as as }
                        property_name = name;
                        name = second_as;
                        can_parse_as_keyword = false;
                    }
                } else if self.can_parse_module_export_name() {
                    // { type as something }
                    // { type as "something" }
                    property_name = name;
                    can_parse_as_keyword = false;
                    (name, name_ok) = self.parse_module_export_name(disallow_keywords);
                } else {
                    // { type as }
                    is_type_only = true;
                    name = first_as;
                }
            } else if self.can_parse_module_export_name() {
                // { type something ...? }
                // { type "something" ...? }
                is_type_only = true;
                (name, name_ok) = self.parse_module_export_name(disallow_keywords);
            }
        }
        if can_parse_as_keyword && self.token == SyntaxKind::AsKeyword {
            property_name = name;
            self.parse_expected(SyntaxKind::AsKeyword);
            (name, name_ok) = self.parse_module_export_name(disallow_keywords);
        }

        if !name_ok {
            let range = self.skip_range_trivia(name.loc());
            self.parse_error_at_range(range, diag::Identifier_expected, args![]);
        }

        (is_type_only, property_name, name)
    }

    // Go: parser/parser.go:2533 canParseModuleExportName
    pub fn can_parse_module_export_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == SyntaxKind::StringLiteral
    }

    // Go: parser/parser.go:2537 parseModuleExportName
    pub fn parse_module_export_name(&mut self, disallow_keywords: bool) -> (Node, bool) {
        let mut name_ok = true;

        if self.token == SyntaxKind::StringLiteral {
            return (self.parse_literal_expression(), name_ok);
        }
        if disallow_keywords && is_keyword(self.token) && !self.is_identifier() {
            name_ok = false;
        }
        (self.parse_identifier_name(), name_ok)
    }

    // Go: parser/parser.go:2549 tryParseImportAttributes
    pub fn try_parse_import_attributes(&mut self) -> Node {
        if self.token == SyntaxKind::WithKeyword
            || (self.token == SyntaxKind::AssertKeyword && !self.has_preceding_line_break())
        {
            if self.token == SyntaxKind::AssertKeyword {
                self.parse_error_at_current_token(
                    diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert,
                    args![],
                );
            }
            let token = self.token;
            return self.parse_import_attributes(token, false /*skipKeyword*/);
        }
        Node::NIL
    }

    // Go: parser/parser.go:2559 parseExportAssignment
    pub fn parse_export_assignment(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let mut is_export_equals = false;
        if self.parse_optional(SyntaxKind::EqualsToken) {
            is_export_equals = true;
        } else {
            self.parse_expected(SyntaxKind::DefaultKeyword);
        }
        let expression = self.parse_assignment_expression_or_higher();
        self.parse_semicolon();
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        let node = self.factory.new_export_assignment(
            modifiers,
            is_export_equals,
            Node::NIL, /*typeNode*/
            expression,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2579 parseNamespaceExportDeclaration
    pub fn parse_namespace_export_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        self.parse_expected(SyntaxKind::AsKeyword);
        self.parse_expected(SyntaxKind::NamespaceKeyword);
        let save_has_await_identifier = self.statement_has_await_identifier;
        let name = self.parse_identifier();
        self.statement_has_await_identifier = save_has_await_identifier;
        self.parse_semicolon();
        // NamespaceExportDeclaration nodes cannot have decorators or modifiers, we attach them here so we can report them in the grammar checker
        let node = self
            .factory
            .new_namespace_export_declaration(modifiers, name);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:2592 parseExportDeclaration
    pub fn parse_export_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let mut export_clause = Node::NIL;
        let mut module_specifier = Node::NIL;
        let mut attributes = Node::NIL;
        let is_type_only = self.parse_optional(SyntaxKind::TypeKeyword);
        let namespace_export_pos = self.node_pos();
        if self.parse_optional(SyntaxKind::AsteriskToken) {
            if self.parse_optional(SyntaxKind::AsKeyword) {
                export_clause = self.parse_namespace_export(namespace_export_pos);
            }
            self.parse_expected(SyntaxKind::FromKeyword);
            module_specifier = self.parse_module_specifier();
        } else {
            export_clause = self.parse_named_exports();
            // It is not uncommon to accidentally omit the 'from' keyword. Additionally, in editing scenarios,
            // the 'from' keyword can be parsed as a named export when the export clause is unterminated (i.e. `export { from "moduleName";`)
            // If we don't have a 'from' keyword, see if we have a string literal such that ASI won't take effect.
            if self.token == SyntaxKind::FromKeyword
                || (self.token == SyntaxKind::StringLiteral && !self.has_preceding_line_break())
            {
                self.parse_expected(SyntaxKind::FromKeyword);
                module_specifier = self.parse_module_specifier();
            }
        }
        if module_specifier.is_some()
            && (self.token == SyntaxKind::WithKeyword || self.token == SyntaxKind::AssertKeyword)
            && !self.has_preceding_line_break()
        {
            if self.token == SyntaxKind::AssertKeyword {
                self.parse_error_at_current_token(
                    diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert,
                    args![],
                );
            }
            let token = self.token;
            attributes = self.parse_import_attributes(token, false /*skipKeyword*/);
        }
        self.parse_semicolon();
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        let node = self.factory.new_export_declaration(
            modifiers,
            is_type_only,
            export_clause,
            module_specifier,
            attributes,
        );
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // Go: parser/parser.go:2632 parseNamespaceExport
    pub fn parse_namespace_export(&mut self, pos: i32) -> Node {
        let (export_name, _) = self.parse_module_export_name(false /*disallowKeywords*/);
        let node = self.factory.new_namespace_export(export_name);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2637 parseNamedExports
    pub fn parse_named_exports(&mut self) -> Node {
        let pos = self.node_pos();
        // NamedImports:
        //  { }
        //  { ImportsList }
        //  { ImportsList, }
        let exports = self.parse_bracketed_list(
            ParsingContext::ImportOrExportSpecifiers,
            Parser::parse_export_specifier,
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
        );
        let node = self.factory.new_named_exports(exports);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2647 parseExportSpecifier
    pub fn parse_export_specifier(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let (is_type_only, property_name, name) =
            self.parse_import_or_export_specifier(SyntaxKind::ExportSpecifier);
        let node = self
            .factory
            .new_export_specifier(is_type_only, property_name, name);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    // TYPES

    // Go: parser/parser.go:2659 parseType
    pub fn parse_type(&mut self) -> Node {
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::TYPE_EXCLUDES_FLAGS, false);
        let mut type_node;
        if self.is_start_of_function_type_or_constructor_type() {
            type_node = self.parse_function_or_constructor_type();
        } else {
            let pos = self.node_pos();
            type_node = self.parse_union_type_or_higher();
            if !self.in_disallow_conditional_types_context()
                && !self.has_preceding_line_break()
                && self.parse_optional(SyntaxKind::ExtendsKeyword)
            {
                // The type following 'extends' is not permitted to be another conditional type
                let extends_type = self.do_in_context(
                    NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    true,
                    Parser::parse_type,
                );
                self.parse_expected(SyntaxKind::QuestionToken);
                let true_type = self.do_in_context(
                    NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    false,
                    Parser::parse_type,
                );
                self.parse_expected(SyntaxKind::ColonToken);
                let false_type = self.do_in_context(
                    NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                    false,
                    Parser::parse_type,
                );
                let conditional_type = self.factory.new_conditional_type_node(
                    type_node,
                    extends_type,
                    true_type,
                    false_type,
                );
                self.finish_node(conditional_type, pos);
                type_node = conditional_type;
            }
        }
        self.context_flags = save_context_flags;
        type_node
    }

    // Go: parser/parser.go:2684 parseUnionTypeOrHigher
    pub fn parse_union_type_or_higher(&mut self) -> Node {
        self.parse_union_or_intersection_type(
            SyntaxKind::BarToken,
            Parser::parse_intersection_type_or_higher,
        )
    }

    // Go: parser/parser.go:2688 parseIntersectionTypeOrHigher
    pub fn parse_intersection_type_or_higher(&mut self) -> Node {
        self.parse_union_or_intersection_type(
            SyntaxKind::AmpersandToken,
            Parser::parse_type_operator_or_higher,
        )
    }

    // Go: parser/parser.go:2692 parseUnionOrIntersectionType
    pub fn parse_union_or_intersection_type(
        &mut self,
        operator: SyntaxKind,
        parse_constituent_type: fn(&mut Parser<'a>) -> Node,
    ) -> Node {
        let pos = self.node_pos();
        let is_union_type = operator == SyntaxKind::BarToken;
        let has_leading_operator = self.parse_optional(operator);
        let mut type_node = if has_leading_operator {
            self.parse_function_or_constructor_type_to_error(is_union_type, parse_constituent_type)
        } else {
            parse_constituent_type(self)
        };
        if self.token == operator || has_leading_operator {
            let mut types: Vec<Node> = Vec::with_capacity(8);
            types.push(type_node);
            while self.parse_optional(operator) {
                let t = self.parse_function_or_constructor_type_to_error(
                    is_union_type,
                    parse_constituent_type,
                );
                types.push(t);
            }
            // PORT: Go `p.nodeSliceArena.Clone(types)` copies into the arena; the list copies the slice.
            let end = self.node_pos();
            let list = self.new_node_list(TextRange::new(pos, end), &types);
            type_node = self.create_union_or_intersection_type_node(operator, list);
            self.finish_node(type_node, pos);
        }
        type_node
    }

    // Go: parser/parser.go:2714 createUnionOrIntersectionTypeNode
    pub fn create_union_or_intersection_type_node(
        &mut self,
        operator: SyntaxKind,
        types: NodeList,
    ) -> Node {
        match operator {
            SyntaxKind::BarToken => self.factory.new_union_type_node(types),
            SyntaxKind::AmpersandToken => self.factory.new_intersection_type_node(types),
            _ => panic!("Unhandled case in createUnionOrIntersectionType"),
        }
    }

    // Go: parser/parser.go:2725 parseTypeOperatorOrHigher
    pub fn parse_type_operator_or_higher(&mut self) -> Node {
        let operator = self.token;
        match operator {
            SyntaxKind::KeyOfKeyword | SyntaxKind::UniqueKeyword | SyntaxKind::ReadonlyKeyword => {
                return self.parse_type_operator(operator);
            }
            SyntaxKind::InferKeyword => {
                return self.parse_infer_type();
            }
            _ => {}
        }
        self.do_in_context(
            NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
            false,
            Parser::parse_postfix_type_or_higher,
        )
    }

    // Go: parser/parser.go:2736 parseTypeOperator
    pub fn parse_type_operator(&mut self, operator: SyntaxKind) -> Node {
        let pos = self.node_pos();
        self.parse_expected(operator);
        let type_node = self.parse_type_operator_or_higher();
        let node = self.factory.new_type_operator_node(operator, type_node);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2742 parseInferType
    pub fn parse_infer_type(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::InferKeyword);
        let type_parameter = self.parse_type_parameter_of_infer_type();
        let node = self.factory.new_infer_type_node(type_parameter);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2748 parseTypeParameterOfInferType
    pub fn parse_type_parameter_of_infer_type(&mut self) -> Node {
        let pos = self.node_pos();
        let name = self.parse_identifier();
        let constraint = self.try_parse_constraint_of_infer_type();
        let node = self.factory.new_type_parameter_declaration(
            ModifierList::NIL, /*modifiers*/
            name,
            constraint,
            Node::NIL, /*expression*/
            Node::NIL, /*defaultType*/
        );
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:2755 tryParseConstraintOfInferType
    pub fn try_parse_constraint_of_infer_type(&mut self) -> Node {
        let state = self.mark();
        if self.parse_optional(SyntaxKind::ExtendsKeyword) {
            let constraint = self.do_in_context(
                NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                true,
                Parser::parse_type,
            );
            if self.in_disallow_conditional_types_context()
                || self.token != SyntaxKind::QuestionToken
            {
                return constraint;
            }
        }
        self.rewind(state);
        Node::NIL
    }

    // Go: parser/parser.go:2767 parsePostfixTypeOrHigher
    pub fn parse_postfix_type_or_higher(&mut self) -> Node {
        let pos = self.node_pos();
        let mut type_node = self.parse_non_array_type();
        while !self.has_preceding_line_break() {
            match self.token {
                SyntaxKind::ExclamationToken => {
                    self.next_token();
                    let node = self.factory.new_js_doc_non_nullable_type(type_node);
                    type_node = self.finish_node(node, pos);
                }
                SyntaxKind::QuestionToken => {
                    // If next token is start of a type we have a conditional type
                    if self.look_ahead(Parser::next_is_start_of_type) {
                        return type_node;
                    }
                    self.next_token();
                    let node = self.factory.new_js_doc_nullable_type(type_node);
                    type_node = self.finish_node(node, pos);
                }
                SyntaxKind::OpenBracketToken => {
                    self.parse_expected(SyntaxKind::OpenBracketToken);
                    if self.is_start_of_type(false /*isStartOfParameter*/) {
                        let index_type = self.parse_type();
                        self.parse_expected(SyntaxKind::CloseBracketToken);
                        let node = self
                            .factory
                            .new_indexed_access_type_node(type_node, index_type);
                        type_node = self.finish_node(node, pos);
                    } else {
                        self.parse_expected(SyntaxKind::CloseBracketToken);
                        let node = self.factory.new_array_type_node(type_node);
                        type_node = self.finish_node(node, pos);
                    }
                }
                _ => return type_node,
            }
        }
        type_node
    }

    // Go: parser/parser.go:2799 nextIsStartOfType
    pub fn next_is_start_of_type(&mut self) -> bool {
        self.next_token();
        self.is_start_of_type(false /*inStartOfParameter*/)
    }

    // Go: parser/parser.go:2804 parseNonArrayType
    pub fn parse_non_array_type(&mut self) -> Node {
        match self.token {
            SyntaxKind::AnyKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::BigIntKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::UndefinedKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::ObjectKeyword => {
                let state = self.mark();
                let keyword_type_node = self.parse_keyword_type_node();
                // If these are followed by a dot then parse these out as a dotted type reference instead
                if self.token != SyntaxKind::DotToken {
                    return keyword_type_node;
                }
                self.rewind(state);
                self.parse_type_reference()
            }
            SyntaxKind::AsteriskEqualsToken => {
                // If there is '*=', treat it as * followed by postfix =
                self.scanner.re_scan_asterisk_equals_token();
                // PORT: Go `fallthrough` into the AsteriskToken case.
                self.parse_js_doc_all_type()
            }
            SyntaxKind::AsteriskToken => self.parse_js_doc_all_type(),
            SyntaxKind::QuestionQuestionToken => {
                // If there is '??', treat it as prefix-'?' in JSDoc type.
                self.scanner.re_scan_question_token();
                // PORT: Go `fallthrough` into the QuestionToken case.
                self.parse_js_doc_nullable_type()
            }
            SyntaxKind::QuestionToken => self.parse_js_doc_nullable_type(),
            SyntaxKind::ExclamationToken => self.parse_js_doc_non_nullable_type(),
            SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NullKeyword => self.parse_literal_type_node(false /*negative*/),
            SyntaxKind::MinusToken => {
                if self.look_ahead(Parser::next_token_is_numeric_or_big_int_literal) {
                    return self.parse_literal_type_node(true /*negative*/);
                }
                self.parse_type_reference()
            }
            SyntaxKind::VoidKeyword => self.parse_keyword_type_node(),
            SyntaxKind::ThisKeyword => {
                let this_keyword = self.parse_this_type_node();
                if self.token == SyntaxKind::IsKeyword && !self.has_preceding_line_break() {
                    return self.parse_this_type_predicate(this_keyword);
                }
                this_keyword
            }
            SyntaxKind::TypeOfKeyword => {
                if self.look_ahead(Parser::next_is_start_of_type_of_import_type) {
                    return self.parse_import_type();
                }
                self.parse_type_query()
            }
            SyntaxKind::OpenBraceToken => {
                if self.look_ahead(Parser::next_is_start_of_mapped_type) {
                    return self.parse_mapped_type();
                }
                self.parse_type_literal()
            }
            SyntaxKind::OpenBracketToken => self.parse_tuple_type(),
            SyntaxKind::OpenParenToken => self.parse_parenthesized_type(),
            SyntaxKind::ImportKeyword => self.parse_import_type(),
            SyntaxKind::AssertsKeyword => {
                if self.look_ahead(Parser::next_token_is_identifier_or_keyword_on_same_line) {
                    return self.parse_asserts_type_predicate();
                }
                self.parse_type_reference()
            }
            SyntaxKind::TemplateHead => self.parse_template_type(),
            _ => self.parse_type_reference(),
        }
    }
}
