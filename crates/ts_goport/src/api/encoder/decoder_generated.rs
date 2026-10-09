//! Port of Go `api/encoder/decoder_generated.go` (generated in Go by
//! `_scripts/generate-encoder.ts`). One arm per kind, in Go order.

use crate::api::encoder::prelude::*;

use crate::gostd::errors;

impl AstDecoder<'_> {
    // Go: api/encoder/decoder_generated.go:11 (*astDecoder).createStringNode
    pub fn create_string_node(
        &self,
        kind: SyntaxKind,
        data: u32,
        common_data: u8,
    ) -> Result<Node, GoError> {
        let str_idx = data & NODE_DATA_STRING_INDEX_MASK;
        let text = self.get_string(str_idx);

        match kind {
            SyntaxKind::Identifier => Ok(self.factory.new_identifier(text)),
            SyntaxKind::PrivateIdentifier => Ok(self.factory.new_private_identifier(text)),
            SyntaxKind::JsxText => {
                let contains_only_trivia_white_spaces = common_data & 1 != 0;
                Ok(self
                    .factory
                    .new_jsx_text(text, contains_only_trivia_white_spaces))
            }
            SyntaxKind::JsDocText => Ok(self.factory.new_js_doc_text(vec![text])),
            SyntaxKind::JsDocLink => Ok(self.factory.new_js_doc_link(Node::NIL, vec![text])),
            SyntaxKind::JsDocLinkPlain => {
                Ok(self.factory.new_js_doc_link_plain(Node::NIL, vec![text]))
            }
            SyntaxKind::JsDocLinkCode => {
                Ok(self.factory.new_js_doc_link_code(Node::NIL, vec![text]))
            }
            _ => Err(errors::errorf(
                format!("unknown string node kind {}", go_kind_string(kind as i16)),
                Vec::new(),
            )),
        }
    }

    // Go: api/encoder/decoder_generated.go:36 (*astDecoder).createExtendedNode
    pub fn create_extended_node(
        &self,
        kind: SyntaxKind,
        data: u32,
        child_indices: &[i64],
        common_data: u8,
    ) -> Result<Node, GoError> {
        match kind {
            SyntaxKind::StringLiteral => {
                self.decode_extended_data_string_literal(data, child_indices, common_data)
            }
            SyntaxKind::NumericLiteral => {
                self.decode_extended_data_numeric_literal(data, child_indices, common_data)
            }
            SyntaxKind::BigIntLiteral => {
                self.decode_extended_data_big_int_literal(data, child_indices, common_data)
            }
            SyntaxKind::RegularExpressionLiteral => self
                .decode_extended_data_regular_expression_literal(data, child_indices, common_data),
            SyntaxKind::NoSubstitutionTemplateLiteral => self
                .decode_extended_data_no_substitution_template_literal(
                    data,
                    child_indices,
                    common_data,
                ),
            SyntaxKind::TemplateHead => {
                self.decode_extended_data_template_head(data, child_indices, common_data)
            }
            SyntaxKind::TemplateMiddle => {
                self.decode_extended_data_template_middle(data, child_indices, common_data)
            }
            SyntaxKind::TemplateTail => {
                self.decode_extended_data_template_tail(data, child_indices, common_data)
            }
            SyntaxKind::SourceFile => {
                self.decode_extended_data_source_file(data, child_indices, common_data)
            }
            _ => Err(errors::errorf(
                format!(
                    "unknown extended data node kind {}",
                    go_kind_string(kind as i16)
                ),
                Vec::new(),
            )),
        }
    }

    // Go: api/encoder/decoder_generated.go:61 (*astDecoder).createChildrenNode
    pub fn create_children_node(
        &self,
        kind: SyntaxKind,
        data: u32,
        child_indices: &[i64],
        common_data: u8,
    ) -> Result<Node, GoError> {
        let mask = (data & NODE_DATA_CHILD_MASK) as u8;

        match kind {
            SyntaxKind::Unknown
            | SyntaxKind::EndOfFile
            | SyntaxKind::SingleLineCommentTrivia
            | SyntaxKind::MultiLineCommentTrivia
            | SyntaxKind::NewLineTrivia
            | SyntaxKind::WhitespaceTrivia
            | SyntaxKind::ConflictMarkerTrivia
            | SyntaxKind::NonTextFileMarkerTrivia
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::JsxText
            | SyntaxKind::JsxTextAllWhiteSpaces
            | SyntaxKind::RegularExpressionLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead
            | SyntaxKind::TemplateMiddle
            | SyntaxKind::TemplateTail
            | SyntaxKind::OpenBraceToken
            | SyntaxKind::CloseBraceToken
            | SyntaxKind::OpenParenToken
            | SyntaxKind::CloseParenToken
            | SyntaxKind::OpenBracketToken
            | SyntaxKind::CloseBracketToken
            | SyntaxKind::DotToken
            | SyntaxKind::DotDotDotToken
            | SyntaxKind::SemicolonToken
            | SyntaxKind::CommaToken
            | SyntaxKind::QuestionDotToken
            | SyntaxKind::LessThanToken
            | SyntaxKind::LessThanSlashToken
            | SyntaxKind::GreaterThanToken
            | SyntaxKind::LessThanEqualsToken
            | SyntaxKind::GreaterThanEqualsToken
            | SyntaxKind::EqualsEqualsToken
            | SyntaxKind::ExclamationEqualsToken
            | SyntaxKind::EqualsEqualsEqualsToken
            | SyntaxKind::ExclamationEqualsEqualsToken
            | SyntaxKind::EqualsGreaterThanToken
            | SyntaxKind::PlusToken
            | SyntaxKind::MinusToken
            | SyntaxKind::AsteriskToken
            | SyntaxKind::AsteriskAsteriskToken
            | SyntaxKind::SlashToken
            | SyntaxKind::PercentToken
            | SyntaxKind::PlusPlusToken
            | SyntaxKind::MinusMinusToken
            | SyntaxKind::LessThanLessThanToken
            | SyntaxKind::GreaterThanGreaterThanToken
            | SyntaxKind::GreaterThanGreaterThanGreaterThanToken
            | SyntaxKind::AmpersandToken
            | SyntaxKind::BarToken
            | SyntaxKind::CaretToken
            | SyntaxKind::ExclamationToken
            | SyntaxKind::TildeToken
            | SyntaxKind::AmpersandAmpersandToken
            | SyntaxKind::BarBarToken
            | SyntaxKind::QuestionToken
            | SyntaxKind::ColonToken
            | SyntaxKind::AtToken
            | SyntaxKind::QuestionQuestionToken
            | SyntaxKind::BacktickToken
            | SyntaxKind::HashToken
            | SyntaxKind::EqualsToken
            | SyntaxKind::PlusEqualsToken
            | SyntaxKind::MinusEqualsToken
            | SyntaxKind::AsteriskEqualsToken
            | SyntaxKind::AsteriskAsteriskEqualsToken
            | SyntaxKind::SlashEqualsToken
            | SyntaxKind::PercentEqualsToken
            | SyntaxKind::LessThanLessThanEqualsToken
            | SyntaxKind::GreaterThanGreaterThanEqualsToken
            | SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken
            | SyntaxKind::AmpersandEqualsToken
            | SyntaxKind::BarEqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken
            | SyntaxKind::QuestionQuestionEqualsToken
            | SyntaxKind::CaretEqualsToken
            | SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::JsDocCommentTextToken
            | SyntaxKind::BreakKeyword
            | SyntaxKind::CaseKeyword
            | SyntaxKind::CatchKeyword
            | SyntaxKind::ClassKeyword
            | SyntaxKind::ConstKeyword
            | SyntaxKind::ContinueKeyword
            | SyntaxKind::DebuggerKeyword
            | SyntaxKind::DefaultKeyword
            | SyntaxKind::DeleteKeyword
            | SyntaxKind::DoKeyword
            | SyntaxKind::ElseKeyword
            | SyntaxKind::EnumKeyword
            | SyntaxKind::ExportKeyword
            | SyntaxKind::ExtendsKeyword
            | SyntaxKind::FinallyKeyword
            | SyntaxKind::ForKeyword
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::IfKeyword
            | SyntaxKind::InKeyword
            | SyntaxKind::InstanceOfKeyword
            | SyntaxKind::NewKeyword
            | SyntaxKind::ReturnKeyword
            | SyntaxKind::SwitchKeyword
            | SyntaxKind::ThrowKeyword
            | SyntaxKind::TryKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::VarKeyword
            | SyntaxKind::WhileKeyword
            | SyntaxKind::WithKeyword
            | SyntaxKind::ImplementsKeyword
            | SyntaxKind::InterfaceKeyword
            | SyntaxKind::LetKeyword
            | SyntaxKind::PackageKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::PublicKeyword
            | SyntaxKind::StaticKeyword
            | SyntaxKind::YieldKeyword
            | SyntaxKind::AbstractKeyword
            | SyntaxKind::AccessorKeyword
            | SyntaxKind::AsKeyword
            | SyntaxKind::AssertsKeyword
            | SyntaxKind::AssertKeyword
            | SyntaxKind::AsyncKeyword
            | SyntaxKind::AwaitKeyword
            | SyntaxKind::ConstructorKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::GetKeyword
            | SyntaxKind::ImmediateKeyword
            | SyntaxKind::InferKeyword
            | SyntaxKind::IsKeyword
            | SyntaxKind::KeyOfKeyword
            | SyntaxKind::ModuleKeyword
            | SyntaxKind::NamespaceKeyword
            | SyntaxKind::OutKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::RequireKeyword
            | SyntaxKind::SatisfiesKeyword
            | SyntaxKind::SetKeyword
            | SyntaxKind::TypeKeyword
            | SyntaxKind::UniqueKeyword
            | SyntaxKind::UsingKeyword
            | SyntaxKind::FromKeyword
            | SyntaxKind::GlobalKeyword
            | SyntaxKind::OverrideKeyword
            | SyntaxKind::OfKeyword
            | SyntaxKind::DeferKeyword
            // ts#63915
            | SyntaxKind::SourceKeyword => Ok(self.factory.new_token(kind)),
            SyntaxKind::QualifiedName => {
                let mut it = new_child_iter(child_indices);
                let left = self.node_at(it.next_if(mask, 0));
                let right = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_qualified_name(left, right))
            }
            SyntaxKind::ComputedPropertyName => Ok(self
                .factory
                .new_computed_property_name(self.single_child(child_indices))),
            SyntaxKind::Decorator => {
                Ok(self.factory.new_decorator(self.single_child(child_indices)))
            }
            SyntaxKind::EmptyStatement => Ok(self.factory.new_empty_statement()),
            SyntaxKind::IfStatement => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let then_statement = self.node_at(it.next_if(mask, 1));
                let else_statement = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_if_statement(expression, then_statement, else_statement))
            }
            SyntaxKind::DoStatement => {
                let mut it = new_child_iter(child_indices);
                let statement = self.node_at(it.next_if(mask, 0));
                let expression = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_do_statement(statement, expression))
            }
            SyntaxKind::WhileStatement => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let statement = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_while_statement(expression, statement))
            }
            SyntaxKind::ForStatement => {
                let mut it = new_child_iter(child_indices);
                let initializer = self.node_at(it.next_if(mask, 0));
                let condition = self.node_at(it.next_if(mask, 1));
                let incrementor = self.node_at(it.next_if(mask, 2));
                let statement = self.node_at(it.next_if(mask, 3));
                Ok(self
                    .factory
                    .new_for_statement(initializer, condition, incrementor, statement))
            }
            SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
                let mut it = new_child_iter(child_indices);
                let await_modifier = self.node_at(it.next_if(mask, 0));
                let initializer = self.node_at(it.next_if(mask, 1));
                let expression = self.node_at(it.next_if(mask, 2));
                let statement = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_for_in_or_of_statement(
                    kind,
                    await_modifier,
                    initializer,
                    expression,
                    statement,
                ))
            }
            SyntaxKind::BreakStatement => Ok(self
                .factory
                .new_break_statement(self.single_child(child_indices))),
            SyntaxKind::ContinueStatement => Ok(self
                .factory
                .new_continue_statement(self.single_child(child_indices))),
            SyntaxKind::ReturnStatement => Ok(self
                .factory
                .new_return_statement(self.single_child(child_indices))),
            SyntaxKind::WithStatement => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let statement = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_with_statement(expression, statement))
            }
            SyntaxKind::SwitchStatement => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let case_block = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_switch_statement(expression, case_block))
            }
            SyntaxKind::CaseBlock => Ok(self
                .factory
                .new_case_block(self.single_node_list_child(child_indices))),
            SyntaxKind::CaseClause | SyntaxKind::DefaultClause => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let statements = self.node_list_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_case_or_default_clause(kind, expression, statements))
            }
            SyntaxKind::ThrowStatement => Ok(self
                .factory
                .new_throw_statement(self.single_child(child_indices))),
            SyntaxKind::TryStatement => {
                let mut it = new_child_iter(child_indices);
                let try_block = self.node_at(it.next_if(mask, 0));
                let catch_clause = self.node_at(it.next_if(mask, 1));
                let finally_block = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_try_statement(try_block, catch_clause, finally_block))
            }
            SyntaxKind::CatchClause => {
                let mut it = new_child_iter(child_indices);
                let variable_declaration = self.node_at(it.next_if(mask, 0));
                let block = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_catch_clause(variable_declaration, block))
            }
            SyntaxKind::DebuggerStatement => Ok(self.factory.new_debugger_statement()),
            SyntaxKind::LabeledStatement => {
                let mut it = new_child_iter(child_indices);
                let label = self.node_at(it.next_if(mask, 0));
                let statement = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_labeled_statement(label, statement))
            }
            SyntaxKind::ExpressionStatement => Ok(self
                .factory
                .new_expression_statement(self.single_child(child_indices))),
            SyntaxKind::Block => {
                let multi_line = common_data & 1 != 0;
                let mut list = NodeList::NIL;
                if !child_indices.is_empty() {
                    list = self.node_list_at(child_indices[0]);
                }
                Ok(self.factory.new_block(list, multi_line))
            }
            SyntaxKind::VariableStatement => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let declaration_list = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_variable_statement(modifiers, declaration_list))
            }
            SyntaxKind::VariableDeclaration => {
                let mut it = new_child_iter(child_indices);
                let name = self.node_at(it.next_if(mask, 0));
                let exclamation_token = self.node_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                let initializer = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_variable_declaration(
                    name,
                    exclamation_token,
                    type_node,
                    initializer,
                ))
            }
            SyntaxKind::VariableDeclarationList => Ok(self.factory.new_variable_declaration_list(
                self.single_node_list_child(child_indices),
                NodeFlags::NONE,
            )),
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => Ok(self
                .factory
                .new_binding_pattern(kind, self.single_node_list_child(child_indices))),
            SyntaxKind::Parameter => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let dot_dot_dot_token = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let question_token = self.node_at(it.next_if(mask, 3));
                let type_node = self.node_at(it.next_if(mask, 4));
                let initializer = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_parameter_declaration(
                    modifiers,
                    dot_dot_dot_token,
                    name,
                    question_token,
                    type_node,
                    initializer,
                ))
            }
            SyntaxKind::BindingElement => {
                let mut it = new_child_iter(child_indices);
                let dot_dot_dot_token = self.node_at(it.next_if(mask, 0));
                let property_name = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let initializer = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_binding_element(
                    dot_dot_dot_token,
                    property_name,
                    name,
                    initializer,
                ))
            }
            SyntaxKind::MissingDeclaration => {
                let mut mods = ModifierList::NIL;
                if !child_indices.is_empty() {
                    mods = self.modifier_list_at(child_indices[0]);
                }
                Ok(self.factory.new_missing_declaration(mods))
            }
            SyntaxKind::FunctionDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let asterisk_token = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let type_parameters = self.node_list_at(it.next_if(mask, 3));
                let parameters = self.node_list_at(it.next_if(mask, 4));
                let type_node = self.node_at(it.next_if(mask, 5));
                let body = self.node_at(it.next_if(mask, 6));
                Ok(self.factory.new_function_declaration(
                    modifiers,
                    asterisk_token,
                    name,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::ClassDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let heritage_clauses = self.node_list_at(it.next_if(mask, 3));
                let members = self.node_list_at(it.next_if(mask, 4));
                Ok(self.factory.new_class_declaration(
                    modifiers,
                    name,
                    type_parameters,
                    heritage_clauses,
                    members,
                ))
            }
            SyntaxKind::ClassExpression => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let heritage_clauses = self.node_list_at(it.next_if(mask, 3));
                let members = self.node_list_at(it.next_if(mask, 4));
                Ok(self.factory.new_class_expression(
                    modifiers,
                    name,
                    type_parameters,
                    heritage_clauses,
                    members,
                ))
            }
            SyntaxKind::HeritageClause => {
                let mut token = SyntaxKind::ExtendsKeyword;
                if common_data & 1 != 0 {
                    token = SyntaxKind::ImplementsKeyword;
                }
                Ok(self
                    .factory
                    .new_heritage_clause(token, self.single_node_list_child(child_indices)))
            }
            SyntaxKind::InterfaceDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let heritage_clauses = self.node_list_at(it.next_if(mask, 3));
                let members = self.node_list_at(it.next_if(mask, 4));
                Ok(self.factory.new_interface_declaration(
                    modifiers,
                    name,
                    type_parameters,
                    heritage_clauses,
                    members,
                ))
            }
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                if kind == SyntaxKind::JsTypeAliasDeclaration {
                    return Ok(self.factory.new_js_type_alias_declaration(
                        modifiers,
                        name,
                        type_parameters,
                        type_node,
                    ));
                }
                Ok(self.factory.new_type_alias_declaration(
                    modifiers,
                    name,
                    type_parameters,
                    type_node,
                ))
            }
            SyntaxKind::EnumMember => {
                let mut it = new_child_iter(child_indices);
                let name = self.node_at(it.next_if(mask, 0));
                let initializer = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_enum_member(name, initializer))
            }
            SyntaxKind::EnumDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let members = self.node_list_at(it.next_if(mask, 2));
                Ok(self.factory.new_enum_declaration(modifiers, name, members))
            }
            SyntaxKind::ModuleBlock => Ok(self
                .factory
                .new_module_block(self.single_node_list_child(child_indices))),
            SyntaxKind::NotEmittedStatement => Ok(self.factory.new_not_emitted_statement()),
            SyntaxKind::NotEmittedTypeElement => Ok(self.factory.new_not_emitted_type_element()),
            SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let import_clause = self.node_at(it.next_if(mask, 1));
                let module_specifier = self.node_at(it.next_if(mask, 2));
                let attributes = self.node_at(it.next_if(mask, 3));
                if kind == SyntaxKind::JsImportDeclaration {
                    return Ok(self.factory.new_js_import_declaration(
                        modifiers,
                        import_clause,
                        module_specifier,
                        attributes,
                    ));
                }
                Ok(self.factory.new_import_declaration(
                    modifiers,
                    import_clause,
                    module_specifier,
                    attributes,
                ))
            }
            SyntaxKind::ExternalModuleReference => Ok(self
                .factory
                .new_external_module_reference(self.single_child(child_indices))),
            SyntaxKind::NamespaceImport => Ok(self
                .factory
                .new_namespace_import(self.single_child(child_indices))),
            SyntaxKind::NamedImports => Ok(self
                .factory
                .new_named_imports(self.single_node_list_child(child_indices))),
            SyntaxKind::ExportAssignment => {
                let is_export_equals = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let type_node = self.node_at(it.next_if(mask, 1));
                let expression = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_export_assignment(
                    modifiers,
                    is_export_equals,
                    type_node,
                    expression,
                ))
            }
            SyntaxKind::NamespaceExportDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_namespace_export_declaration(modifiers, name))
            }
            SyntaxKind::NamespaceExport => Ok(self
                .factory
                .new_namespace_export(self.single_child(child_indices))),
            SyntaxKind::NamedExports => Ok(self
                .factory
                .new_named_exports(self.single_node_list_child(child_indices))),
            SyntaxKind::ExportSpecifier => {
                let is_type_only = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let property_name = self.node_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_export_specifier(is_type_only, property_name, name))
            }
            SyntaxKind::CallSignature => {
                let mut it = new_child_iter(child_indices);
                let type_parameters = self.node_list_at(it.next_if(mask, 0));
                let parameters = self.node_list_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_call_signature_declaration(
                    type_parameters,
                    parameters,
                    type_node,
                ))
            }
            SyntaxKind::ConstructSignature => {
                let mut it = new_child_iter(child_indices);
                let type_parameters = self.node_list_at(it.next_if(mask, 0));
                let parameters = self.node_list_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_construct_signature_declaration(
                    type_parameters,
                    parameters,
                    type_node,
                ))
            }
            SyntaxKind::Constructor => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let type_parameters = self.node_list_at(it.next_if(mask, 1));
                let parameters = self.node_list_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let body = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_constructor_declaration(
                    modifiers,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::GetAccessor => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let parameters = self.node_list_at(it.next_if(mask, 3));
                let type_node = self.node_at(it.next_if(mask, 4));
                let body = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_get_accessor_declaration(
                    modifiers,
                    name,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::SetAccessor => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let parameters = self.node_list_at(it.next_if(mask, 3));
                let type_node = self.node_at(it.next_if(mask, 4));
                let body = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_set_accessor_declaration(
                    modifiers,
                    name,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::IndexSignature => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let parameters = self.node_list_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_index_signature_declaration(modifiers, parameters, type_node))
            }
            SyntaxKind::MethodSignature => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let postfix_token = self.node_at(it.next_if(mask, 2));
                let type_parameters = self.node_list_at(it.next_if(mask, 3));
                let parameters = self.node_list_at(it.next_if(mask, 4));
                let type_node = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_method_signature_declaration(
                    modifiers,
                    name,
                    postfix_token,
                    type_parameters,
                    parameters,
                    type_node,
                ))
            }
            SyntaxKind::MethodDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let asterisk_token = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let postfix_token = self.node_at(it.next_if(mask, 3));
                let type_parameters = self.node_list_at(it.next_if(mask, 4));
                let parameters = self.node_list_at(it.next_if(mask, 5));
                let type_node = self.node_at(it.next_if(mask, 6));
                let body = self.node_at(it.next_if(mask, 7));
                Ok(self.factory.new_method_declaration(
                    modifiers,
                    asterisk_token,
                    name,
                    postfix_token,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::PropertySignature => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let postfix_token = self.node_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let initializer = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_property_signature_declaration(
                    modifiers,
                    name,
                    postfix_token,
                    type_node,
                    initializer,
                ))
            }
            SyntaxKind::PropertyDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let postfix_token = self.node_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let initializer = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_property_declaration(
                    modifiers,
                    name,
                    postfix_token,
                    type_node,
                    initializer,
                ))
            }
            SyntaxKind::SemicolonClassElement => Ok(self.factory.new_semicolon_class_element()),
            SyntaxKind::ClassStaticBlockDeclaration => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let body = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_class_static_block_declaration(modifiers, body))
            }
            SyntaxKind::OmittedExpression => Ok(self.factory.new_omitted_expression()),
            SyntaxKind::FalseKeyword
            | SyntaxKind::ImportKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::ThisKeyword
            | SyntaxKind::TrueKeyword => Ok(self.factory.new_keyword_expression(kind)),
            SyntaxKind::BinaryExpression => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let left = self.node_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                let operator_token = self.node_at(it.next_if(mask, 3));
                let right = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_binary_expression(
                    modifiers,
                    left,
                    type_node,
                    operator_token,
                    right,
                ))
            }
            SyntaxKind::PrefixUnaryExpression => {
                let mut operator = SyntaxKind::Unknown;
                match common_data & 7 {
                    0 => operator = SyntaxKind::PlusToken,
                    1 => operator = SyntaxKind::MinusToken,
                    2 => operator = SyntaxKind::TildeToken,
                    3 => operator = SyntaxKind::ExclamationToken,
                    4 => operator = SyntaxKind::PlusPlusToken,
                    5 => operator = SyntaxKind::MinusMinusToken,
                    _ => {}
                }
                Ok(self
                    .factory
                    .new_prefix_unary_expression(operator, self.single_child(child_indices)))
            }
            SyntaxKind::PostfixUnaryExpression => {
                let mut operator = SyntaxKind::PlusPlusToken;
                if common_data & 1 != 0 {
                    operator = SyntaxKind::MinusMinusToken;
                }
                Ok(self
                    .factory
                    .new_postfix_unary_expression(self.single_child(child_indices), operator))
            }
            SyntaxKind::YieldExpression => {
                let mut it = new_child_iter(child_indices);
                let asterisk_token = self.node_at(it.next_if(mask, 0));
                let expression = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_yield_expression(asterisk_token, expression))
            }
            SyntaxKind::ArrowFunction => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let type_parameters = self.node_list_at(it.next_if(mask, 1));
                let parameters = self.node_list_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let equals_greater_than_token = self.node_at(it.next_if(mask, 4));
                let body = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_arrow_function(
                    modifiers,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    equals_greater_than_token,
                    body,
                ))
            }
            SyntaxKind::FunctionExpression => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let asterisk_token = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let type_parameters = self.node_list_at(it.next_if(mask, 3));
                let parameters = self.node_list_at(it.next_if(mask, 4));
                let type_node = self.node_at(it.next_if(mask, 5));
                let body = self.node_at(it.next_if(mask, 6));
                Ok(self.factory.new_function_expression(
                    modifiers,
                    asterisk_token,
                    name,
                    type_parameters,
                    parameters,
                    type_node,
                    Node::NIL,
                    body,
                ))
            }
            SyntaxKind::AsExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let type_node = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_as_expression(expression, type_node))
            }
            SyntaxKind::SatisfiesExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let type_node = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_satisfies_expression(expression, type_node))
            }
            SyntaxKind::ConditionalExpression => {
                let mut it = new_child_iter(child_indices);
                let condition = self.node_at(it.next_if(mask, 0));
                let question_token = self.node_at(it.next_if(mask, 1));
                let when_true = self.node_at(it.next_if(mask, 2));
                let colon_token = self.node_at(it.next_if(mask, 3));
                let when_false = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_conditional_expression(
                    condition,
                    question_token,
                    when_true,
                    colon_token,
                    when_false,
                ))
            }
            SyntaxKind::PropertyAccessExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let question_dot_token = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_property_access_expression(
                    expression,
                    question_dot_token,
                    name,
                    NodeFlags::NONE,
                ))
            }
            SyntaxKind::ElementAccessExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let question_dot_token = self.node_at(it.next_if(mask, 1));
                let argument_expression = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_element_access_expression(
                    expression,
                    question_dot_token,
                    argument_expression,
                    NodeFlags::NONE,
                ))
            }
            SyntaxKind::CallExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let question_dot_token = self.node_at(it.next_if(mask, 1));
                let type_arguments = self.node_list_at(it.next_if(mask, 2));
                let arguments = self.node_list_at(it.next_if(mask, 3));
                Ok(self.factory.new_call_expression(
                    expression,
                    question_dot_token,
                    type_arguments,
                    arguments,
                    NodeFlags::NONE,
                ))
            }
            SyntaxKind::NewExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                let arguments = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_new_expression(expression, type_arguments, arguments))
            }
            SyntaxKind::MetaProperty => {
                let mut keyword_token = SyntaxKind::ImportKeyword;
                if common_data & 1 != 0 {
                    keyword_token = SyntaxKind::NewKeyword;
                }
                Ok(self
                    .factory
                    .new_meta_property(keyword_token, self.single_child(child_indices)))
            }
            SyntaxKind::NonNullExpression => Ok(self
                .factory
                .new_non_null_expression(self.single_child(child_indices), NodeFlags::NONE)),
            SyntaxKind::SpreadElement => Ok(self
                .factory
                .new_spread_element(self.single_child(child_indices))),
            SyntaxKind::TemplateExpression => {
                let mut it = new_child_iter(child_indices);
                let head = self.node_at(it.next_if(mask, 0));
                let template_spans = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_template_expression(head, template_spans))
            }
            SyntaxKind::TemplateSpan => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let literal = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_template_span(expression, literal))
            }
            SyntaxKind::TaggedTemplateExpression => {
                let mut it = new_child_iter(child_indices);
                let tag = self.node_at(it.next_if(mask, 0));
                let question_dot_token = self.node_at(it.next_if(mask, 1));
                let type_arguments = self.node_list_at(it.next_if(mask, 2));
                let template = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_tagged_template_expression(
                    tag,
                    question_dot_token,
                    type_arguments,
                    template,
                    NodeFlags::NONE,
                ))
            }
            SyntaxKind::ParenthesizedExpression => Ok(self
                .factory
                .new_parenthesized_expression(self.single_child(child_indices))),
            SyntaxKind::ArrayLiteralExpression => {
                let multi_line = common_data & 1 != 0;
                let mut list = NodeList::NIL;
                if !child_indices.is_empty() {
                    list = self.node_list_at(child_indices[0]);
                }
                Ok(self.factory.new_array_literal_expression(list, multi_line))
            }
            SyntaxKind::ObjectLiteralExpression => {
                let multi_line = common_data & 1 != 0;
                let mut list = NodeList::NIL;
                if !child_indices.is_empty() {
                    list = self.node_list_at(child_indices[0]);
                }
                Ok(self.factory.new_object_literal_expression(list, multi_line))
            }
            SyntaxKind::SpreadAssignment => Ok(self
                .factory
                .new_spread_assignment(self.single_child(child_indices))),
            SyntaxKind::PropertyAssignment => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let postfix_token = self.node_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let initializer = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_property_assignment(
                    modifiers,
                    name,
                    postfix_token,
                    type_node,
                    initializer,
                ))
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let postfix_token = self.node_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                let equals_token = self.node_at(it.next_if(mask, 4));
                let object_assignment_initializer = self.node_at(it.next_if(mask, 5));
                Ok(self.factory.new_shorthand_property_assignment(
                    modifiers,
                    name,
                    postfix_token,
                    type_node,
                    equals_token,
                    object_assignment_initializer,
                ))
            }
            SyntaxKind::DeleteExpression => Ok(self
                .factory
                .new_delete_expression(self.single_child(child_indices))),
            SyntaxKind::TypeOfExpression => Ok(self
                .factory
                .new_type_of_expression(self.single_child(child_indices))),
            SyntaxKind::VoidExpression => Ok(self
                .factory
                .new_void_expression(self.single_child(child_indices))),
            SyntaxKind::AwaitExpression => Ok(self
                .factory
                .new_await_expression(self.single_child(child_indices))),
            SyntaxKind::TypeAssertionExpression => {
                let mut it = new_child_iter(child_indices);
                let type_node = self.node_at(it.next_if(mask, 0));
                let expression = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_type_assertion(type_node, expression))
            }
            SyntaxKind::VoidKeyword
            | SyntaxKind::AnyKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::IntrinsicKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::ObjectKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::UndefinedKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::BigIntKeyword => Ok(self.factory.new_keyword_type_node(kind)),
            SyntaxKind::UnionType => Ok(self
                .factory
                .new_union_type_node(self.single_node_list_child(child_indices))),
            SyntaxKind::IntersectionType => Ok(self
                .factory
                .new_intersection_type_node(self.single_node_list_child(child_indices))),
            SyntaxKind::ConditionalType => {
                let mut it = new_child_iter(child_indices);
                let check_type = self.node_at(it.next_if(mask, 0));
                let extends_type = self.node_at(it.next_if(mask, 1));
                let true_type = self.node_at(it.next_if(mask, 2));
                let false_type = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_conditional_type_node(
                    check_type,
                    extends_type,
                    true_type,
                    false_type,
                ))
            }
            SyntaxKind::TypeOperator => {
                let mut operator = SyntaxKind::Unknown;
                match common_data & 3 {
                    0 => operator = SyntaxKind::KeyOfKeyword,
                    1 => operator = SyntaxKind::ReadonlyKeyword,
                    2 => operator = SyntaxKind::UniqueKeyword,
                    _ => {}
                }
                Ok(self
                    .factory
                    .new_type_operator_node(operator, self.single_child(child_indices)))
            }
            SyntaxKind::InferType => Ok(self
                .factory
                .new_infer_type_node(self.single_child(child_indices))),
            SyntaxKind::ArrayType => Ok(self
                .factory
                .new_array_type_node(self.single_child(child_indices))),
            SyntaxKind::IndexedAccessType => {
                let mut it = new_child_iter(child_indices);
                let object_type = self.node_at(it.next_if(mask, 0));
                let index_type = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_indexed_access_type_node(object_type, index_type))
            }
            SyntaxKind::TypeReference => {
                let mut it = new_child_iter(child_indices);
                let type_name = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_type_reference_node(type_name, type_arguments))
            }
            SyntaxKind::ExpressionWithTypeArguments => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_expression_with_type_arguments(expression, type_arguments))
            }
            SyntaxKind::LiteralType => Ok(self
                .factory
                .new_literal_type_node(self.single_child(child_indices))),
            SyntaxKind::ThisType => Ok(self.factory.new_this_type_node()),
            SyntaxKind::TypePredicate => {
                let mut it = new_child_iter(child_indices);
                let asserts_modifier = self.node_at(it.next_if(mask, 0));
                let parameter_name = self.node_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_type_predicate_node(
                    asserts_modifier,
                    parameter_name,
                    type_node,
                ))
            }
            SyntaxKind::ImportAttribute => {
                let mut it = new_child_iter(child_indices);
                let name = self.node_at(it.next_if(mask, 0));
                let value = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_import_attribute(name, value))
            }
            SyntaxKind::ImportAttributes => {
                let multi_line = common_data & 1 != 0;
                let mut token = SyntaxKind::WithKeyword;
                if (common_data >> 1) & 1 != 0 {
                    token = SyntaxKind::AssertKeyword;
                }
                let mut list = NodeList::NIL;
                if !child_indices.is_empty() {
                    list = self.node_list_at(child_indices[0]);
                }
                Ok(self.factory.new_import_attributes(token, list, multi_line))
            }
            SyntaxKind::TypeQuery => {
                let mut it = new_child_iter(child_indices);
                let expr_name = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_type_query_node(expr_name, type_arguments))
            }
            SyntaxKind::MappedType => {
                let mut it = new_child_iter(child_indices);
                let readonly_token = self.node_at(it.next_if(mask, 0));
                let type_parameter = self.node_at(it.next_if(mask, 1));
                let name_type = self.node_at(it.next_if(mask, 2));
                let question_token = self.node_at(it.next_if(mask, 3));
                let type_node = self.node_at(it.next_if(mask, 4));
                let members = self.node_list_at(it.next_if(mask, 5));
                Ok(self.factory.new_mapped_type_node(
                    readonly_token,
                    type_parameter,
                    name_type,
                    question_token,
                    type_node,
                    members,
                ))
            }
            SyntaxKind::TypeLiteral => Ok(self
                .factory
                .new_type_literal_node(self.single_node_list_child(child_indices))),
            SyntaxKind::TupleType => Ok(self
                .factory
                .new_tuple_type_node(self.single_node_list_child(child_indices))),
            SyntaxKind::NamedTupleMember => {
                let mut it = new_child_iter(child_indices);
                let dot_dot_dot_token = self.node_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let question_token = self.node_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_named_tuple_member(
                    dot_dot_dot_token,
                    name,
                    question_token,
                    type_node,
                ))
            }
            SyntaxKind::OptionalType => Ok(self
                .factory
                .new_optional_type_node(self.single_child(child_indices))),
            SyntaxKind::RestType => Ok(self
                .factory
                .new_rest_type_node(self.single_child(child_indices))),
            SyntaxKind::ParenthesizedType => Ok(self
                .factory
                .new_parenthesized_type_node(self.single_child(child_indices))),
            SyntaxKind::FunctionType => {
                let mut it = new_child_iter(child_indices);
                let type_parameters = self.node_list_at(it.next_if(mask, 0));
                let parameters = self.node_list_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_function_type_node(type_parameters, parameters, type_node))
            }
            SyntaxKind::ConstructorType => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let type_parameters = self.node_list_at(it.next_if(mask, 1));
                let parameters = self.node_list_at(it.next_if(mask, 2));
                let type_node = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_constructor_type_node(
                    modifiers,
                    type_parameters,
                    parameters,
                    type_node,
                ))
            }
            SyntaxKind::TemplateLiteralType => {
                let mut it = new_child_iter(child_indices);
                let head = self.node_at(it.next_if(mask, 0));
                let template_spans = self.node_list_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_template_literal_type_node(head, template_spans))
            }
            SyntaxKind::TemplateLiteralTypeSpan => {
                let mut it = new_child_iter(child_indices);
                let type_node = self.node_at(it.next_if(mask, 0));
                let literal = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_template_literal_type_span(type_node, literal))
            }
            SyntaxKind::SyntheticExpression => {
                let (type_node, is_spread) =
                    decode_node_common_data_synthetic_expression(common_data);
                Ok(self.factory.new_synthetic_expression(
                    type_node,
                    is_spread,
                    self.single_child(child_indices),
                ))
            }
            SyntaxKind::PartiallyEmittedExpression => Ok(self
                .factory
                .new_partially_emitted_expression(self.single_child(child_indices))),
            SyntaxKind::JsxElement => {
                let mut it = new_child_iter(child_indices);
                let opening_element = self.node_at(it.next_if(mask, 0));
                let children = self.node_list_at(it.next_if(mask, 1));
                let closing_element = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_jsx_element(opening_element, children, closing_element))
            }
            SyntaxKind::JsxAttributes => Ok(self
                .factory
                .new_jsx_attributes(self.single_node_list_child(child_indices))),
            SyntaxKind::JsxNamespacedName => {
                let mut it = new_child_iter(child_indices);
                let namespace = self.node_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_jsx_namespaced_name(namespace, name))
            }
            SyntaxKind::JsxOpeningElement => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                let attributes = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_jsx_opening_element(tag_name, type_arguments, attributes))
            }
            SyntaxKind::JsxSelfClosingElement => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_arguments = self.node_list_at(it.next_if(mask, 1));
                let attributes = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_jsx_self_closing_element(tag_name, type_arguments, attributes))
            }
            SyntaxKind::JsxFragment => {
                let mut it = new_child_iter(child_indices);
                let opening_fragment = self.node_at(it.next_if(mask, 0));
                let children = self.node_list_at(it.next_if(mask, 1));
                let closing_fragment = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_jsx_fragment(opening_fragment, children, closing_fragment))
            }
            SyntaxKind::JsxOpeningFragment => Ok(self.factory.new_jsx_opening_fragment()),
            SyntaxKind::JsxClosingFragment => Ok(self.factory.new_jsx_closing_fragment()),
            SyntaxKind::JsxAttribute => {
                let mut it = new_child_iter(child_indices);
                let name = self.node_at(it.next_if(mask, 0));
                let initializer = self.node_at(it.next_if(mask, 1));
                Ok(self.factory.new_jsx_attribute(name, initializer))
            }
            SyntaxKind::JsxSpreadAttribute => Ok(self
                .factory
                .new_jsx_spread_attribute(self.single_child(child_indices))),
            SyntaxKind::JsxClosingElement => Ok(self
                .factory
                .new_jsx_closing_element(self.single_child(child_indices))),
            SyntaxKind::JsxExpression => {
                let mut it = new_child_iter(child_indices);
                let dot_dot_dot_token = self.node_at(it.next_if(mask, 0));
                let expression = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_jsx_expression(dot_dot_dot_token, expression))
            }
            SyntaxKind::SyntaxList => {
                // PORT: `allocNodeSlice` returns an empty slice, so Go panics here (index
                // out of range) when there is a child. The Rust index panics the same way.
                let mut nodes = self.alloc_node_slice(child_indices.len() as i64);
                for (i, &ci) in child_indices.iter().enumerate() {
                    nodes[i] = self.nodes[ci as usize];
                }
                Ok(self.factory.new_syntax_list(&nodes))
            }
            SyntaxKind::JsDoc => {
                let mut it = new_child_iter(child_indices);
                let comment = self.node_list_at(it.next_if(mask, 0));
                let tags = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc(comment, tags))
            }
            SyntaxKind::JsDocTypeExpression => Ok(self
                .factory
                .new_js_doc_type_expression(self.single_child(child_indices))),
            SyntaxKind::JsDocNonNullableType => Ok(self
                .factory
                .new_js_doc_non_nullable_type(self.single_child(child_indices))),
            SyntaxKind::JsDocNullableType => Ok(self
                .factory
                .new_js_doc_nullable_type(self.single_child(child_indices))),
            SyntaxKind::JsDocAllType => Ok(self.factory.new_js_doc_all_type()),
            SyntaxKind::JsDocVariadicType => Ok(self
                .factory
                .new_js_doc_variadic_type(self.single_child(child_indices))),
            SyntaxKind::JsDocOptionalType => Ok(self
                .factory
                .new_js_doc_optional_type(self.single_child(child_indices))),
            SyntaxKind::JsDocTypeTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_type_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocUnknownTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_unknown_tag(tag_name, comment))
            }
            SyntaxKind::JsDocTemplateTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let constraint = self.node_at(it.next_if(mask, 1));
                let type_parameters = self.node_list_at(it.next_if(mask, 2));
                let comment = self.node_list_at(it.next_if(mask, 3));
                Ok(self.factory.new_js_doc_template_tag(
                    tag_name,
                    constraint,
                    type_parameters,
                    comment,
                ))
            }
            SyntaxKind::JsDocReturnTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_return_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocPublicTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_public_tag(tag_name, comment))
            }
            SyntaxKind::JsDocPrivateTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_private_tag(tag_name, comment))
            }
            SyntaxKind::JsDocProtectedTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_protected_tag(tag_name, comment))
            }
            SyntaxKind::JsDocReadonlyTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_readonly_tag(tag_name, comment))
            }
            SyntaxKind::JsDocOverrideTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_override_tag(tag_name, comment))
            }
            SyntaxKind::JsDocDeprecatedTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let comment = self.node_list_at(it.next_if(mask, 1));
                Ok(self.factory.new_js_doc_deprecated_tag(tag_name, comment))
            }
            SyntaxKind::JsDocSeeTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let name_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_see_tag(tag_name, name_expression, comment))
            }
            SyntaxKind::JsDocImplementsTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let class_name = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_implements_tag(tag_name, class_name, comment))
            }
            SyntaxKind::JsDocAugmentsTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let class_name = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_augments_tag(tag_name, class_name, comment))
            }
            SyntaxKind::JsDocSatisfiesTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_satisfies_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocThrowsTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_throws_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocThisTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_this_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocImportTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let import_clause = self.node_at(it.next_if(mask, 1));
                let module_specifier = self.node_at(it.next_if(mask, 2));
                let attributes = self.node_at(it.next_if(mask, 3));
                let comment = self.node_list_at(it.next_if(mask, 4));
                Ok(self.factory.new_js_doc_import_tag(
                    tag_name,
                    import_clause,
                    module_specifier,
                    attributes,
                    comment,
                ))
            }
            SyntaxKind::JsDocCallbackTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let comment = self.node_list_at(it.next_if(mask, 3));
                Ok(self
                    .factory
                    .new_js_doc_callback_tag(tag_name, type_expression, name, comment))
            }
            SyntaxKind::JsDocOverloadTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let comment = self.node_list_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_overload_tag(tag_name, type_expression, comment))
            }
            SyntaxKind::JsDocTypedefTag => {
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let type_expression = self.node_at(it.next_if(mask, 1));
                let name = self.node_at(it.next_if(mask, 2));
                let comment = self.node_list_at(it.next_if(mask, 3));
                Ok(self
                    .factory
                    .new_js_doc_typedef_tag(tag_name, type_expression, name, comment))
            }
            SyntaxKind::JsDocSignature => {
                let mut it = new_child_iter(child_indices);
                let type_parameters = self.node_list_at(it.next_if(mask, 0));
                let parameters = self.node_list_at(it.next_if(mask, 1));
                let type_node = self.node_at(it.next_if(mask, 2));
                Ok(self
                    .factory
                    .new_js_doc_signature(type_parameters, parameters, type_node))
            }
            SyntaxKind::JsDocNameReference => Ok(self
                .factory
                .new_js_doc_name_reference(self.single_child(child_indices))),
            SyntaxKind::ModuleDeclaration => {
                let mut keyword = SyntaxKind::ModuleKeyword;
                if common_data & 1 != 0 {
                    keyword = SyntaxKind::NamespaceKeyword;
                }
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let attributes = self.node_at(it.next_if(mask, 2));
                let body = self.node_at(it.next_if(mask, 3));
                Ok(self
                    .factory
                    .new_module_declaration(modifiers, keyword, name, attributes, body))
            }
            SyntaxKind::ImportEqualsDeclaration => {
                let is_type_only = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let module_reference = self.node_at(it.next_if(mask, 2));
                Ok(self.factory.new_import_equals_declaration(
                    modifiers,
                    is_type_only,
                    name,
                    module_reference,
                ))
            }
            SyntaxKind::ExportDeclaration => {
                let is_type_only = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let export_clause = self.node_at(it.next_if(mask, 1));
                let module_specifier = self.node_at(it.next_if(mask, 2));
                let attributes = self.node_at(it.next_if(mask, 3));
                Ok(self.factory.new_export_declaration(
                    modifiers,
                    is_type_only,
                    export_clause,
                    module_specifier,
                    attributes,
                ))
            }
            SyntaxKind::ImportType => {
                let is_type_of = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let argument = self.node_at(it.next_if(mask, 0));
                let attributes = self.node_at(it.next_if(mask, 1));
                let qualifier = self.node_at(it.next_if(mask, 2));
                let type_arguments = self.node_list_at(it.next_if(mask, 3));
                Ok(self.factory.new_import_type_node(
                    is_type_of,
                    argument,
                    attributes,
                    qualifier,
                    type_arguments,
                ))
            }
            SyntaxKind::ImportClause => {
                let mut phase_modifier = SyntaxKind::Unknown;
                match common_data & 3 {
                    1 => phase_modifier = SyntaxKind::TypeKeyword,
                    2 => phase_modifier = SyntaxKind::DeferKeyword,
                    // ts#63915
                    3 => phase_modifier = SyntaxKind::SourceKeyword,
                    _ => {}
                }
                let mut it = new_child_iter(child_indices);
                let name = self.node_at(it.next_if(mask, 0));
                let named_bindings = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_import_clause(phase_modifier, name, named_bindings))
            }
            SyntaxKind::ImportSpecifier => {
                let is_type_only = common_data & 1 != 0;
                let mut it = new_child_iter(child_indices);
                let property_name = self.node_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_import_specifier(is_type_only, property_name, name))
            }
            SyntaxKind::TypeParameter => {
                let mut it = new_child_iter(child_indices);
                let modifiers = self.modifier_list_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let constraint = self.node_at(it.next_if(mask, 2));
                let expression = self.node_at(it.next_if(mask, 3));
                let default_type = self.node_at(it.next_if(mask, 4));
                Ok(self.factory.new_type_parameter_declaration(
                    modifiers,
                    name,
                    constraint,
                    expression,
                    default_type,
                ))
            }
            SyntaxKind::SyntheticReferenceExpression => {
                let mut it = new_child_iter(child_indices);
                let expression = self.node_at(it.next_if(mask, 0));
                let this_arg = self.node_at(it.next_if(mask, 1));
                Ok(self
                    .factory
                    .new_synthetic_reference_expression(expression, this_arg))
            }
            SyntaxKind::JsDocTypeLiteral => {
                let is_array_type = common_data & 1 != 0;
                // PORT: `allocNodeSlice` returns an empty slice, so Go panics here (index
                // out of range) when there is a child. The Rust index panics the same way.
                let mut nodes = self.alloc_node_slice(child_indices.len() as i64);
                for (i, &ci) in child_indices.iter().enumerate() {
                    nodes[i] = self.nodes[ci as usize];
                }
                Ok(self.factory.new_js_doc_type_literal(&nodes, is_array_type))
            }
            SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag => {
                let is_bracketed = common_data & 1 != 0;
                let is_name_first = common_data & 2 != 0;
                let mut it = new_child_iter(child_indices);
                let tag_name = self.node_at(it.next_if(mask, 0));
                let name = self.node_at(it.next_if(mask, 1));
                let type_expression = self.node_at(it.next_if(mask, 2));
                let comment = self.node_list_at(it.next_if(mask, 3));
                Ok(self.factory.new_js_doc_parameter_or_property_tag(
                    kind,
                    tag_name,
                    name,
                    is_bracketed,
                    type_expression,
                    is_name_first,
                    comment,
                ))
            }
            _ => Err(errors::errorf(
                format!(
                    "unhandled node kind {} with {} children",
                    go_kind_string(kind as i16),
                    child_indices.len()
                ),
                Vec::new(),
            )),
        }
    }
}
