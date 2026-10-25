//! Port of Go `parser/parser.go` lines 2827 to 4183 (unit U6): type nodes,
//! type members, signatures, parameters, modifiers and the start of the
//! expression parser.
//!
//! Callback rules (contract 4.5): Go `func(p *Parser) T` arguments are passed
//! as `&mut Self::method` or `&mut |p: &mut Parser| ...`. That form works for
//! both `impl FnMut`/`impl FnOnce` generics and `&mut dyn FnMut` parameters.
//!
//! Borrowing: Go evaluates call arguments left to right. The Rust code parses
//! children into locals first, in the same order, and then calls the factory,
//! because `self.factory` cannot stay borrowed while a child parse runs.

use crate::frontend::prelude::*;
use smallvec::SmallVec;

// PORT: Go reads `p.diagnostics` directly. The parser and the scanner share
// the diagnostics through `Rc<RefCell<ParseDiagnostics>>` (contract 4.5).
// These two helpers are the only places in this file that touch that store.
fn diagnostics_len(p: &Parser<'_>) -> usize {
    p.diagnostics.borrow().diagnostics.len()
}

// PORT: Go repeats this block in parseImportType and parseImportAttributes:
//
//	if len(p.diagnostics) != 0 {
//		lastDiagnostic := p.diagnostics[len(p.diagnostics)-1]
//		if lastDiagnostic.Code() == diagnostics.X_0_expected.Code() {
//			related := ast.NewDiagnostic(nil, core.NewTextRange(openBracePosition, openBracePosition), diagnostics.The_parser_expected_to_find_a_1_to_match_the_0_token_here, "{", "}")
//			lastDiagnostic.AddRelatedInfo(related)
//		}
//	}
//
// Both copies call this helper. Go changes the diagnostic through its pointer;
// Rust changes the stored diagnostic in place.
fn add_related_brace_info_to_last_diagnostic(p: &Parser<'_>, open_brace_position: i32) {
    let mut sink = p.diagnostics.borrow_mut();
    if let Some(last_diagnostic) = sink.diagnostics.last_mut() {
        if last_diagnostic.code == diag::X_0_expected.code() as i32 {
            let related = new_diagnostic(
                Node::NIL,
                TextRange::new(open_brace_position, open_brace_position),
                diag::The_parser_expected_to_find_a_1_to_match_the_0_token_here,
                args!["{", "}"],
            );
            last_diagnostic.add_related_info(Some(related));
        }
    }
}

impl<'a> Parser<'a> {
    // Go: parser/parser.go:2874 parseKeywordTypeNode
    pub fn parse_keyword_type_node(&mut self) -> Node {
        let pos = self.node_pos();
        let result = self.factory.new_keyword_type_node(self.token);
        self.next_token();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2881 parseThisTypeNode
    pub fn parse_this_type_node(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let result = self.factory.new_this_type_node();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2887 parseThisTypePredicate
    pub fn parse_this_type_predicate(&mut self, lhs: Node) -> Node {
        self.next_token();
        let type_node = self.parse_type();
        let result = self.factory.new_type_predicate_node(
            Node::NIL, /*assertsModifier*/
            lhs,
            type_node,
        );
        self.finish_node(result, lhs.pos())
    }

    // Go: parser/parser.go:2892 parseJSDocAllType
    pub fn parse_js_doc_all_type(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let result = self.factory.new_js_doc_all_type();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2898 parseJSDocNonNullableType
    pub fn parse_js_doc_non_nullable_type(&mut self) -> Node {
        let pos = self.node_pos();
        self.next_token();
        let type_node = self.parse_type_operator_or_higher();
        let result = self.factory.new_js_doc_non_nullable_type(type_node);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2904 parseJSDocNullableType
    pub fn parse_js_doc_nullable_type(&mut self) -> Node {
        let pos = self.node_pos();
        // skip the ?
        self.next_token();
        let type_node = self.parse_type_operator_or_higher();
        let result = self.factory.new_js_doc_nullable_type(type_node);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2911 parseJSDocType
    pub fn parse_js_doc_type(&mut self) -> Node {
        self.scanner.set_skip_js_doc_leading_asterisks(true);
        let pos = self.node_pos();

        let has_dot_dot_dot = self.parse_optional(SyntaxKind::DotDotDotToken);
        let mut t = self.parse_type_or_type_predicate();
        self.scanner.set_skip_js_doc_leading_asterisks(false);
        if has_dot_dot_dot {
            let variadic = self.factory.new_js_doc_variadic_type(t);
            t = self.finish_node(variadic, pos);
        }
        if self.token == SyntaxKind::EqualsToken {
            self.next_token();
            let optional = self.factory.new_js_doc_optional_type(t);
            return self.finish_node(optional, pos);
        }
        t
    }

    // Go: parser/parser.go:2928 parseLiteralTypeNode
    pub fn parse_literal_type_node(&mut self, negative: bool) -> Node {
        let pos = self.node_pos();
        if negative {
            self.next_token();
        }
        let mut expression = if self.token == SyntaxKind::TrueKeyword
            || self.token == SyntaxKind::FalseKeyword
            || self.token == SyntaxKind::NullKeyword
        {
            self.parse_keyword_expression()
        } else {
            self.parse_literal_expression()
        };
        if negative {
            let prefix = self
                .factory
                .new_prefix_unary_expression(SyntaxKind::MinusToken, expression);
            expression = self.finish_node(prefix, pos);
        }
        let result = self.factory.new_literal_type_node(expression);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2945 parseTypeReference
    pub fn parse_type_reference(&mut self) -> Node {
        let pos = self.node_pos();
        let type_name = self.parse_entity_name_of_type_reference();
        let type_arguments = self.parse_type_arguments_of_type_reference();
        let result = self
            .factory
            .new_type_reference_node(type_name, type_arguments);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:2950 parseEntityNameOfTypeReference
    pub fn parse_entity_name_of_type_reference(&mut self) -> Node {
        self.parse_entity_name(
            true,  /*allowReservedWords*/
            false, /*allowPrivateName*/
            Some(diag::Type_expected),
        )
    }

    // Go: parser/parser.go:2954 parseEntityName
    // PORT: Go `diagnosticMessage` can be nil; it is `Option<&'static Message>`.
    pub fn parse_entity_name(
        &mut self,
        allow_reserved_words: bool,
        allow_private_name: bool,
        diagnostic_message: Option<&'static crate::diagnostics::Message>,
    ) -> Node {
        let pos = self.node_pos();
        let mut entity = if allow_reserved_words {
            self.parse_identifier_name_with_diagnostic(diagnostic_message)
        } else {
            self.parse_identifier_with_diagnostic(diagnostic_message, None)
        };
        while self.parse_optional(SyntaxKind::DotToken) {
            if self.token == SyntaxKind::LessThanToken {
                // The entity is part of a JSDoc-style generic. We will use the gap between `typeName` and
                // `typeArguments` to report it as a grammar error in the checker.
                break;
            }
            let right = self.parse_right_side_of_dot(
                allow_reserved_words,
                allow_private_name,
                true, /*allowUnicodeEscapeSequenceInIdentifierName*/
            );
            let qualified = self.factory.new_qualified_name(entity, right);
            entity = self.finish_node(qualified, pos);
        }
        entity
    }

    // Go: parser/parser.go:2973 parseRightSideOfDot
    pub fn parse_right_side_of_dot(
        &mut self,
        allow_identifier_names: bool,
        allow_private_identifiers: bool,
        allow_unicode_escape_sequence_in_identifier_name: bool,
    ) -> Node {
        // Technically a keyword is valid here as all identifiers and keywords are identifier names.
        // However, often we'll encounter this in error situations when the identifier or keyword
        // is actually starting another valid construct.
        //
        // So, we check for the following specific case:
        //
        //      name.
        //      identifierOrKeyword identifierNameOrKeyword
        //
        // Note: the newlines are important here.  For example, if that above code
        // were rewritten into:
        //
        //      name.identifierOrKeyword
        //      identifierNameOrKeyword
        //
        // Then we would consider it valid.  That's because ASI would take effect and
        // the code would be implicitly: "name.identifierOrKeyword; identifierNameOrKeyword".
        // In the first case though, ASI will not take effect because there is not a
        // line terminator after the identifier or keyword.
        if self.has_preceding_line_break()
            && token_is_identifier_or_keyword(self.token)
            && self.look_ahead(&mut Self::next_token_is_identifier_or_keyword_on_same_line)
        {
            // Report that we need an identifier.  However, report it right after the dot,
            // and not on the next token.  This is because the next token might actually
            // be an identifier and the error would be quite confusing.
            let pos = self.node_pos();
            self.parse_error_at(pos, pos, diag::Identifier_expected, args![]);
            return self.create_missing_identifier();
        }
        if self.token == SyntaxKind::PrivateIdentifier {
            let node = self.parse_private_identifier();
            if allow_private_identifiers {
                return node;
            }
            let pos = self.node_pos();
            self.parse_error_at(pos, pos, diag::Identifier_expected, args![]);
            return self.create_missing_identifier();
        }
        if allow_identifier_names {
            if allow_unicode_escape_sequence_in_identifier_name {
                return self.parse_identifier_name();
            }
            return self.parse_identifier_name_error_on_unicode_escape_sequence();
        }
        let save_has_await_identifier = self.statement_has_await_identifier;
        let id = self.parse_identifier();
        self.statement_has_await_identifier = save_has_await_identifier;
        id
    }

    // Go: parser/parser.go:3041 newIdentifier
    pub fn new_identifier(&mut self, text: &str) -> Node {
        // Go `int` wraps (see `program::identifier_count`).
        self.identifier_count = self.identifier_count.wrapping_add(1);
        let id = self.factory.new_identifier(text);
        if text == "await" {
            self.statement_has_await_identifier = true;
        }
        id
    }

    // Go: parser/parser.go:3029 createMissingIdentifier
    pub fn create_missing_identifier(&mut self) -> Node {
        let id = self.new_identifier("");
        let pos = self.node_pos();
        self.finish_node(id, pos)
    }

    // Go: parser/parser.go:3033 parsePrivateIdentifier
    pub fn parse_private_identifier(&mut self) -> Node {
        let pos = self.node_pos();
        let text = self.scanner.token_value();
        self.next_token();
        let result = self.factory.new_private_identifier(text);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3040 reScanLessThanToken
    pub fn re_scan_less_than_token(&mut self) -> SyntaxKind {
        self.token = self.scanner.re_scan_less_than_token();
        self.token
    }

    // Go: parser/parser.go:3045 reScanGreaterThanToken
    pub fn re_scan_greater_than_token(&mut self) -> SyntaxKind {
        self.token = self.scanner.re_scan_greater_than_token();
        self.token
    }

    // Go: parser/parser.go:3050 reScanSlashToken
    pub fn re_scan_slash_token(&mut self) -> SyntaxKind {
        self.token = self.scanner.re_scan_slash_token(false);
        self.token
    }

    // Go: parser/parser.go:3055 reScanTemplateToken
    pub fn re_scan_template_token(&mut self, is_tagged_template: bool) -> SyntaxKind {
        self.token = self.scanner.re_scan_template_token(is_tagged_template);
        self.token
    }

    // Go: parser/parser.go:3060 parseTypeArgumentsOfTypeReference
    pub fn parse_type_arguments_of_type_reference(&mut self) -> NodeList {
        if !self.has_preceding_line_break()
            && self.re_scan_less_than_token() == SyntaxKind::LessThanToken
        {
            return self.parse_type_arguments();
        }
        NodeList::NIL
    }

    // Go: parser/parser.go:3067 parseTypeArguments
    pub fn parse_type_arguments(&mut self) -> NodeList {
        if self.token == SyntaxKind::LessThanToken {
            return self.parse_bracketed_list(
                ParsingContext::TypeArguments,
                &mut Self::parse_type,
                SyntaxKind::LessThanToken,
                SyntaxKind::GreaterThanToken,
            );
        }
        NodeList::NIL
    }

    // Go: parser/parser.go:3074 nextIsStartOfTypeOfImportType
    pub fn next_is_start_of_type_of_import_type(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::ImportKeyword
    }

    // Go: parser/parser.go:3079 parseImportType
    pub fn parse_import_type(&mut self) -> Node {
        self.source_flags |= NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
        let pos = self.node_pos();
        let is_type_of = self.parse_optional(SyntaxKind::TypeOfKeyword);
        self.parse_expected(SyntaxKind::ImportKeyword);
        self.parse_expected(SyntaxKind::OpenParenToken);
        let type_node = self.parse_type();
        let mut attributes = Node::NIL;
        if self.parse_optional(SyntaxKind::CommaToken) {
            let open_brace_position = self.scanner.token_start();
            self.parse_expected(SyntaxKind::OpenBraceToken);
            let current_token = self.token;
            if current_token == SyntaxKind::WithKeyword
                || current_token == SyntaxKind::AssertKeyword
            {
                if current_token == SyntaxKind::AssertKeyword {
                    self.parse_error_at_current_token(
                        diag::Import_assertions_have_been_replaced_by_import_attributes_Use_with_instead_of_assert,
                        args![],
                    );
                }
                self.next_token();
            } else {
                self.parse_error_at_current_token(
                    diag::X_0_expected,
                    args![token_to_string(SyntaxKind::WithKeyword)],
                );
            }
            self.parse_expected(SyntaxKind::ColonToken);
            attributes = self.parse_import_attributes(current_token, true /*skipKeyword*/);
            self.parse_optional(SyntaxKind::CommaToken);
            if !self.parse_expected(SyntaxKind::CloseBraceToken) {
                add_related_brace_info_to_last_diagnostic(self, open_brace_position);
            }
        }
        self.parse_expected(SyntaxKind::CloseParenToken);
        let mut qualifier = Node::NIL;
        if self.parse_optional(SyntaxKind::DotToken) {
            qualifier = self.parse_entity_name_of_type_reference();
        }
        let type_arguments = self.parse_type_arguments_of_type_reference();
        let result = self.factory.new_import_type_node(
            is_type_of,
            type_node,
            attributes,
            qualifier,
            type_arguments,
        );
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3121 parseImportAttribute
    pub fn parse_import_attribute(&mut self) -> Node {
        let pos = self.node_pos();
        let mut name = Node::NIL;
        if token_is_identifier_or_keyword(self.token) {
            name = self.parse_identifier_name();
        } else if self.token == SyntaxKind::StringLiteral {
            name = self.parse_literal_expression();
        }
        if name.is_some() {
            self.parse_expected(SyntaxKind::ColonToken);
        } else {
            self.parse_error_at_current_token(diag::Identifier_or_string_literal_expected, args![]);
        }
        let value = self.parse_assignment_expression_or_higher();
        let result = self.factory.new_import_attribute(name, value);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3138 parseImportAttributes
    pub fn parse_import_attributes(&mut self, token: SyntaxKind, skip_keyword: bool) -> Node {
        let pos = self.node_pos();
        if !skip_keyword {
            self.parse_expected(token);
        }
        let elements;
        let mut multi_line = false;
        let open_brace_position = self.scanner.token_start();
        if self.parse_expected(SyntaxKind::OpenBraceToken) {
            multi_line = self.has_preceding_line_break();
            elements = self.parse_delimited_list(
                ParsingContext::ImportAttributes,
                &mut Self::parse_import_attribute,
            );
            if !self.parse_expected(SyntaxKind::CloseBraceToken) {
                add_related_brace_info_to_last_diagnostic(self, open_brace_position);
            }
        } else {
            elements = self.parse_empty_node_list();
        }
        let result = self
            .factory
            .new_import_attributes(token, elements, multi_line);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3164 parseTypeQuery
    pub fn parse_type_query(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::TypeOfKeyword);
        let entity_name = self.parse_entity_name(
            true, /*allowReservedWords*/
            true, /*allowPrivateName*/
            None,
        );
        // Make sure we perform ASI to prevent parsing the next line's type arguments as part of an instantiation expression
        let mut type_arguments = NodeList::NIL;
        if !self.has_preceding_line_break() {
            type_arguments = self.parse_type_arguments();
        }
        let result = self
            .factory
            .new_type_query_node(entity_name, type_arguments);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3176 nextIsStartOfMappedType
    pub fn next_is_start_of_mapped_type(&mut self) -> bool {
        self.next_token();
        if self.token == SyntaxKind::PlusToken || self.token == SyntaxKind::MinusToken {
            return self.next_token() == SyntaxKind::ReadonlyKeyword;
        }
        if self.token == SyntaxKind::ReadonlyKeyword {
            self.next_token();
        }
        self.token == SyntaxKind::OpenBracketToken
            && self.next_token_is_identifier()
            && self.next_token() == SyntaxKind::InKeyword
    }

    // Go: parser/parser.go:3187 parseMappedType
    pub fn parse_mapped_type(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBraceToken);
        let mut readonly_token = Node::NIL; // ReadonlyKeyword | PlusToken | MinusToken
        if self.token == SyntaxKind::ReadonlyKeyword
            || self.token == SyntaxKind::PlusToken
            || self.token == SyntaxKind::MinusToken
        {
            readonly_token = self.parse_token_node();
            if readonly_token.kind() != SyntaxKind::ReadonlyKeyword {
                self.parse_expected(SyntaxKind::ReadonlyKeyword);
            }
        }
        self.parse_expected(SyntaxKind::OpenBracketToken);
        let type_parameter = self.parse_mapped_type_parameter();
        let mut name_type = Node::NIL;
        if self.parse_optional(SyntaxKind::AsKeyword) {
            name_type = self.parse_type();
        }
        self.parse_expected(SyntaxKind::CloseBracketToken);
        let mut question_token = Node::NIL; // QuestionToken | PlusToken | MinusToken
        if self.token == SyntaxKind::QuestionToken
            || self.token == SyntaxKind::PlusToken
            || self.token == SyntaxKind::MinusToken
        {
            question_token = self.parse_token_node();
            if question_token.kind() != SyntaxKind::QuestionToken {
                self.parse_expected(SyntaxKind::QuestionToken);
            }
        }
        let type_node = self.parse_type_annotation();
        self.parse_semicolon();
        let members = self.parse_list(ParsingContext::TypeMembers, &mut Self::parse_type_member);
        self.parse_expected(SyntaxKind::CloseBraceToken);
        let result = self.factory.new_mapped_type_node(
            readonly_token,
            type_parameter,
            name_type,
            question_token,
            type_node,
            members,
        );
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3218 parseMappedTypeParameter
    pub fn parse_mapped_type_parameter(&mut self) -> Node {
        let pos = self.node_pos();
        let name = self.parse_identifier_name();
        self.parse_expected(SyntaxKind::InKeyword);
        let type_node = self.parse_type();
        let result = self.factory.new_type_parameter_declaration(
            ModifierList::NIL, /*modifiers*/
            name,
            type_node,
            Node::NIL, /*expression*/
            Node::NIL, /*defaultType*/
        );
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3226 parseTypeMember
    pub fn parse_type_member(&mut self) -> Node {
        if self.token == SyntaxKind::OpenParenToken || self.token == SyntaxKind::LessThanToken {
            return self.parse_signature_member(SyntaxKind::CallSignature);
        }
        if self.token == SyntaxKind::NewKeyword
            && self.look_ahead(&mut Self::next_token_is_open_paren_or_less_than)
        {
            return self.parse_signature_member(SyntaxKind::ConstructSignature);
        }
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        if self.parse_contextual_modifier(SyntaxKind::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::GetAccessor,
                ParseFlags::TYPE,
            );
        }
        if self.parse_contextual_modifier(SyntaxKind::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::SetAccessor,
                ParseFlags::TYPE,
            );
        }
        if self.is_index_signature() {
            return self.parse_index_signature_declaration(pos, jsdoc, modifiers);
        }
        self.parse_property_or_method_signature(pos, jsdoc, modifiers)
    }

    // Go: parser/parser.go:3248 nextTokenIsOpenParenOrLessThan
    pub fn next_token_is_open_paren_or_less_than(&mut self) -> bool {
        self.next_token();
        self.token == SyntaxKind::OpenParenToken || self.token == SyntaxKind::LessThanToken
    }

    // Go: parser/parser.go:3253 parseSignatureMember
    pub fn parse_signature_member(&mut self, kind: SyntaxKind) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if kind == SyntaxKind::ConstructSignature {
            self.parse_expected(SyntaxKind::NewKeyword);
        }
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(ParseFlags::TYPE);
        let type_node = self.parse_return_type(SyntaxKind::ColonToken, true /*isType*/);
        self.parse_type_member_semicolon();
        let result = if kind == SyntaxKind::CallSignature {
            self.factory
                .new_call_signature_declaration(type_parameters, parameters, type_node)
        } else {
            self.factory
                .new_construct_signature_declaration(type_parameters, parameters, type_node)
        };
        self.finish_node(result, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:3274 parseTypeParameters
    pub fn parse_type_parameters(&mut self) -> NodeList {
        if self.token == SyntaxKind::LessThanToken {
            return self.parse_bracketed_list(
                ParsingContext::TypeParameters,
                &mut Self::parse_type_parameter,
                SyntaxKind::LessThanToken,
                SyntaxKind::GreaterThanToken,
            );
        }
        NodeList::NIL
    }

    // Go: parser/parser.go:3281 parseTypeParameter
    pub fn parse_type_parameter(&mut self) -> Node {
        let pos = self.node_pos();
        let modifiers = self.parse_modifiers_ex(
            false, /*allowDecorators*/
            true,  /*permitConstAsModifier*/
            false, /*stopOnStartOfClassStaticBlock*/
        );
        let name = self.parse_identifier();
        let mut constraint = Node::NIL;
        let mut expression = Node::NIL;
        if self.parse_optional(SyntaxKind::ExtendsKeyword) {
            // It's not uncommon for people to write improper constraints to a generic.  If the
            // user writes a constraint that is an expression and not an actual type, then parse
            // it out as an expression (so we can recover well), but report that a type is needed
            // instead.
            if self.is_start_of_type(false /*inStartOfParameter*/) || !self.is_start_of_expression()
            {
                constraint = self.parse_type();
            } else {
                // It was not a type, and it looked like an expression.  Parse out an expression
                // here so we recover well.  Note: it is important that we call parseUnaryExpression
                // and not parseExpression here.  If the user has:
                //
                //      <T extends "">
                //
                // We do *not* want to consume the `>` as we're consuming the expression for "".
                expression = self.parse_unary_expression_or_higher();
            }
        }
        let mut default_type = Node::NIL;
        if self.parse_optional(SyntaxKind::EqualsToken) {
            default_type = self.parse_type();
        }
        let result = self.factory.new_type_parameter_declaration(
            modifiers,
            name,
            constraint,
            expression,
            default_type,
        );
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3313 parseParameters
    pub fn parse_parameters(&mut self, flags: ParseFlags) -> NodeList {
        // FormalParameters [Yield,Await]: (modified)
        //      [empty]
        //      FormalParameterList[?Yield,Await]
        //
        // FormalParameter[Yield,Await]: (modified)
        //      BindingElement[?Yield,Await]
        //
        // BindingElement [Yield,Await]: (modified)
        //      SingleNameBinding[?Yield,?Await]
        //      BindingPattern[?Yield,?Await]Initializer [In, ?Yield,?Await] opt
        //
        // SingleNameBinding [Yield,Await]:
        //      BindingIdentifier[?Yield,?Await]Initializer [In, ?Yield,?Await] opt
        if self.parse_expected(SyntaxKind::OpenParenToken) {
            let parameters = self.parse_parameters_worker(flags, true /*allowAmbiguity*/);
            self.parse_expected(SyntaxKind::CloseParenToken);
            return parameters;
        }
        self.create_missing_list()
    }

    // Go: parser/parser.go:3335 parseParametersWorker
    pub fn parse_parameters_worker(
        &mut self,
        flags: ParseFlags,
        allow_ambiguity: bool,
    ) -> NodeList {
        // FormalParameters [Yield,Await]: (modified)
        //      [empty]
        //      FormalParameterList[?Yield,Await]
        //
        // FormalParameter[Yield,Await]: (modified)
        //      BindingElement[?Yield,Await]
        //
        // BindingElement [Yield,Await]: (modified)
        //      SingleNameBinding[?Yield,?Await]
        //      BindingPattern[?Yield,?Await]Initializer [In, ?Yield,?Await] opt
        //
        // SingleNameBinding [Yield,Await]:
        //      BindingIdentifier[?Yield,?Await]Initializer [In, ?Yield,?Await] opt
        let in_await_context = self.context_flags.intersects(NodeFlags::AWAIT_CONTEXT);
        let save_context_flags = self.context_flags;
        self.set_context_flags(
            NodeFlags::YIELD_CONTEXT,
            flags.intersects(ParseFlags::YIELD),
        );
        self.set_context_flags(
            NodeFlags::AWAIT_CONTEXT,
            flags.intersects(ParseFlags::AWAIT),
        );
        let parameters =
            self.parse_delimited_list(ParsingContext::Parameters, &mut |p: &mut Parser<'a>| {
                let parameter = p.parse_parameter_ex(in_await_context, allow_ambiguity);
                if parameter.is_some() && !flags.intersects(ParseFlags::TYPE) {
                    p.check_js_syntax(parameter);
                }
                parameter
            });
        self.context_flags = save_context_flags;
        parameters
    }

    // Go: parser/parser.go:3364 parseParameter
    pub fn parse_parameter(&mut self) -> Node {
        self.parse_parameter_ex(
            false, /*inOuterAwaitContext*/
            true,  /*allowAmbiguity*/
        )
    }

    // Go: parser/parser.go:3368 parseParameterEx
    pub fn parse_parameter_ex(
        &mut self,
        in_outer_await_context: bool,
        allow_ambiguity: bool,
    ) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        // FormalParameter [Yield,Await]:
        //      BindingElement[?Yield,?Await]
        // Decorators are parsed in the outer [Await] context, the rest of the parameter is parsed in the function's [Await] context.
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, in_outer_await_context);
        let modifiers = self.parse_modifiers_ex(
            true,  /*allowDecorators*/
            false, /*permitConstAsModifier*/
            false, /*stopOnStartOfClassStaticBlock*/
        );
        self.context_flags = save_context_flags;
        // PORT: not in Go (bindgapcost1). Each decorated parameter of a file
        // is made here or is a reparse clone of one made here, so a file
        // without this mark has none (`ParsedSourceFile::has_decorated_parameter`).
        if modifiers.is_some() && modifiers.nodes().iter().any(is_decorator) {
            self.has_decorated_parameter = true;
        }
        if self.token == SyntaxKind::ThisKeyword {
            let name = self.create_identifier(true /*isIdentifier*/);
            let type_node = self.parse_type_annotation();
            let result = self.factory.new_parameter_declaration(
                modifiers,
                Node::NIL, /*dotDotDotToken*/
                name,
                Node::NIL, /*questionToken*/
                type_node,
                Node::NIL, /*initializer*/
            );
            if modifiers.is_some() {
                let loc = modifiers.nodes().get(0).loc();
                self.parse_error_at_range(
                    loc,
                    diag::Neither_decorators_nor_modifiers_may_be_applied_to_this_parameters,
                    args![],
                );
            }
            let finished = self.finish_node(result, pos);
            self.with_js_doc(finished, jsdoc);
            return result;
        }
        let dot_dot_dot_token = self.parse_optional_token(SyntaxKind::DotDotDotToken);
        if !allow_ambiguity && !self.is_parameter_name_start() {
            return Node::NIL;
        }
        let name = self.parse_name_of_parameter(modifiers);
        let question_token = self.parse_optional_token(SyntaxKind::QuestionToken);
        let type_node = self.parse_type_annotation();
        let initializer = self.parse_initializer();
        let result = self.factory.new_parameter_declaration(
            modifiers,
            dot_dot_dot_token,
            name,
            question_token,
            type_node,
            initializer,
        );
        let finished = self.finish_node(result, pos);
        self.with_js_doc(finished, jsdoc);
        result
    }

    // Go: parser/parser.go:3409 isParameterNameStart
    pub fn is_parameter_name_start(&mut self) -> bool {
        // Be permissive about await and yield by calling isBindingIdentifier instead of isIdentifier; disallowing
        // them during a speculative parse leads to many more follow-on errors than allowing the function to parse then later
        // complaining about the use of the keywords.
        self.is_binding_identifier()
            || self.token == SyntaxKind::OpenBracketToken
            || self.token == SyntaxKind::OpenBraceToken
    }

    // Go: parser/parser.go:3416 parseNameOfParameter
    pub fn parse_name_of_parameter(&mut self, modifiers: ModifierList) -> Node {
        // FormalParameter [Yield,Await]:
        //      BindingElement[?Yield,?Await]
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            diag::Private_identifiers_cannot_be_used_as_parameters,
        ));
        if name.loc().len() == 0 && modifiers.is_nil() && is_modifier_kind(self.token) {
            // in cases like
            // 'use strict'
            // function foo(static)
            // isParameter('static') == true, because of isModifier('static')
            // however 'static' is not a legal identifier in a strict mode.
            // so result of this function will be Parameter (flags = 0, name = missing, type = undefined, initializer = undefined)
            // and current token will not change => parsing of the enclosing parameter list will last till the end of time (or OOM)
            // to avoid this we'll advance cursor to the next token.
            self.next_token();
        }
        name
    }

    // Go: parser/parser.go:3434 parseReturnType
    pub fn parse_return_type(&mut self, return_token: SyntaxKind, is_type: bool) -> Node {
        if self.should_parse_return_type(return_token, is_type) {
            return self.do_in_context(
                NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                false,
                &mut Self::parse_type_or_type_predicate,
            );
        }
        Node::NIL
    }

    // Go: parser/parser.go:3441 shouldParseReturnType
    pub fn should_parse_return_type(&mut self, return_token: SyntaxKind, is_type: bool) -> bool {
        if return_token == SyntaxKind::EqualsGreaterThanToken {
            self.parse_expected(return_token);
            return true;
        } else if self.parse_optional(SyntaxKind::ColonToken) {
            return true;
        } else if is_type && self.token == SyntaxKind::EqualsGreaterThanToken {
            // This is easy to get backward, especially in type contexts, so parse the type anyway
            self.parse_error_at_current_token(
                diag::X_0_expected,
                args![token_to_string(SyntaxKind::ColonToken)],
            );
            self.next_token();
            return true;
        }
        false
    }

    // Go: parser/parser.go:3456 parseTypeOrTypePredicate
    pub fn parse_type_or_type_predicate(&mut self) -> Node {
        if self.is_identifier() {
            let state = self.mark();
            let pos = self.node_pos();
            let id = self.parse_identifier();
            if self.token == SyntaxKind::IsKeyword && !self.has_preceding_line_break() {
                self.next_token();
                let type_node = self.parse_type();
                let result = self.factory.new_type_predicate_node(
                    Node::NIL, /*assertsModifier*/
                    id,
                    type_node,
                );
                return self.finish_node(result, pos);
            }
            self.rewind(state);
        }
        self.parse_type()
    }

    // Go: parser/parser.go:3470 parseTypeMemberSemicolon
    pub fn parse_type_member_semicolon(&mut self) {
        // We allow type members to be separated by commas or (possibly ASI) semicolons.
        // First check if it was a comma.  If so, we're done with the member.
        if self.parse_optional(SyntaxKind::CommaToken) {
            return;
        }
        // Didn't have a comma.  We must have a (possible ASI) semicolon.
        self.parse_semicolon();
    }

    // Go: parser/parser.go:3480 parseAccessorDeclaration
    pub fn parse_accessor_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
        kind: SyntaxKind,
        flags: ParseFlags,
    ) -> Node {
        let name = self.parse_property_name();
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(ParseFlags::NONE);
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false /*isType*/);
        let body = self.parse_function_block_or_semicolon(flags, None /*diagnosticMessage*/);
        // Keep track of `typeParameters` (for both) and `type` (for setters) if they were parsed those indicate grammar errors
        let result = if kind == SyntaxKind::GetAccessor {
            self.factory.new_get_accessor_declaration(
                modifiers,
                name,
                type_parameters,
                parameters,
                return_type,
                Node::NIL, /*fullSignature*/
                body,
            )
        } else {
            self.factory.new_set_accessor_declaration(
                modifiers,
                name,
                type_parameters,
                parameters,
                return_type,
                Node::NIL, /*fullSignature*/
                body,
            )
        };
        let finished = self.finish_node(result, pos);
        self.with_js_doc(finished, jsdoc);
        if !flags.intersects(ParseFlags::TYPE) {
            self.check_js_syntax(result);
        }
        result
    }

    // Go: parser/parser.go:3500 parsePropertyName
    pub fn parse_property_name(&mut self) -> Node {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let prop = self.parse_property_name_worker(true /*allowComputedPropertyNames*/);
        self.statement_has_await_identifier = save_has_await_identifier;
        prop
    }

    // Go: parser/parser.go:3507 parsePropertyNameWorker
    pub fn parse_property_name_worker(&mut self, allow_computed_property_names: bool) -> Node {
        if self.token == SyntaxKind::StringLiteral
            || self.token == SyntaxKind::NumericLiteral
            || self.token == SyntaxKind::BigIntLiteral
        {
            return self.parse_literal_expression();
        }
        if allow_computed_property_names && self.token == SyntaxKind::OpenBracketToken {
            return self.parse_computed_property_name();
        }
        if self.token == SyntaxKind::PrivateIdentifier {
            return self.parse_private_identifier();
        }
        self.parse_identifier_name()
    }

    // Go: parser/parser.go:3520 parseComputedPropertyName
    pub fn parse_computed_property_name(&mut self) -> Node {
        // PropertyName [Yield]:
        //      LiteralPropertyName
        //      ComputedPropertyName[?Yield]
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenBracketToken);
        // We parse any expression (including a comma expression). But the grammar
        // says that only an assignment expression is allowed, so the grammar checker
        // will error if it sees a comma expression.
        let expression = self.parse_expression_allow_in();
        self.parse_expected(SyntaxKind::CloseBracketToken);
        let result = self.factory.new_computed_property_name(expression);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3534 parseFunctionBlockOrSemicolon
    // PORT: Go `diagnosticMessage` can be nil; it is `Option<&'static Message>`.
    pub fn parse_function_block_or_semicolon(
        &mut self,
        flags: ParseFlags,
        diagnostic_message: Option<&'static crate::diagnostics::Message>,
    ) -> Node {
        if self.token != SyntaxKind::OpenBraceToken {
            if flags.intersects(ParseFlags::TYPE) {
                self.parse_type_member_semicolon();
                return Node::NIL;
            }
            if self.can_parse_semicolon() {
                self.parse_semicolon();
                return Node::NIL;
            }
        }
        self.parse_function_block(flags, diagnostic_message)
    }

    // Go: parser/parser.go:3548 parseFunctionBlock
    // PORT: Go `diagnosticMessage` can be nil; it is `Option<&'static Message>`.
    pub fn parse_function_block(
        &mut self,
        flags: ParseFlags,
        diagnostic_message: Option<&'static crate::diagnostics::Message>,
    ) -> Node {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(
            NodeFlags::YIELD_CONTEXT,
            flags.intersects(ParseFlags::YIELD),
        );
        self.set_context_flags(
            NodeFlags::AWAIT_CONTEXT,
            flags.intersects(ParseFlags::AWAIT),
        );
        // We may be in a [Decorator] context when parsing a function expression or
        // arrow function. The body of the function is not in [Decorator] context.
        self.set_context_flags(NodeFlags::DECORATOR_CONTEXT, false);
        let block = self.parse_block(
            flags.intersects(ParseFlags::IGNORE_MISSING_OPEN_BRACE),
            diagnostic_message,
        );
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        block
    }

    // Go: parser/parser.go:3562 isIndexSignature
    pub fn is_index_signature(&mut self) -> bool {
        self.token == SyntaxKind::OpenBracketToken
            && self.look_ahead(&mut Self::next_is_unambiguously_index_signature)
    }

    // Go: parser/parser.go:3566 nextIsUnambiguouslyIndexSignature
    pub fn next_is_unambiguously_index_signature(&mut self) -> bool {
        // The only allowed sequence is:
        //
        //   [id:
        //
        // However, for error recovery, we also check the following cases:
        //
        //   [...
        //   [id,
        //   [id?,
        //   [id?:
        //   [id?]
        //   [public id
        //   [private id
        //   [protected id
        //   []
        //
        self.next_token();
        if self.token == SyntaxKind::DotDotDotToken || self.token == SyntaxKind::CloseBracketToken {
            return true;
        }
        if is_modifier_kind(self.token) {
            self.next_token();
            if self.is_identifier() {
                return true;
            }
        } else if !self.is_identifier() {
            return false;
        } else {
            // Skip the identifier
            self.next_token();
        }
        // A colon signifies a well formed indexer
        // A comma should be a badly formed indexer because comma expressions are not allowed
        // in computed properties.
        if self.token == SyntaxKind::ColonToken || self.token == SyntaxKind::CommaToken {
            return true;
        }
        // Question mark could be an indexer with an optional property,
        // or it could be a conditional expression in a computed property.
        if self.token != SyntaxKind::QuestionToken {
            return false;
        }
        // If any of the following tokens are after the question mark, it cannot
        // be a conditional expression, so treat it as an indexer.
        self.next_token();
        self.token == SyntaxKind::ColonToken
            || self.token == SyntaxKind::CommaToken
            || self.token == SyntaxKind::CloseBracketToken
    }

    // Go: parser/parser.go:3615 parseIndexSignatureDeclaration
    pub fn parse_index_signature_declaration(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let parameters = self.parse_bracketed_list(
            ParsingContext::Parameters,
            &mut Self::parse_parameter,
            SyntaxKind::OpenBracketToken,
            SyntaxKind::CloseBracketToken,
        );
        let type_node = self.parse_type_annotation();
        self.parse_type_member_semicolon();
        let node = self
            .factory
            .new_index_signature_declaration(modifiers, parameters, type_node);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:3624 parsePropertyOrMethodSignature
    pub fn parse_property_or_method_signature(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        let name = self.parse_property_name();
        let question_token = self.parse_optional_token(SyntaxKind::QuestionToken);
        let result;
        if self.token == SyntaxKind::OpenParenToken || self.token == SyntaxKind::LessThanToken {
            // Method signatures don't exist in expression contexts.  So they have neither
            // [Yield] nor [Await]
            let type_parameters = self.parse_type_parameters();
            let parameters = self.parse_parameters(ParseFlags::TYPE);
            let return_type = self.parse_return_type(SyntaxKind::ColonToken, true /*isType*/);
            result = self.factory.new_method_signature_declaration(
                modifiers,
                name,
                question_token,
                type_parameters,
                parameters,
                return_type,
            );
        } else {
            let type_node = self.parse_type_annotation();
            // Although type literal properties cannot not have initializers, we attempt
            // to parse an initializer so we can report in the checker that an interface
            // property or type literal property cannot have an initializer.
            let mut initializer = Node::NIL;
            if self.token == SyntaxKind::EqualsToken {
                initializer = self.parse_initializer();
            }
            result = self.factory.new_property_signature_declaration(
                modifiers,
                name,
                question_token,
                type_node,
                initializer,
            );
        }
        self.parse_type_member_semicolon();
        let finished = self.finish_node(result, pos);
        self.with_js_doc(finished, jsdoc);
        result
    }

    // Go: parser/parser.go:3651 parseTypeLiteral
    pub fn parse_type_literal(&mut self) -> Node {
        let pos = self.node_pos();
        let members = self.parse_object_type_members();
        let node = self.factory.new_type_literal_node(members);
        self.finish_node(node, pos)
    }

    // Go: parser/parser.go:3657 parseObjectTypeMembers
    pub fn parse_object_type_members(&mut self) -> NodeList {
        if self.parse_expected(SyntaxKind::OpenBraceToken) {
            let members =
                self.parse_list(ParsingContext::TypeMembers, &mut Self::parse_type_member);
            self.parse_expected(SyntaxKind::CloseBraceToken);
            return members;
        }
        self.create_missing_list()
    }

    // Go: parser/parser.go:3666 parseTupleType
    pub fn parse_tuple_type(&mut self) -> Node {
        let pos = self.node_pos();
        let elements = self.parse_bracketed_list(
            ParsingContext::TupleElementTypes,
            &mut Self::parse_tuple_element_name_or_tuple_element_type,
            SyntaxKind::OpenBracketToken,
            SyntaxKind::CloseBracketToken,
        );
        let result = self.factory.new_tuple_type_node(elements);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3671 parseTupleElementNameOrTupleElementType
    pub fn parse_tuple_element_name_or_tuple_element_type(&mut self) -> Node {
        if self.look_ahead(&mut Self::scan_start_of_named_tuple_element) {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let dot_dot_dot_token = self.parse_optional_token(SyntaxKind::DotDotDotToken);
            let name = self.parse_identifier_name();
            let question_token = self.parse_optional_token(SyntaxKind::QuestionToken);
            self.parse_expected(SyntaxKind::ColonToken);
            let type_node = self.parse_tuple_element_type();
            let node = self.factory.new_named_tuple_member(
                dot_dot_dot_token,
                name,
                question_token,
                type_node,
            );
            let result = self.finish_node(node, pos);
            self.with_js_doc(result, jsdoc);
            return result;
        }
        self.parse_tuple_element_type()
    }

    // Go: parser/parser.go:3687 scanStartOfNamedTupleElement
    pub fn scan_start_of_named_tuple_element(&mut self) -> bool {
        if self.token == SyntaxKind::DotDotDotToken {
            return token_is_identifier_or_keyword(self.next_token())
                && self.next_token_is_colon_or_question_colon();
        }
        token_is_identifier_or_keyword(self.token) && self.next_token_is_colon_or_question_colon()
    }

    // Go: parser/parser.go:3694 nextTokenIsColonOrQuestionColon
    pub fn next_token_is_colon_or_question_colon(&mut self) -> bool {
        self.next_token() == SyntaxKind::ColonToken
            || self.token == SyntaxKind::QuestionToken
                && self.next_token() == SyntaxKind::ColonToken
    }

    // Go: parser/parser.go:3698 parseTupleElementType
    pub fn parse_tuple_element_type(&mut self) -> Node {
        let pos = self.node_pos();
        if self.parse_optional(SyntaxKind::DotDotDotToken) {
            let type_node = self.parse_type();
            let result = self.factory.new_rest_type_node(type_node);
            return self.finish_node(result, pos);
        }
        let type_node = self.parse_type();
        if is_js_doc_nullable_type(type_node) && type_node.pos() == type_node.type_().pos() {
            let node = self.factory.new_optional_type_node(type_node.type_());
            set_node_flags(node, type_node.flags());
            set_node_loc(node, type_node.loc());
            set_node_parent(type_node.type_(), node);
            return node;
        }
        type_node
    }

    // Go: parser/parser.go:3714 parseParenthesizedType
    pub fn parse_parenthesized_type(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::OpenParenToken);
        let type_node = self.parse_type();
        self.parse_expected(SyntaxKind::CloseParenToken);
        let result = self.factory.new_parenthesized_type_node(type_node);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3722 parseAssertsTypePredicate
    pub fn parse_asserts_type_predicate(&mut self) -> Node {
        let pos = self.node_pos();
        let asserts_modifier = self.parse_expected_token(SyntaxKind::AssertsKeyword);
        let parameter_name = if self.token == SyntaxKind::ThisKeyword {
            self.parse_this_type_node()
        } else {
            self.parse_identifier()
        };
        let mut type_node = Node::NIL;
        if self.parse_optional(SyntaxKind::IsKeyword) {
            type_node = self.parse_type();
        }
        let result =
            self.factory
                .new_type_predicate_node(asserts_modifier, parameter_name, type_node);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3738 parseTemplateType
    pub fn parse_template_type(&mut self) -> Node {
        let pos = self.node_pos();
        let head = self.parse_template_head(false /*isTaggedTemplate*/);
        let spans = self.parse_template_type_spans();
        let result = self.factory.new_template_literal_type_node(head, spans);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3743 parseTemplateHead
    pub fn parse_template_head(&mut self, is_tagged_template: bool) -> Node {
        if !is_tagged_template
            && self
                .scanner
                .token_flags()
                .intersects(TokenFlags::IS_INVALID)
        {
            self.re_scan_template_token(false /*isTaggedTemplate*/);
        }
        let pos = self.node_pos();
        let text = self.scanner.token_value().to_string();
        let raw_text = self.get_template_literal_raw_text(2 /*endLength*/);
        let result = self
            .factory
            .new_template_head(text, raw_text, self.scanner.token_flags());
        self.next_token();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3753 getTemplateLiteralRawText
    pub fn get_template_literal_raw_text(&self, end_length: i32) -> String {
        let token_text = self.scanner.token_text();
        let mut end_length = end_length;
        if self
            .scanner
            .token_flags()
            .intersects(TokenFlags::UNTERMINATED)
        {
            end_length = 0;
        }
        // PORT: Go slices the token text by byte offsets; the offsets fall on
        // ASCII delimiters (a backtick, `${`, `}`), so they are char boundaries.
        token_text[1..token_text.len() - end_length as usize].to_string()
    }

    // Go: parser/parser.go:3761 parseTemplateTypeSpans
    pub fn parse_template_type_spans(&mut self) -> NodeList {
        let pos = self.node_pos();
        let mut list: Vec<Node> = Vec::new();
        loop {
            let span = self.parse_template_type_span();
            list.push(span);
            if span.literal().kind() != SyntaxKind::TemplateMiddle {
                break;
            }
        }
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &list)
    }

    // Go: parser/parser.go:3774 parseTemplateTypeSpan
    pub fn parse_template_type_span(&mut self) -> Node {
        let pos = self.node_pos();
        let type_node = self.parse_type();
        let literal = self.parse_literal_of_template_span(false /*isTaggedTemplate*/);
        let result = self
            .factory
            .new_template_literal_type_span(type_node, literal);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3779 parseLiteralOfTemplateSpan
    pub fn parse_literal_of_template_span(&mut self, is_tagged_template: bool) -> Node {
        if self.token == SyntaxKind::CloseBraceToken {
            self.re_scan_template_token(is_tagged_template);
            return self.parse_template_middle_or_tail();
        }
        self.parse_error_at_current_token(
            diag::X_0_expected,
            args![token_to_string(SyntaxKind::CloseBraceToken)],
        );
        let result = self.factory.new_template_tail("", "", TokenFlags::NONE);
        let pos = self.node_pos();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3788 parseTemplateMiddleOrTail
    pub fn parse_template_middle_or_tail(&mut self) -> Node {
        let pos = self.node_pos();
        let text = self.scanner.token_value().to_string();
        let result = if self.token == SyntaxKind::TemplateMiddle {
            let raw_text = self.get_template_literal_raw_text(2 /*endLength*/);
            self.factory
                .new_template_middle(text, raw_text, self.scanner.token_flags())
        } else {
            let raw_text = self.get_template_literal_raw_text(1 /*endLength*/);
            self.factory
                .new_template_tail(text, raw_text, self.scanner.token_flags())
        };
        self.next_token();
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3800 parseFunctionOrConstructorTypeToError
    pub fn parse_function_or_constructor_type_to_error(
        &mut self,
        is_in_union_type: bool,
        parse_constituent_type: impl FnOnce(&mut Parser<'a>) -> Node,
    ) -> Node {
        // the function type and constructor type shorthand notation
        // are not allowed directly in unions and intersections, but we'll
        // try to parse them gracefully and issue a helpful message.
        if self.is_start_of_function_type_or_constructor_type() {
            let type_node = self.parse_function_or_constructor_type();
            let diagnostic = if type_node.kind() == SyntaxKind::FunctionType {
                if is_in_union_type {
                    diag::Function_type_notation_must_be_parenthesized_when_used_in_a_union_type
                } else {
                    diag::Function_type_notation_must_be_parenthesized_when_used_in_an_intersection_type
                }
            } else if is_in_union_type {
                diag::Constructor_type_notation_must_be_parenthesized_when_used_in_a_union_type
            } else {
                diag::Constructor_type_notation_must_be_parenthesized_when_used_in_an_intersection_type
            };
            self.parse_error_at_range(type_node.loc(), diagnostic, args![]);
            return type_node;
        }
        parse_constituent_type(self)
    }

    // Go: parser/parser.go:3822 isStartOfFunctionTypeOrConstructorType
    pub fn is_start_of_function_type_or_constructor_type(&mut self) -> bool {
        self.token == SyntaxKind::LessThanToken
            || self.token == SyntaxKind::OpenParenToken
                && self.look_ahead(&mut Self::next_is_unambiguously_start_of_function_type)
            || self.token == SyntaxKind::NewKeyword
            || self.token == SyntaxKind::AbstractKeyword
                && self.look_ahead(&mut Self::next_token_is_new_keyword)
    }

    // Go: parser/parser.go:3829 parseFunctionOrConstructorType
    pub fn parse_function_or_constructor_type(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_constructor_type();
        let is_constructor_type = self.parse_optional(SyntaxKind::NewKeyword);
        debug_assert!(
            modifiers.is_nil() || is_constructor_type,
            "Per isStartOfFunctionOrConstructorType, a function type cannot have modifiers."
        );
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(ParseFlags::TYPE);
        let return_type =
            self.parse_return_type(SyntaxKind::EqualsGreaterThanToken, false /*isType*/);
        let result = if is_constructor_type {
            self.factory.new_constructor_type_node(
                modifiers,
                type_parameters,
                parameters,
                return_type,
            )
        } else {
            self.factory
                .new_function_type_node(type_parameters, parameters, return_type)
        };
        self.finish_node(result, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser/parser.go:3849 parseModifiersForConstructorType
    pub fn parse_modifiers_for_constructor_type(&mut self) -> ModifierList {
        if self.token == SyntaxKind::AbstractKeyword {
            let pos = self.node_pos();
            let modifier = self.factory.new_modifier(self.token);
            self.next_token();
            self.finish_node(modifier, pos);
            // PORT: Go `p.nodeSliceArena.NewSlice1(modifier)` is a one-element slice.
            return self.new_modifier_list(modifier.loc(), &[modifier]);
        }
        ModifierList::NIL
    }

    // Go: parser/parser.go:3860 nextTokenIsNewKeyword
    pub fn next_token_is_new_keyword(&mut self) -> bool {
        self.next_token() == SyntaxKind::NewKeyword
    }

    // Go: parser/parser.go:3864 nextIsUnambiguouslyStartOfFunctionType
    pub fn next_is_unambiguously_start_of_function_type(&mut self) -> bool {
        self.next_token();
        if self.token == SyntaxKind::CloseParenToken || self.token == SyntaxKind::DotDotDotToken {
            // ( )
            // ( ...
            return true;
        }
        if self.skip_parameter_start() {
            // We successfully skipped modifiers (if any) and an identifier or binding pattern,
            // now see if we have something that indicates a parameter declaration
            if self.token == SyntaxKind::ColonToken
                || self.token == SyntaxKind::CommaToken
                || self.token == SyntaxKind::QuestionToken
                || self.token == SyntaxKind::EqualsToken
            {
                // ( xxx :
                // ( xxx ,
                // ( xxx ?
                // ( xxx =
                return true;
            }
            if self.token == SyntaxKind::CloseParenToken
                && self.next_token() == SyntaxKind::EqualsGreaterThanToken
            {
                // ( xxx ) =>
                return true;
            }
        }
        false
    }

    // Go: parser/parser.go:3889 skipParameterStart
    pub fn skip_parameter_start(&mut self) -> bool {
        if is_modifier_kind(self.token) {
            // Skip modifiers
            self.parse_modifiers();
        }
        self.parse_optional(SyntaxKind::DotDotDotToken);
        if self.is_identifier() || self.token == SyntaxKind::ThisKeyword {
            self.next_token();
            return true;
        }
        if self.token == SyntaxKind::OpenBracketToken || self.token == SyntaxKind::OpenBraceToken {
            // Return true if we can parse an array or object binding pattern with no errors
            let previous_error_count = diagnostics_len(self);
            self.parse_identifier_or_pattern();
            return previous_error_count == diagnostics_len(self);
        }
        false
    }

    // Go: parser/parser.go:3908 parseModifiers
    pub fn parse_modifiers(&mut self) -> ModifierList {
        self.parse_modifiers_ex(false, false, false)
    }

    // Go: parser/parser.go:3912 parseModifiersEx
    pub fn parse_modifiers_ex(
        &mut self,
        allow_decorators: bool,
        permit_const_as_modifier: bool,
        stop_on_start_of_class_static_block: bool,
    ) -> ModifierList {
        let mut has_leading_modifier = false;
        let mut has_trailing_decorator = false;
        let mut has_trailing_modifier = false;
        let mut has_static_modifier = false;
        // Decorators should be contiguous in a list of modifiers but can potentially appear in two places (i.e., `[...leadingDecorators, ...leadingModifiers, ...trailingDecorators, ...trailingModifiers]`).
        // The leading modifiers *should* only contain `export` and `default` when trailingDecorators are present, but we'll handle errors for any other leading modifiers in the checker.
        // It is illegal to have both leadingDecorators and trailingDecorators, but we will report that as a grammar check in the checker.
        // parse leading decorators
        let pos = self.node_pos();
        // PERF: `new_modifier_list` copies the nodes out, so the scratch list
        // stays on the stack.
        let mut list: SmallVec<[Node; 16]> = SmallVec::new();
        loop {
            if allow_decorators && self.token == SyntaxKind::AtToken && !has_trailing_modifier {
                let decorator = self.parse_decorator();
                list.push(decorator);
                if has_leading_modifier {
                    has_trailing_decorator = true;
                }
            } else {
                let modifier = self.try_parse_modifier(
                    has_static_modifier,
                    permit_const_as_modifier,
                    stop_on_start_of_class_static_block,
                );
                if modifier.is_nil() {
                    break;
                }
                if modifier.kind() == SyntaxKind::StaticKeyword {
                    has_static_modifier = true;
                }
                list.push(modifier);
                if has_trailing_decorator {
                    has_trailing_modifier = true;
                } else {
                    has_leading_modifier = true;
                }
            }
        }
        if !list.is_empty() {
            let end = self.node_pos();
            // PORT: Go `p.nodeSliceArena.Clone(list)` copies into an arena slice.
            return self.new_modifier_list(TextRange::new(pos, end), &list);
        }
        ModifierList::NIL
    }

    // Go: parser/parser.go:3952 parseDecorator
    pub fn parse_decorator(&mut self) -> Node {
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::AtToken);
        let expression = self.do_in_context(
            NodeFlags::DECORATOR_CONTEXT,
            true,
            &mut Self::parse_decorator_expression,
        );
        let result = self.factory.new_decorator(expression);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3959 parseDecoratorExpression
    pub fn parse_decorator_expression(&mut self) -> Node {
        if self.in_await_context() && self.token == SyntaxKind::AwaitKeyword {
            // `@await` is disallowed in an [Await] context, but can cause parsing to go off the rails
            // This simply parses the missing identifier and moves on.
            let pos = self.node_pos();
            let await_expression =
                self.parse_identifier_with_diagnostic(Some(diag::Expression_expected), None);
            self.next_token();
            let member_expression = self.parse_member_expression_rest(
                pos,
                await_expression,
                true, /*allowOptionalChain*/
            );
            return self.parse_call_expression_rest(pos, member_expression);
        }
        self.parse_left_hand_side_expression_or_higher()
    }

    // Go: parser/parser.go:3972 tryParseModifier
    pub fn try_parse_modifier(
        &mut self,
        has_seen_static_modifier: bool,
        permit_const_as_modifier: bool,
        stop_on_start_of_class_static_block: bool,
    ) -> Node {
        let pos = self.node_pos();
        let kind = self.token;
        if self.token == SyntaxKind::ConstKeyword && permit_const_as_modifier {
            // We need to ensure that any subsequent modifiers appear on the same line
            // so that when 'const' is a standalone declaration, we don't issue an error.
            if !self.look_ahead(&mut Self::next_token_is_on_same_line_and_can_follow_modifier) {
                return Node::NIL;
            } else {
                self.next_token();
            }
        } else if stop_on_start_of_class_static_block
            && self.token == SyntaxKind::StaticKeyword
            && self.look_ahead(&mut Self::next_token_is_open_brace)
        {
            return Node::NIL;
        } else if has_seen_static_modifier && self.token == SyntaxKind::StaticKeyword {
            return Node::NIL;
        } else if !self.parse_any_contextual_modifier() {
            return Node::NIL;
        }
        let result = self.factory.new_modifier(kind);
        self.finish_node(result, pos)
    }

    // Go: parser/parser.go:3995 parseContextualModifier
    pub fn parse_contextual_modifier(&mut self, t: SyntaxKind) -> bool {
        let state = self.mark();
        if self.token == t && self.next_token_can_follow_modifier() {
            return true;
        }
        self.rewind(state);
        false
    }

    // Go: parser/parser.go:4004 parseAnyContextualModifier
    pub fn parse_any_contextual_modifier(&mut self) -> bool {
        let state = self.mark();
        if is_modifier_kind(self.token) && self.next_token_can_follow_modifier() {
            return true;
        }
        self.rewind(state);
        false
    }

    // Go: parser/parser.go:4013 nextTokenCanFollowModifier
    pub fn next_token_can_follow_modifier(&mut self) -> bool {
        match self.token {
            SyntaxKind::ConstKeyword => {
                // 'const' is only a modifier if followed by 'enum'.
                self.next_token() == SyntaxKind::EnumKeyword
            }
            SyntaxKind::ExportKeyword => {
                self.next_token();
                if self.token == SyntaxKind::DefaultKeyword {
                    return self.look_ahead(&mut Self::next_token_can_follow_default_keyword);
                }
                if self.token == SyntaxKind::TypeKeyword {
                    return self.look_ahead(&mut Self::next_token_can_follow_export_modifier);
                }
                self.can_follow_export_modifier()
            }
            SyntaxKind::DefaultKeyword => self.next_token_can_follow_default_keyword(),
            SyntaxKind::StaticKeyword => {
                self.next_token();
                self.can_follow_modifier()
            }
            SyntaxKind::GetKeyword | SyntaxKind::SetKeyword => {
                self.next_token();
                self.can_follow_get_or_set_keyword()
            }
            _ => self.next_token_is_on_same_line_and_can_follow_modifier(),
        }
    }

    // Go: parser/parser.go:4040 nextTokenCanFollowDefaultKeyword
    pub fn next_token_can_follow_default_keyword(&mut self) -> bool {
        match self.next_token() {
            SyntaxKind::ClassKeyword
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::InterfaceKeyword
            | SyntaxKind::AtToken => true,
            SyntaxKind::AbstractKeyword => {
                self.look_ahead(&mut Self::next_token_is_class_keyword_on_same_line)
            }
            SyntaxKind::AsyncKeyword => {
                self.look_ahead(&mut Self::next_token_is_function_keyword_on_same_line)
            }
            _ => false,
        }
    }

    // Go: parser/parser.go:4052 nextTokenIsIdentifierOrKeyword
    pub fn next_token_is_identifier_or_keyword(&mut self) -> bool {
        token_is_identifier_or_keyword(self.next_token())
    }

    // Go: parser/parser.go:4056 nextTokenIsIdentifierOrKeywordOrGreaterThan
    pub fn next_token_is_identifier_or_keyword_or_greater_than(&mut self) -> bool {
        token_is_identifier_or_keyword_or_greater_than(self.next_token())
    }

    // Go: parser/parser.go:4060 nextTokenIsIdentifierOrKeywordOnSameLine
    pub fn next_token_is_identifier_or_keyword_on_same_line(&mut self) -> bool {
        self.next_token_is_identifier_or_keyword() && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:4064 nextTokenIsIdentifierOrKeywordOrLiteralOnSameLine
    pub fn next_token_is_identifier_or_keyword_or_literal_on_same_line(&mut self) -> bool {
        (self.next_token_is_identifier_or_keyword()
            || self.token == SyntaxKind::NumericLiteral
            || self.token == SyntaxKind::BigIntLiteral
            || self.token == SyntaxKind::StringLiteral)
            && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:4068 nextTokenIsClassKeywordOnSameLine
    pub fn next_token_is_class_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == SyntaxKind::ClassKeyword && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:4072 nextTokenIsFunctionKeywordOnSameLine
    pub fn next_token_is_function_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == SyntaxKind::FunctionKeyword && !self.has_preceding_line_break()
    }

    // Go: parser/parser.go:4076 nextTokenCanFollowExportModifier
    pub fn next_token_can_follow_export_modifier(&mut self) -> bool {
        self.next_token();
        self.can_follow_export_modifier()
    }

    // Go: parser/parser.go:4081 canFollowExportModifier
    pub fn can_follow_export_modifier(&mut self) -> bool {
        self.token == SyntaxKind::AtToken
            || self.token != SyntaxKind::AsteriskToken
                && self.token != SyntaxKind::AsKeyword
                && self.token != SyntaxKind::OpenBraceToken
                && self.can_follow_modifier()
    }

    // Go: parser/parser.go:4085 canFollowModifier
    pub fn can_follow_modifier(&mut self) -> bool {
        self.token == SyntaxKind::OpenBracketToken
            || self.token == SyntaxKind::OpenBraceToken
            || self.token == SyntaxKind::AsteriskToken
            || self.token == SyntaxKind::DotDotDotToken
            || self.is_literal_property_name()
    }

    // Go: parser/parser.go:4089 canFollowGetOrSetKeyword
    pub fn can_follow_get_or_set_keyword(&mut self) -> bool {
        self.token == SyntaxKind::OpenBracketToken || self.is_literal_property_name()
    }

    // Go: parser/parser.go:4093 nextTokenIsOnSameLineAndCanFollowModifier
    pub fn next_token_is_on_same_line_and_can_follow_modifier(&mut self) -> bool {
        self.next_token();
        if self.has_preceding_line_break() {
            return false;
        }
        self.can_follow_modifier()
    }

    // Go: parser/parser.go:4101 nextTokenIsOpenBrace
    pub fn next_token_is_open_brace(&mut self) -> bool {
        self.next_token() == SyntaxKind::OpenBraceToken
    }

    // Go: parser/parser.go:4105 parseExpression
    pub fn parse_expression(&mut self) -> Node {
        // Expression[in]:
        //      AssignmentExpression[in]
        //      Expression[in] , AssignmentExpression[in]

        // clear the decorator context when parsing Expression, as it should be unambiguous when parsing a decorator
        let save_context_flags = self.context_flags;
        self.context_flags = self.context_flags.without(NodeFlags::DECORATOR_CONTEXT);
        let pos = self.node_pos();
        let mut expr = self.parse_assignment_expression_or_higher();
        loop {
            let operator_token = self.parse_optional_token(SyntaxKind::CommaToken);
            if operator_token.is_nil() {
                break;
            }
            let right = self.parse_assignment_expression_or_higher();
            expr = self.make_binary_expression(expr, operator_token, right, pos);
        }
        self.context_flags = save_context_flags;
        expr
    }

    // Go: parser/parser.go:4126 parseExpressionAllowIn
    pub fn parse_expression_allow_in(&mut self) -> Node {
        self.do_in_context(
            NodeFlags::DISALLOW_IN_CONTEXT,
            false,
            &mut Self::parse_expression,
        )
    }

    // Go: parser/parser.go:4130 parseAssignmentExpressionOrHigher
    pub fn parse_assignment_expression_or_higher(&mut self) -> Node {
        self.parse_assignment_expression_or_higher_worker(
            true, /*allowReturnTypeInArrowFunction*/
        )
    }

    // Go: parser/parser.go:4134 parseAssignmentExpressionOrHigherWorker
    pub fn parse_assignment_expression_or_higher_worker(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Node {
        //  AssignmentExpression[in,yield]:
        //      1) ConditionalExpression[?in,?yield]
        //      2) LeftHandSideExpression = AssignmentExpression[?in,?yield]
        //      3) LeftHandSideExpression AssignmentOperator AssignmentExpression[?in,?yield]
        //      4) ArrowFunctionExpression[?in,?yield]
        //      5) AsyncArrowFunctionExpression[in,yield,await]
        //      6) [+Yield] YieldExpression[?In]
        //
        // Note: for ease of implementation we treat productions '2' and '3' as the same thing.
        // (i.e. they're both BinaryExpressions with an assignment operator in it).
        // First, do the simple check if we have a YieldExpression (production '6').
        if self.is_yield_expression() {
            return self.parse_yield_expression();
        }
        // Then, check if we have an arrow function (production '4' and '5') that starts with a parenthesized
        // parameter list or is an async arrow function.
        // AsyncArrowFunctionExpression:
        //      1) async[no LineTerminator here]AsyncArrowBindingIdentifier[?Yield][no LineTerminator here]=>AsyncConciseBody[?In]
        //      2) CoverCallExpressionAndAsyncArrowHead[?Yield, ?Await][no LineTerminator here]=>AsyncConciseBody[?In]
        // Production (1) of AsyncArrowFunctionExpression is parsed in "tryParseAsyncSimpleArrowFunctionExpression".
        // And production (2) is parsed in "tryParseParenthesizedArrowFunctionExpression".
        //
        // If we do successfully parse arrow-function, we must *not* recurse for productions 1, 2 or 3. An ArrowFunction is
        // not a LeftHandSideExpression, nor does it start a ConditionalExpression.  So we are done
        // with AssignmentExpression if we see one.
        let mut arrow_expression = self
            .try_parse_parenthesized_arrow_function_expression(allow_return_type_in_arrow_function);
        if arrow_expression.is_some() {
            return arrow_expression;
        }
        arrow_expression = self
            .try_parse_async_simple_arrow_function_expression(allow_return_type_in_arrow_function);
        if arrow_expression.is_some() {
            return arrow_expression;
        }
        // Now try to see if we're in production '1', '2' or '3'.  A conditional expression can
        // start with a LogicalOrExpression, while the assignment productions can only start with
        // LeftHandSideExpressions.
        //
        // So, first, we try to just parse out a BinaryExpression.  If we get something that is a
        // LeftHandSide or higher, then we can try to parse out the assignment expression part.
        // Otherwise, we try to parse out the conditional expression bit.  We want to allow any
        // binary expression here, so we pass in the 'lowest' precedence here so that it matches
        // and consumes anything.
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let expr = self.parse_binary_expression_or_higher(OperatorPrecedence::LOWEST);
        // To avoid a look-ahead, we did not handle the case of an arrow function with a single un-parenthesized
        // parameter ('x => ...') above. We handle it here by checking if the parsed expression was a single
        // identifier and the current token is an arrow.
        // PERF: parsefast2. The token test goes first: both tests are pure,
        // and a node kind read is a store access here.
        if self.token == SyntaxKind::EqualsGreaterThanToken && expr.kind() == SyntaxKind::Identifier
        {
            return self.parse_simple_arrow_function_expression(
                pos,
                expr,
                allow_return_type_in_arrow_function,
                jsdoc,
                ModifierList::NIL, /*asyncModifier*/
            );
        }
        // Now see if we might be in cases '2' or '3'.
        // If the expression was a LHS expression, and we have an assignment operator, then
        // we're in '2' or '3'. Consume the assignment and return.
        //
        // Note: we call reScanGreaterToken so that we get an appropriately merged token
        // for cases like `> > =` becoming `>>=`
        // PERF: parsefast2. reScanGreaterToken changes only a `>` token. At
        // any other token that is not an assignment operator the Go test is
        // false, so the test of the token goes first and skips the read of
        // the node kind (a store access here) for most expressions.
        debug_assert_eq!(self.token, self.scanner.token());
        if (self.token == SyntaxKind::GreaterThanToken || is_assignment_operator(self.token))
            && is_left_hand_side_expression(expr)
            && is_assignment_operator(self.re_scan_greater_than_token())
        {
            let operator_token = self.parse_token_node();
            let right = self
                .parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function);
            return self.make_binary_expression(expr, operator_token, right, pos);
        }
        // It wasn't an assignment or a lambda.  This is a conditional expression:
        self.parse_conditional_expression_rest(expr, pos, allow_return_type_in_arrow_function)
    }

    // Go: parser/parser.go:4203 isYieldExpression
    // PERF: parsefast2. The token test is inline; the rest (yield is rare)
    // is out of line.
    #[inline]
    pub fn is_yield_expression(&mut self) -> bool {
        self.token == SyntaxKind::YieldKeyword && self.is_yield_expression_at_yield()
    }

    /// `is_yield_expression` at a `yield` token.
    #[inline(never)]
    fn is_yield_expression_at_yield(&mut self) -> bool {
        if self.token == SyntaxKind::YieldKeyword {
            // If we have a 'yield' keyword, and this is a context where yield expressions are
            // allowed, then definitely parse out a yield expression.
            if self.in_yield_context() {
                return true;
            }

            // We're in a context where 'yield expr' is not allowed.  However, if we can
            // definitely tell that the user was trying to parse a 'yield expr' and not
            // just a normal expr that start with a 'yield' identifier, then parse out
            // a 'yield expr'.  We can then report an error later that they are only
            // allowed in generator expressions.
            //
            // for example, if we see 'yield(foo)', then we'll have to treat that as an
            // invocation expression of something called 'yield'.  However, if we have
            // 'yield foo' then that is not legal as a normal expression, so we can
            // definitely recognize this as a yield expression.
            //
            // for now we just check if the next token is an identifier.  More heuristics
            // can be added here later as necessary.  We just need to make sure that we
            // don't accidentally consume something legal.
            return self.look_ahead(
                &mut Self::next_token_is_identifier_or_keyword_or_literal_on_same_line,
            );
        }
        false
    }
}
