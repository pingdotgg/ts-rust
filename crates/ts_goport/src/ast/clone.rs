//! Go `Clone` of AST nodes and lists (`ast/ast_generated.go` Clone methods,
//! `ast/ast.go` `cloneNode`, `NodeList.Clone`, `ModifierList.Clone`) and
//! `ast/deepclone.go`.
//!
//! Go dispatches `node.Clone(f)` on the node data type. Here
//! `NodeFactory::clone_node` matches on the astdata `NodeData` variant, which
//! has one variant per Go data type.

use crate::astdata::NodeData as D;
use crate::prelude::*;

// Go: ast/ast.go:114 cloneNode
// PORT: Go name `cloneNode` is `clone_node_from`, because
// `NodeFactory::clone_node` is Go `Node.Clone`.
fn clone_node_from(updated: Node, original: Node, hooks: &NodeFactoryHooks) -> Node {
    update_node(updated, original, hooks);
    if updated != original
        && let Some(on_clone) = &hooks.on_clone
    {
        on_clone(updated, original);
    }
    updated
}

impl NodeFactory {
    // Go: ast/ast.go:196 (n *Node) Clone
    /// Go `node.Clone(f)`: a new node with the same kind, children, flags and
    /// `Loc`. Children are shared, not cloned.
    pub fn clone_node(&self, node: Node) -> Node {
        with_ast_data(node, |d| match d {
            D::Token(_) => self.clone_token(node),
            D::Identifier(_) => self.clone_identifier(node),
            D::PrivateIdentifier(_) => self.clone_private_identifier(node),
            D::QualifiedName(_) => self.clone_qualified_name(node),
            D::ComputedPropertyName(_) => self.clone_computed_property_name(node),
            D::Decorator(_) => self.clone_decorator(node),
            D::EmptyStatement(_) => self.clone_empty_statement(node),
            D::IfStatement(_) => self.clone_if_statement(node),
            D::DoStatement(_) => self.clone_do_statement(node),
            D::WhileStatement(_) => self.clone_while_statement(node),
            D::ForStatement(_) => self.clone_for_statement(node),
            D::ForInOrOfStatement(_) => self.clone_for_in_or_of_statement(node),
            D::BreakStatement(_) => self.clone_break_statement(node),
            D::ContinueStatement(_) => self.clone_continue_statement(node),
            D::ReturnStatement(_) => self.clone_return_statement(node),
            D::WithStatement(_) => self.clone_with_statement(node),
            D::SwitchStatement(_) => self.clone_switch_statement(node),
            D::CaseBlock(_) => self.clone_case_block(node),
            D::CaseOrDefaultClause(_) => self.clone_case_or_default_clause(node),
            D::ThrowStatement(_) => self.clone_throw_statement(node),
            D::TryStatement(_) => self.clone_try_statement(node),
            D::CatchClause(_) => self.clone_catch_clause(node),
            D::DebuggerStatement(_) => self.clone_debugger_statement(node),
            D::LabeledStatement(_) => self.clone_labeled_statement(node),
            D::ExpressionStatement(_) => self.clone_expression_statement(node),
            D::Block(_) => self.clone_block(node),
            D::VariableStatement(_) => self.clone_variable_statement(node),
            D::VariableDeclaration(_) => self.clone_variable_declaration(node),
            D::VariableDeclarationList(_) => self.clone_variable_declaration_list(node),
            D::BindingPattern(_) => self.clone_binding_pattern(node),
            D::ParameterDeclaration(_) => self.clone_parameter_declaration(node),
            D::BindingElement(_) => self.clone_binding_element(node),
            D::MissingDeclaration(_) => self.clone_missing_declaration(node),
            D::FunctionDeclaration(_) => self.clone_function_declaration(node),
            D::ClassDeclaration(_) => self.clone_class_declaration(node),
            D::ClassExpression(_) => self.clone_class_expression(node),
            D::HeritageClause(_) => self.clone_heritage_clause(node),
            D::InterfaceDeclaration(_) => self.clone_interface_declaration(node),
            D::TypeAliasDeclaration(_) => self.clone_type_alias_declaration(node),
            D::EnumMember(_) => self.clone_enum_member(node),
            D::EnumDeclaration(_) => self.clone_enum_declaration(node),
            D::ModuleBlock(_) => self.clone_module_block(node),
            D::NotEmittedStatement(_) => self.clone_not_emitted_statement(node),
            D::NotEmittedTypeElement(_) => self.clone_not_emitted_type_element(node),
            D::ImportDeclaration(_) => self.clone_import_declaration(node),
            D::ExternalModuleReference(_) => self.clone_external_module_reference(node),
            D::NamespaceImport(_) => self.clone_namespace_import(node),
            D::NamedImports(_) => self.clone_named_imports(node),
            D::ExportAssignment(_) => self.clone_export_assignment(node),
            D::NamespaceExportDeclaration(_) => self.clone_namespace_export_declaration(node),
            D::NamespaceExport(_) => self.clone_namespace_export(node),
            D::NamedExports(_) => self.clone_named_exports(node),
            D::ExportSpecifier(_) => self.clone_export_specifier(node),
            D::CallSignatureDeclaration(_) => self.clone_call_signature_declaration(node),
            D::ConstructSignatureDeclaration(_) => self.clone_construct_signature_declaration(node),
            D::ConstructorDeclaration(_) => self.clone_constructor_declaration(node),
            D::GetAccessorDeclaration(_) => self.clone_get_accessor_declaration(node),
            D::SetAccessorDeclaration(_) => self.clone_set_accessor_declaration(node),
            D::IndexSignatureDeclaration(_) => self.clone_index_signature_declaration(node),
            D::MethodSignatureDeclaration(_) => self.clone_method_signature_declaration(node),
            D::MethodDeclaration(_) => self.clone_method_declaration(node),
            D::PropertySignatureDeclaration(_) => self.clone_property_signature_declaration(node),
            D::PropertyDeclaration(_) => self.clone_property_declaration(node),
            D::SemicolonClassElement(_) => self.clone_semicolon_class_element(node),
            D::ClassStaticBlockDeclaration(_) => self.clone_class_static_block_declaration(node),
            D::OmittedExpression(_) => self.clone_omitted_expression(node),
            D::KeywordExpression(_) => self.clone_keyword_expression(node),
            D::StringLiteral(_) => self.clone_string_literal(node),
            D::NumericLiteral(_) => self.clone_numeric_literal(node),
            D::BigIntLiteral(_) => self.clone_big_int_literal(node),
            D::RegularExpressionLiteral(_) => self.clone_regular_expression_literal(node),
            D::NoSubstitutionTemplateLiteral(_) => {
                self.clone_no_substitution_template_literal(node)
            }
            D::BinaryExpression(_) => self.clone_binary_expression(node),
            D::PrefixUnaryExpression(_) => self.clone_prefix_unary_expression(node),
            D::PostfixUnaryExpression(_) => self.clone_postfix_unary_expression(node),
            D::YieldExpression(_) => self.clone_yield_expression(node),
            D::ArrowFunction(_) => self.clone_arrow_function(node),
            D::FunctionExpression(_) => self.clone_function_expression(node),
            D::AsExpression(_) => self.clone_as_expression(node),
            D::SatisfiesExpression(_) => self.clone_satisfies_expression(node),
            D::ConditionalExpression(_) => self.clone_conditional_expression(node),
            D::PropertyAccessExpression(_) => self.clone_property_access_expression(node),
            D::ElementAccessExpression(_) => self.clone_element_access_expression(node),
            D::CallExpression(_) => self.clone_call_expression(node),
            D::NewExpression(_) => self.clone_new_expression(node),
            D::MetaProperty(_) => self.clone_meta_property(node),
            D::NonNullExpression(_) => self.clone_non_null_expression(node),
            D::SpreadElement(_) => self.clone_spread_element(node),
            D::TemplateExpression(_) => self.clone_template_expression(node),
            D::TemplateSpan(_) => self.clone_template_span(node),
            D::TaggedTemplateExpression(_) => self.clone_tagged_template_expression(node),
            D::ParenthesizedExpression(_) => self.clone_parenthesized_expression(node),
            D::ArrayLiteralExpression(_) => self.clone_array_literal_expression(node),
            D::ObjectLiteralExpression(_) => self.clone_object_literal_expression(node),
            D::SpreadAssignment(_) => self.clone_spread_assignment(node),
            D::PropertyAssignment(_) => self.clone_property_assignment(node),
            D::ShorthandPropertyAssignment(_) => self.clone_shorthand_property_assignment(node),
            D::DeleteExpression(_) => self.clone_delete_expression(node),
            D::TypeOfExpression(_) => self.clone_type_of_expression(node),
            D::VoidExpression(_) => self.clone_void_expression(node),
            D::AwaitExpression(_) => self.clone_await_expression(node),
            D::TypeAssertion(_) => self.clone_type_assertion(node),
            D::KeywordTypeNode(_) => self.clone_keyword_type_node(node),
            D::UnionTypeNode(_) => self.clone_union_type_node(node),
            D::IntersectionTypeNode(_) => self.clone_intersection_type_node(node),
            D::ConditionalTypeNode(_) => self.clone_conditional_type_node(node),
            D::TypeOperatorNode(_) => self.clone_type_operator_node(node),
            D::InferTypeNode(_) => self.clone_infer_type_node(node),
            D::ArrayTypeNode(_) => self.clone_array_type_node(node),
            D::IndexedAccessTypeNode(_) => self.clone_indexed_access_type_node(node),
            D::TypeReferenceNode(_) => self.clone_type_reference_node(node),
            D::ExpressionWithTypeArguments(_) => self.clone_expression_with_type_arguments(node),
            D::LiteralTypeNode(_) => self.clone_literal_type_node(node),
            D::ThisTypeNode(_) => self.clone_this_type_node(node),
            D::TypePredicateNode(_) => self.clone_type_predicate_node(node),
            D::ImportAttribute(_) => self.clone_import_attribute(node),
            D::ImportAttributes(_) => self.clone_import_attributes(node),
            D::TypeQueryNode(_) => self.clone_type_query_node(node),
            D::MappedTypeNode(_) => self.clone_mapped_type_node(node),
            D::TypeLiteralNode(_) => self.clone_type_literal_node(node),
            D::TupleTypeNode(_) => self.clone_tuple_type_node(node),
            D::NamedTupleMember(_) => self.clone_named_tuple_member(node),
            D::OptionalTypeNode(_) => self.clone_optional_type_node(node),
            D::RestTypeNode(_) => self.clone_rest_type_node(node),
            D::ParenthesizedTypeNode(_) => self.clone_parenthesized_type_node(node),
            D::FunctionTypeNode(_) => self.clone_function_type_node(node),
            D::ConstructorTypeNode(_) => self.clone_constructor_type_node(node),
            D::TemplateHead(_) => self.clone_template_head(node),
            D::TemplateMiddle(_) => self.clone_template_middle(node),
            D::TemplateTail(_) => self.clone_template_tail(node),
            D::TemplateLiteralTypeNode(_) => self.clone_template_literal_type_node(node),
            D::TemplateLiteralTypeSpan(_) => self.clone_template_literal_type_span(node),
            D::SyntheticExpression(_) => self.clone_synthetic_expression(node),
            D::PartiallyEmittedExpression(_) => self.clone_partially_emitted_expression(node),
            D::JsxElement(_) => self.clone_jsx_element(node),
            D::JsxAttributes(_) => self.clone_jsx_attributes(node),
            D::JsxNamespacedName(_) => self.clone_jsx_namespaced_name(node),
            D::JsxOpeningElement(_) => self.clone_jsx_opening_element(node),
            D::JsxSelfClosingElement(_) => self.clone_jsx_self_closing_element(node),
            D::JsxFragment(_) => self.clone_jsx_fragment(node),
            D::JsxOpeningFragment(_) => self.clone_jsx_opening_fragment(node),
            D::JsxClosingFragment(_) => self.clone_jsx_closing_fragment(node),
            D::JsxAttribute(_) => self.clone_jsx_attribute(node),
            D::JsxSpreadAttribute(_) => self.clone_jsx_spread_attribute(node),
            D::JsxClosingElement(_) => self.clone_jsx_closing_element(node),
            D::JsxExpression(_) => self.clone_jsx_expression(node),
            D::JsxText(_) => self.clone_jsx_text(node),
            D::SyntaxList(_) => self.clone_syntax_list(node),
            D::JsDoc(_) => self.clone_js_doc(node),
            D::JsDocTypeExpression(_) => self.clone_js_doc_type_expression(node),
            D::JsDocNonNullableType(_) => self.clone_js_doc_non_nullable_type(node),
            D::JsDocNullableType(_) => self.clone_js_doc_nullable_type(node),
            D::JsDocAllType(_) => self.clone_js_doc_all_type(node),
            D::JsDocVariadicType(_) => self.clone_js_doc_variadic_type(node),
            D::JsDocOptionalType(_) => self.clone_js_doc_optional_type(node),
            D::JsDocTypeTag(_) => self.clone_js_doc_type_tag(node),
            D::JsDocUnknownTag(_) => self.clone_js_doc_unknown_tag(node),
            D::JsDocTemplateTag(_) => self.clone_js_doc_template_tag(node),
            D::JsDocReturnTag(_) => self.clone_js_doc_return_tag(node),
            D::JsDocPublicTag(_) => self.clone_js_doc_public_tag(node),
            D::JsDocPrivateTag(_) => self.clone_js_doc_private_tag(node),
            D::JsDocProtectedTag(_) => self.clone_js_doc_protected_tag(node),
            D::JsDocReadonlyTag(_) => self.clone_js_doc_readonly_tag(node),
            D::JsDocOverrideTag(_) => self.clone_js_doc_override_tag(node),
            D::JsDocDeprecatedTag(_) => self.clone_js_doc_deprecated_tag(node),
            D::JsDocSeeTag(_) => self.clone_js_doc_see_tag(node),
            D::JsDocImplementsTag(_) => self.clone_js_doc_implements_tag(node),
            D::JsDocAugmentsTag(_) => self.clone_js_doc_augments_tag(node),
            D::JsDocSatisfiesTag(_) => self.clone_js_doc_satisfies_tag(node),
            D::JsDocThrowsTag(_) => self.clone_js_doc_throws_tag(node),
            D::JsDocThisTag(_) => self.clone_js_doc_this_tag(node),
            D::JsDocImportTag(_) => self.clone_js_doc_import_tag(node),
            D::JsDocCallbackTag(_) => self.clone_js_doc_callback_tag(node),
            D::JsDocOverloadTag(_) => self.clone_js_doc_overload_tag(node),
            D::JsDocTypedefTag(_) => self.clone_js_doc_typedef_tag(node),
            D::JsDocSignature(_) => self.clone_js_doc_signature(node),
            D::JsDocNameReference(_) => self.clone_js_doc_name_reference(node),
            D::ModuleDeclaration(_) => self.clone_module_declaration(node),
            D::ImportEqualsDeclaration(_) => self.clone_import_equals_declaration(node),
            D::ExportDeclaration(_) => self.clone_export_declaration(node),
            D::ImportTypeNode(_) => self.clone_import_type_node(node),
            D::ImportClause(_) => self.clone_import_clause(node),
            D::ImportSpecifier(_) => self.clone_import_specifier(node),
            D::JsDocText(_) => self.clone_js_doc_text(node),
            D::JsDocLink(_) => self.clone_js_doc_link(node),
            D::JsDocLinkPlain(_) => self.clone_js_doc_link_plain(node),
            D::JsDocLinkCode(_) => self.clone_js_doc_link_code(node),
            D::TypeParameterDeclaration(_) => self.clone_type_parameter_declaration(node),
            D::SyntheticReferenceExpression(_) => self.clone_synthetic_reference_expression(node),
            D::JsDocTypeLiteral(_) => self.clone_js_doc_type_literal(node),
            D::JsDocParameterOrPropertyTag(_) => self.clone_js_doc_parameter_or_property_tag(node),
            D::SourceFile(_) => self.clone_source_file(node),
        })
    }

    // Go: ast/ast.go:147 (list *NodeList) Clone
    // PORT: a factory list fixes its `Loc` at creation, so it is passed to
    // `new_node_list_with_loc` instead of being set after `NewNodeList`.
    #[must_use]
    pub fn clone_node_list(&self, list: NodeList) -> NodeList {
        self.new_node_list_with_loc(&list.nodes().to_vec(), list.loc())
    }

    // Go: ast/ast.go:168 (list *ModifierList) Clone
    // PORT: Go copies `ModifierFlags`; `new_modifier_list_with_loc`
    // recomputes `ModifiersToFlags(nodes)`, which is the same value for a
    // list the factory or the parser made.
    #[must_use]
    pub fn clone_modifier_list(&self, list: ModifierList) -> ModifierList {
        self.new_modifier_list_with_loc(&list.nodes().to_vec(), list.loc())
    }

    // Go: ast/ast_generated.go:604 (node *Token) Clone
    fn clone_token(&self, node: Node) -> Node {
        clone_node_from(self.new_token(node.kind()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:799 (node *Identifier) Clone
    fn clone_identifier(&self, node: Node) -> Node {
        clone_node_from(self.new_identifier(node.text()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:823 (node *PrivateIdentifier) Clone
    fn clone_private_identifier(&self, node: Node) -> Node {
        clone_node_from(self.new_private_identifier(node.text()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:865 (node *QualifiedName) Clone
    fn clone_qualified_name(&self, node: Node) -> Node {
        clone_node_from(
            self.new_qualified_name(node.left(), node.right()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:909 (node *ComputedPropertyName) Clone
    fn clone_computed_property_name(&self, node: Node) -> Node {
        clone_node_from(
            self.new_computed_property_name(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:952 (node *Decorator) Clone
    fn clone_decorator(&self, node: Node) -> Node {
        clone_node_from(self.new_decorator(node.expression()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:973 (node *EmptyStatement) Clone
    fn clone_empty_statement(&self, node: Node) -> Node {
        clone_node_from(self.new_empty_statement(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:1016 (node *IfStatement) Clone
    fn clone_if_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_if_statement(
                node.expression(),
                node.then_statement(),
                node.else_statement(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1062 (node *DoStatement) Clone
    fn clone_do_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_do_statement(node.statement(), node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1107 (node *WhileStatement) Clone
    fn clone_while_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_while_statement(node.expression(), node.statement()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1160 (node *ForStatement) Clone
    fn clone_for_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_for_statement(
                node.initializer(),
                node.condition(),
                node.incrementor(),
                node.statement(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1216 (node *ForInOrOfStatement) Clone
    fn clone_for_in_or_of_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_for_in_or_of_statement(
                node.kind(),
                node.await_modifier(),
                node.initializer(),
                node.expression(),
                node.statement(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1258 (node *BreakStatement) Clone
    fn clone_break_statement(&self, node: Node) -> Node {
        clone_node_from(self.new_break_statement(node.label()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:1296 (node *ContinueStatement) Clone
    fn clone_continue_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_continue_statement(node.label()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1335 (node *ReturnStatement) Clone
    fn clone_return_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_return_statement(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1376 (node *WithStatement) Clone
    fn clone_with_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_with_statement(node.expression(), node.statement()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1422 (node *SwitchStatement) Clone
    fn clone_switch_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_switch_statement(node.expression(), node.case_block()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1467 (node *CaseBlock) Clone
    fn clone_case_block(&self, node: Node) -> Node {
        clone_node_from(self.new_case_block(node.clauses()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:1513 (node *CaseOrDefaultClause) Clone
    fn clone_case_or_default_clause(&self, node: Node) -> Node {
        clone_node_from(
            self.new_case_or_default_clause(node.kind(), node.expression(), node.statement_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1561 (node *ThrowStatement) Clone
    fn clone_throw_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_throw_statement(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1608 (node *TryStatement) Clone
    fn clone_try_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_try_statement(node.try_block(), node.catch_clause(), node.finally_block()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1656 (node *CatchClause) Clone
    fn clone_catch_clause(&self, node: Node) -> Node {
        clone_node_from(
            self.new_catch_clause(node.variable_declaration(), node.block()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1677 (node *DebuggerStatement) Clone
    fn clone_debugger_statement(&self, node: Node) -> Node {
        clone_node_from(self.new_debugger_statement(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:1717 (node *LabeledStatement) Clone
    fn clone_labeled_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_labeled_statement(node.label(), node.statement()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1760 (node *ExpressionStatement) Clone
    fn clone_expression_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_expression_statement(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1806 (node *Block) Clone
    fn clone_block(&self, node: Node) -> Node {
        clone_node_from(
            self.new_block(node.statement_list(), node.multi_line()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1851 (node *VariableStatement) Clone
    fn clone_variable_statement(&self, node: Node) -> Node {
        clone_node_from(
            self.new_variable_statement(node.modifiers(), node.declaration_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1901 (node *VariableDeclaration) Clone
    fn clone_variable_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_variable_declaration(
                node.name(),
                node.exclamation_token(),
                node.type_(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1946 (node *VariableDeclarationList) Clone
    fn clone_variable_declaration_list(&self, node: Node) -> Node {
        clone_node_from(
            self.new_variable_declaration_list(node.declarations(), node.flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:1985 (node *BindingPattern) Clone
    fn clone_binding_pattern(&self, node: Node) -> Node {
        clone_node_from(
            self.new_binding_pattern(node.kind(), node.element_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2044 (node *ParameterDeclaration) Clone
    fn clone_parameter_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_parameter_declaration(
                node.modifiers(),
                node.dot_dot_dot_token(),
                node.name(),
                node.question_token(),
                node.type_(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2099 (node *BindingElement) Clone
    fn clone_binding_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_binding_element(
                node.dot_dot_dot_token(),
                node.property_name(),
                node.name(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2142 (node *MissingDeclaration) Clone
    fn clone_missing_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_missing_declaration(node.modifiers()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2200 (node *FunctionDeclaration) Clone
    fn clone_function_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_function_declaration(
                node.modifiers(),
                node.asterisk_token(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2251 (node *ClassDeclaration) Clone
    fn clone_class_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_class_declaration(
                node.modifiers(),
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2301 (node *ClassExpression) Clone
    fn clone_class_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_class_expression(
                node.modifiers(),
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2346 (node *HeritageClause) Clone
    fn clone_heritage_clause(&self, node: Node) -> Node {
        clone_node_from(
            self.new_heritage_clause(node.token(), node.types()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2399 (node *InterfaceDeclaration) Clone
    fn clone_interface_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_interface_declaration(
                node.modifiers(),
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2470 (node *TypeAliasDeclaration) Clone
    fn clone_type_alias_declaration(&self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::TypeAliasDeclaration => clone_node_from(
                self.new_type_alias_declaration(
                    node.modifiers(),
                    node.name(),
                    node.type_parameter_list(),
                    node.type_(),
                ),
                node,
                self.hooks(),
            ),
            SyntaxKind::JsTypeAliasDeclaration => clone_node_from(
                self.new_js_type_alias_declaration(
                    node.modifiers(),
                    node.name(),
                    node.type_parameter_list(),
                    node.type_(),
                ),
                node,
                self.hooks(),
            ),
            _ => panic!(
                "unexpected kind in TypeAliasDeclaration.Clone: {:?}",
                node.kind()
            ),
        }
    }

    // Go: ast/ast_generated.go:2526 (node *EnumMember) Clone
    fn clone_enum_member(&self, node: Node) -> Node {
        clone_node_from(
            self.new_enum_member(node.name(), node.initializer()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2575 (node *EnumDeclaration) Clone
    fn clone_enum_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_enum_declaration(node.modifiers(), node.name(), node.member_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2618 (node *ModuleBlock) Clone
    fn clone_module_block(&self, node: Node) -> Node {
        clone_node_from(
            self.new_module_block(node.statement_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2643 (node *NotEmittedStatement) Clone
    fn clone_not_emitted_statement(&self, node: Node) -> Node {
        clone_node_from(self.new_not_emitted_statement(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:2665 (node *NotEmittedTypeElement) Clone
    fn clone_not_emitted_type_element(&self, node: Node) -> Node {
        clone_node_from(self.new_not_emitted_type_element(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:2730 (node *ImportDeclaration) Clone
    fn clone_import_declaration(&self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::ImportDeclaration => clone_node_from(
                self.new_import_declaration(
                    node.modifiers(),
                    node.import_clause(),
                    node.module_specifier(),
                    node.attributes(),
                ),
                node,
                self.hooks(),
            ),
            SyntaxKind::JsImportDeclaration => clone_node_from(
                self.new_js_import_declaration(
                    node.modifiers(),
                    node.import_clause(),
                    node.module_specifier(),
                    node.attributes(),
                ),
                node,
                self.hooks(),
            ),
            _ => panic!(
                "unexpected kind in ImportDeclaration.Clone: {:?}",
                node.kind()
            ),
        }
    }

    // Go: ast/ast_generated.go:2786 (node *ExternalModuleReference) Clone
    fn clone_external_module_reference(&self, node: Node) -> Node {
        clone_node_from(
            self.new_external_module_reference(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2830 (node *NamespaceImport) Clone
    fn clone_namespace_import(&self, node: Node) -> Node {
        clone_node_from(self.new_namespace_import(node.name()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:2877 (node *NamedImports) Clone
    fn clone_named_imports(&self, node: Node) -> Node {
        clone_node_from(
            self.new_named_imports(node.element_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2927 (node *ExportAssignment) Clone
    fn clone_export_assignment(&self, node: Node) -> Node {
        clone_node_from(
            self.new_export_assignment(
                node.modifiers(),
                node.is_export_equals(),
                node.type_(),
                node.expression(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:2969 (node *NamespaceExportDeclaration) Clone
    fn clone_namespace_export_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_namespace_export_declaration(node.modifiers(), node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3012 (node *NamespaceExport) Clone
    fn clone_namespace_export(&self, node: Node) -> Node {
        clone_node_from(self.new_namespace_export(node.name()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:3059 (node *NamedExports) Clone
    fn clone_named_exports(&self, node: Node) -> Node {
        clone_node_from(
            self.new_named_exports(node.element_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3108 (node *ExportSpecifier) Clone
    fn clone_export_specifier(&self, node: Node) -> Node {
        clone_node_from(
            self.new_export_specifier(node.is_type_only(), node.property_name(), node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3155 (node *CallSignatureDeclaration) Clone
    fn clone_call_signature_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_call_signature_declaration(
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3198 (node *ConstructSignatureDeclaration) Clone
    fn clone_construct_signature_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_construct_signature_declaration(
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3251 (node *ConstructorDeclaration) Clone
    fn clone_constructor_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_constructor_declaration(
                node.modifiers(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3300 (node *GetAccessorDeclaration) Clone
    fn clone_get_accessor_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_get_accessor_declaration(
                node.modifiers(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3353 (node *SetAccessorDeclaration) Clone
    fn clone_set_accessor_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_set_accessor_declaration(
                node.modifiers(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3402 (node *IndexSignatureDeclaration) Clone
    fn clone_index_signature_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_index_signature_declaration(
                node.modifiers(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3453 (node *MethodSignatureDeclaration) Clone
    fn clone_method_signature_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_method_signature_declaration(
                node.modifiers(),
                node.name(),
                node.postfix_token(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3516 (node *MethodDeclaration) Clone
    fn clone_method_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_method_declaration(
                node.modifiers(),
                node.asterisk_token(),
                node.name(),
                node.postfix_token(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3570 (node *PropertySignatureDeclaration) Clone
    fn clone_property_signature_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_property_signature_declaration(
                node.modifiers(),
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3624 (node *PropertyDeclaration) Clone
    fn clone_property_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_property_declaration(
                node.modifiers(),
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3651 (node *SemicolonClassElement) Clone
    fn clone_semicolon_class_element(&self, node: Node) -> Node {
        clone_node_from(self.new_semicolon_class_element(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:3696 (node *ClassStaticBlockDeclaration) Clone
    fn clone_class_static_block_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_class_static_block_declaration(node.modifiers(), node.body()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3717 (node *OmittedExpression) Clone
    fn clone_omitted_expression(&self, node: Node) -> Node {
        clone_node_from(self.new_omitted_expression(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:3739 (node *KeywordExpression) Clone
    fn clone_keyword_expression(&self, node: Node) -> Node {
        clone_node_from(self.new_keyword_expression(node.kind()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:3772 (node *StringLiteral) Clone
    fn clone_string_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_string_literal(node.text(), node.token_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3796 (node *NumericLiteral) Clone
    fn clone_numeric_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_numeric_literal(node.text(), node.token_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3820 (node *BigIntLiteral) Clone
    fn clone_big_int_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_big_int_literal(node.text(), node.token_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3844 (node *RegularExpressionLiteral) Clone
    fn clone_regular_expression_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_regular_expression_literal(node.text(), node.token_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3870 (node *NoSubstitutionTemplateLiteral) Clone
    fn clone_no_substitution_template_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_no_substitution_template_literal(node.text(), node.template_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3922 (node *BinaryExpression) Clone
    fn clone_binary_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_binary_expression(
                node.modifiers(),
                node.left(),
                node.type_(),
                node.operator_token(),
                node.right(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:3962 (node *PrefixUnaryExpression) Clone
    fn clone_prefix_unary_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_prefix_unary_expression(node.operator(), node.operand()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4006 (node *PostfixUnaryExpression) Clone
    fn clone_postfix_unary_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_postfix_unary_expression(node.operand(), node.operator()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4050 (node *YieldExpression) Clone
    fn clone_yield_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_yield_expression(node.asterisk_token(), node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4105 (node *ArrowFunction) Clone
    fn clone_arrow_function(&self, node: Node) -> Node {
        clone_node_from(
            self.new_arrow_function(
                node.modifiers(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.equals_greater_than_token(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4163 (node *FunctionExpression) Clone
    fn clone_function_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_function_expression(
                node.modifiers(),
                node.asterisk_token(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4207 (node *AsExpression) Clone
    fn clone_as_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_as_expression(node.expression(), node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4247 (node *SatisfiesExpression) Clone
    fn clone_satisfies_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_satisfies_expression(node.expression(), node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4298 (node *ConditionalExpression) Clone
    fn clone_conditional_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_conditional_expression(
                node.condition(),
                node.question_token(),
                node.when_true(),
                node.colon_token(),
                node.when_false(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4352 (node *PropertyAccessExpression) Clone
    fn clone_property_access_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_property_access_expression(
                node.expression(),
                node.question_dot_token(),
                node.name(),
                node.flags(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4402 (node *ElementAccessExpression) Clone
    fn clone_element_access_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_element_access_expression(
                node.expression(),
                node.question_dot_token(),
                node.argument_expression(),
                node.flags(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4459 (node *CallExpression) Clone
    fn clone_call_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_call_expression(
                node.expression(),
                node.question_dot_token(),
                node.type_argument_list(),
                node.argument_list(),
                node.flags(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4502 (node *NewExpression) Clone
    fn clone_new_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_new_expression(
                node.expression(),
                node.type_argument_list(),
                node.argument_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4544 (node *MetaProperty) Clone
    fn clone_meta_property(&self, node: Node) -> Node {
        clone_node_from(
            self.new_meta_property(node.keyword_token(), node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4588 (node *NonNullExpression) Clone
    fn clone_non_null_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_non_null_expression(node.expression(), node.flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4626 (node *SpreadElement) Clone
    fn clone_spread_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_spread_element(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4667 (node *TemplateExpression) Clone
    fn clone_template_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_expression(node.head(), node.template_spans()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4712 (node *TemplateSpan) Clone
    fn clone_template_span(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_span(node.expression(), node.literal()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4767 (node *TaggedTemplateExpression) Clone
    fn clone_tagged_template_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_tagged_template_expression(
                node.tag(),
                node.question_dot_token(),
                node.type_argument_list(),
                node.template(),
                node.flags(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4805 (node *ParenthesizedExpression) Clone
    fn clone_parenthesized_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_parenthesized_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4850 (node *ArrayLiteralExpression) Clone
    fn clone_array_literal_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_array_literal_expression(node.element_list(), node.multi_line()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4896 (node *ObjectLiteralExpression) Clone
    fn clone_object_literal_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_object_literal_expression(node.property_list(), node.multi_line()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4940 (node *SpreadAssignment) Clone
    fn clone_spread_assignment(&self, node: Node) -> Node {
        clone_node_from(
            self.new_spread_assignment(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:4990 (node *PropertyAssignment) Clone
    fn clone_property_assignment(&self, node: Node) -> Node {
        clone_node_from(
            self.new_property_assignment(
                node.modifiers(),
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5047 (node *ShorthandPropertyAssignment) Clone
    fn clone_shorthand_property_assignment(&self, node: Node) -> Node {
        clone_node_from(
            self.new_shorthand_property_assignment(
                node.modifiers(),
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.equals_token(),
                node.object_assignment_initializer(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5089 (node *DeleteExpression) Clone
    fn clone_delete_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_delete_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5131 (node *TypeOfExpression) Clone
    fn clone_type_of_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_of_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5173 (node *VoidExpression) Clone
    fn clone_void_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_void_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5215 (node *AwaitExpression) Clone
    fn clone_await_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_await_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5255 (node *TypeAssertion) Clone
    fn clone_type_assertion(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_assertion(node.type_(), node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5276 (node *KeywordTypeNode) Clone
    fn clone_keyword_type_node(&self, node: Node) -> Node {
        clone_node_from(self.new_keyword_type_node(node.kind()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:5329 (node *UnionTypeNode) Clone
    fn clone_union_type_node(&self, node: Node) -> Node {
        clone_node_from(self.new_union_type_node(node.types()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:5367 (node *IntersectionTypeNode) Clone
    fn clone_intersection_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_intersection_type_node(node.types()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5415 (node *ConditionalTypeNode) Clone
    fn clone_conditional_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_conditional_type_node(
                node.check_type(),
                node.extends_type(),
                node.true_type(),
                node.false_type(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5455 (node *TypeOperatorNode) Clone
    fn clone_type_operator_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_operator_node(node.operator(), node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5493 (node *InferTypeNode) Clone
    fn clone_infer_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_infer_type_node(node.type_parameter()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5531 (node *ArrayTypeNode) Clone
    fn clone_array_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_array_type_node(node.element_type()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5571 (node *IndexedAccessTypeNode) Clone
    fn clone_indexed_access_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_indexed_access_type_node(node.object_type(), node.index_type()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5610 (node *TypeReferenceNode) Clone
    fn clone_type_reference_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_reference_node(node.type_name(), node.type_argument_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5651 (node *ExpressionWithTypeArguments) Clone
    fn clone_expression_with_type_arguments(&self, node: Node) -> Node {
        clone_node_from(
            self.new_expression_with_type_arguments(node.expression(), node.type_argument_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5689 (node *LiteralTypeNode) Clone
    fn clone_literal_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_literal_type_node(node.literal()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5710 (node *ThisTypeNode) Clone
    fn clone_this_type_node(&self, node: Node) -> Node {
        clone_node_from(self.new_this_type_node(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:5752 (node *TypePredicateNode) Clone
    fn clone_type_predicate_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_predicate_node(
                node.asserts_modifier(),
                node.parameter_name(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5793 (node *ImportAttribute) Clone
    fn clone_import_attribute(&self, node: Node) -> Node {
        clone_node_from(
            self.new_import_attribute(node.name(), node.value()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5845 (node *ImportAttributes) Clone
    fn clone_import_attributes(&self, node: Node) -> Node {
        // PORT: `Node::attributes` does not cover ImportAttributes, whose
        // `Attributes` field is a NodeList. The list is read from the data.
        let attributes = import_attributes_list(node);
        clone_node_from(
            self.new_import_attributes(node.token(), attributes, node.multi_line()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5888 (node *TypeQueryNode) Clone
    fn clone_type_query_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_query_node(node.expr_name(), node.type_argument_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5943 (node *MappedTypeNode) Clone
    fn clone_mapped_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_mapped_type_node(
                node.readonly_token(),
                node.type_parameter(),
                node.name_type(),
                node.question_token(),
                node.type_(),
                node.member_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:5982 (node *TypeLiteralNode) Clone
    fn clone_type_literal_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_literal_node(node.member_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6020 (node *TupleTypeNode) Clone
    fn clone_tuple_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_tuple_type_node(node.element_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6068 (node *NamedTupleMember) Clone
    fn clone_named_tuple_member(&self, node: Node) -> Node {
        clone_node_from(
            self.new_named_tuple_member(
                node.dot_dot_dot_token(),
                node.name(),
                node.question_token(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6110 (node *OptionalTypeNode) Clone
    fn clone_optional_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_optional_type_node(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6148 (node *RestTypeNode) Clone
    fn clone_rest_type_node(&self, node: Node) -> Node {
        clone_node_from(self.new_rest_type_node(node.type_()), node, self.hooks())
    }

    // Go: ast/ast_generated.go:6186 (node *ParenthesizedTypeNode) Clone
    fn clone_parenthesized_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_parenthesized_type_node(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6226 (node *FunctionTypeNode) Clone
    fn clone_function_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_function_type_node(
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6270 (node *ConstructorTypeNode) Clone
    fn clone_constructor_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_constructor_type_node(
                node.modifiers(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6296 (node *TemplateHead) Clone
    fn clone_template_head(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_head(node.text(), node.raw_text(), node.template_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6322 (node *TemplateMiddle) Clone
    fn clone_template_middle(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_middle(node.text(), node.raw_text(), node.template_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6348 (node *TemplateTail) Clone
    fn clone_template_tail(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_tail(node.text(), node.raw_text(), node.template_flags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6388 (node *TemplateLiteralTypeNode) Clone
    fn clone_template_literal_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_literal_type_node(node.head(), node.template_spans()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6428 (node *TemplateLiteralTypeSpan) Clone
    fn clone_template_literal_type_span(&self, node: Node) -> Node {
        clone_node_from(
            self.new_template_literal_type_span(node.type_(), node.literal()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6470 (node *SyntheticExpression) Clone
    fn clone_synthetic_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_synthetic_expression(
                synthetic_expression_type(node),
                node.is_spread(),
                node.tuple_name_source(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6508 (node *PartiallyEmittedExpression) Clone
    fn clone_partially_emitted_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_partially_emitted_expression(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6555 (node *JsxElement) Clone
    fn clone_jsx_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_element(
                node.opening_element(),
                node.children(),
                node.closing_element(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6595 (node *JsxAttributes) Clone
    fn clone_jsx_attributes(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_attributes(node.property_list()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6636 (node *JsxNamespacedName) Clone
    fn clone_jsx_namespaced_name(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_namespaced_name(node.namespace(), node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6683 (node *JsxOpeningElement) Clone
    fn clone_jsx_opening_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_opening_element(
                node.tag_name(),
                node.type_argument_list(),
                node.attributes(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6726 (node *JsxSelfClosingElement) Clone
    fn clone_jsx_self_closing_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_self_closing_element(
                node.tag_name(),
                node.type_argument_list(),
                node.attributes(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6769 (node *JsxFragment) Clone
    fn clone_jsx_fragment(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_fragment(
                node.opening_fragment(),
                node.children(),
                node.closing_fragment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6790 (node *JsxOpeningFragment) Clone
    fn clone_jsx_opening_fragment(&self, node: Node) -> Node {
        clone_node_from(self.new_jsx_opening_fragment(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:6811 (node *JsxClosingFragment) Clone
    fn clone_jsx_closing_fragment(&self, node: Node) -> Node {
        clone_node_from(self.new_jsx_closing_fragment(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:6853 (node *JsxAttribute) Clone
    fn clone_jsx_attribute(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_attribute(node.name(), node.initializer()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6896 (node *JsxSpreadAttribute) Clone
    fn clone_jsx_spread_attribute(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_spread_attribute(node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6934 (node *JsxClosingElement) Clone
    fn clone_jsx_closing_element(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_closing_element(node.tag_name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:6974 (node *JsxExpression) Clone
    fn clone_jsx_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_expression(node.dot_dot_dot_token(), node.expression()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7000 (node *JsxText) Clone
    fn clone_jsx_text(&self, node: Node) -> Node {
        clone_node_from(
            self.new_jsx_text(node.text(), node.contains_only_trivia_white_spaces()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7039 (node *SyntaxList) Clone
    fn clone_syntax_list(&self, node: Node) -> Node {
        clone_node_from(
            self.new_syntax_list(&syntax_list_children(node)),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7079 (node *JSDoc) Clone
    fn clone_js_doc(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc(node.comment(), node.tags()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7117 (node *JSDocTypeExpression) Clone
    fn clone_js_doc_type_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_type_expression(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7155 (node *JSDocNonNullableType) Clone
    fn clone_js_doc_non_nullable_type(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_non_nullable_type(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7193 (node *JSDocNullableType) Clone
    fn clone_js_doc_nullable_type(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_nullable_type(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7214 (node *JSDocAllType) Clone
    fn clone_js_doc_all_type(&self, node: Node) -> Node {
        clone_node_from(self.new_js_doc_all_type(), node, self.hooks())
    }

    // Go: ast/ast_generated.go:7252 (node *JSDocVariadicType) Clone
    fn clone_js_doc_variadic_type(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_variadic_type(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7290 (node *JSDocOptionalType) Clone
    fn clone_js_doc_optional_type(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_optional_type(node.type_()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7330 (node *JSDocTypeTag) Clone
    fn clone_js_doc_type_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_type_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7368 (node *JSDocUnknownTag) Clone
    fn clone_js_doc_unknown_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_unknown_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7413 (node *JSDocTemplateTag) Clone
    fn clone_js_doc_template_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_template_tag(
                node.tag_name(),
                node.constraint(),
                node.type_parameter_list(),
                node.comment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7453 (node *JSDocReturnTag) Clone
    fn clone_js_doc_return_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_return_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7491 (node *JSDocPublicTag) Clone
    fn clone_js_doc_public_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_public_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7529 (node *JSDocPrivateTag) Clone
    fn clone_js_doc_private_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_private_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7567 (node *JSDocProtectedTag) Clone
    fn clone_js_doc_protected_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_protected_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7605 (node *JSDocReadonlyTag) Clone
    fn clone_js_doc_readonly_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_readonly_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7643 (node *JSDocOverrideTag) Clone
    fn clone_js_doc_override_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_override_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7681 (node *JSDocDeprecatedTag) Clone
    fn clone_js_doc_deprecated_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_deprecated_tag(node.tag_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7721 (node *JSDocSeeTag) Clone
    fn clone_js_doc_see_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_see_tag(node.tag_name(), node.name_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7761 (node *JSDocImplementsTag) Clone
    fn clone_js_doc_implements_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_implements_tag(node.tag_name(), node.class_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7801 (node *JSDocAugmentsTag) Clone
    fn clone_js_doc_augments_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_augments_tag(node.tag_name(), node.class_name(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7841 (node *JSDocSatisfiesTag) Clone
    fn clone_js_doc_satisfies_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_satisfies_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7881 (node *JSDocThrowsTag) Clone
    fn clone_js_doc_throws_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_throws_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7921 (node *JSDocThisTag) Clone
    fn clone_js_doc_this_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_this_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:7969 (node *JSDocImportTag) Clone
    fn clone_js_doc_import_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_import_tag(
                node.tag_name(),
                node.import_clause(),
                node.module_specifier(),
                node.attributes(),
                node.comment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8014 (node *JSDocCallbackTag) Clone
    fn clone_js_doc_callback_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_callback_tag(
                node.tag_name(),
                node.type_expression(),
                node.name(),
                node.comment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8058 (node *JSDocOverloadTag) Clone
    fn clone_js_doc_overload_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_overload_tag(node.tag_name(), node.type_expression(), node.comment()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8103 (node *JSDocTypedefTag) Clone
    fn clone_js_doc_typedef_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_typedef_tag(
                node.tag_name(),
                node.type_expression(),
                node.name(),
                node.comment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8147 (node *JSDocSignature) Clone
    fn clone_js_doc_signature(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_signature(
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8185 (node *JSDocNameReference) Clone
    fn clone_js_doc_name_reference(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_name_reference(node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8077 (node *ModuleDeclaration) Clone
    fn clone_module_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_module_declaration(
                node.modifiers(),
                node.keyword(),
                node.name(),
                node.attributes(),
                node.body(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8297 (node *ImportEqualsDeclaration) Clone
    fn clone_import_equals_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_import_equals_declaration(
                node.modifiers(),
                node.is_type_only(),
                node.name(),
                node.module_reference(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8352 (node *ExportDeclaration) Clone
    fn clone_export_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_export_declaration(
                node.modifiers(),
                node.is_type_only(),
                node.export_clause(),
                node.module_specifier(),
                node.attributes(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8400 (node *ImportTypeNode) Clone
    fn clone_import_type_node(&self, node: Node) -> Node {
        clone_node_from(
            self.new_import_type_node(
                node.is_type_of(),
                node.argument(),
                node.attributes(),
                node.qualifier(),
                node.type_argument_list(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8445 (node *ImportClause) Clone
    fn clone_import_clause(&self, node: Node) -> Node {
        clone_node_from(
            self.new_import_clause(node.phase_modifier(), node.name(), node.named_bindings()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8494 (node *ImportSpecifier) Clone
    fn clone_import_specifier(&self, node: Node) -> Node {
        clone_node_from(
            self.new_import_specifier(node.is_type_only(), node.property_name(), node.name()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8521 (node *JSDocText) Clone
    fn clone_js_doc_text(&self, node: Node) -> Node {
        // PORT: `Node::text` joins the parts. Go copies the `[]string`, so the
        // parts are read from the data.
        let text = with_ast_data(node, |d| match d {
            D::JsDocText(d) => d.text.clone(),
            _ => panic!("AsJSDocText called on {:?}", node.kind()),
        });
        clone_node_from(self.new_js_doc_text(text), node, self.hooks())
    }

    // Go: ast/ast_generated.go:8561 (node *JSDocLink) Clone
    fn clone_js_doc_link(&self, node: Node) -> Node {
        // PORT: `Node::text` joins the parts. Go copies the `[]string`, so the
        // parts are read from the data.
        let text = with_ast_data(node, |d| match d {
            D::JsDocLink(d) => d.text.clone(),
            _ => panic!("AsJSDocLink called on {:?}", node.kind()),
        });
        clone_node_from(self.new_js_doc_link(node.name(), text), node, self.hooks())
    }

    // Go: ast/ast_generated.go:8605 (node *JSDocLinkPlain) Clone
    fn clone_js_doc_link_plain(&self, node: Node) -> Node {
        // PORT: `Node::text` joins the parts. Go copies the `[]string`, so the
        // parts are read from the data.
        let text = with_ast_data(node, |d| match d {
            D::JsDocLinkPlain(d) => d.text.clone(),
            _ => panic!("AsJSDocLinkPlain called on {:?}", node.kind()),
        });
        clone_node_from(
            self.new_js_doc_link_plain(node.name(), text),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8649 (node *JSDocLinkCode) Clone
    fn clone_js_doc_link_code(&self, node: Node) -> Node {
        // PORT: `Node::text` joins the parts. Go copies the `[]string`, so the
        // parts are read from the data.
        let text = with_ast_data(node, |d| match d {
            D::JsDocLinkCode(d) => d.text.clone(),
            _ => panic!("AsJSDocLinkCode called on {:?}", node.kind()),
        });
        clone_node_from(
            self.new_js_doc_link_code(node.name(), text),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8705 (node *TypeParameterDeclaration) Clone
    fn clone_type_parameter_declaration(&self, node: Node) -> Node {
        clone_node_from(
            self.new_type_parameter_declaration(
                node.modifiers(),
                node.name(),
                node.constraint(),
                node.expression(),
                node.default_type(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8749 (node *SyntheticReferenceExpression) Clone
    fn clone_synthetic_reference_expression(&self, node: Node) -> Node {
        clone_node_from(
            self.new_synthetic_reference_expression(node.expression(), node.this_arg()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8796 (node *JSDocTypeLiteral) Clone
    fn clone_js_doc_type_literal(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_type_literal(&node.js_doc_property_tags(), node.is_array_type()),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast_generated.go:8842 (node *JSDocParameterOrPropertyTag) Clone
    fn clone_js_doc_parameter_or_property_tag(&self, node: Node) -> Node {
        clone_node_from(
            self.new_js_doc_parameter_or_property_tag(
                node.kind(),
                node.tag_name(),
                node.name(),
                node.is_bracketed(),
                node.type_expression(),
                node.is_name_first(),
                node.comment(),
            ),
            node,
            self.hooks(),
        )
    }

    // Go: ast/ast.go:2682 (node *SourceFile) Clone
    fn clone_source_file(&self, node: Node) -> Node {
        let updated =
            self.new_source_file_from(node, node.statement_list(), node.end_of_file_token());
        clone_node_from(updated, node, self.hooks())
    }
}

// Go: ast/deepclone.go:6 getDeepCloneVisitor
// Ideally, this would get cached on the node factory so there's only ever one set of closures made per factory
// PORT: Go sets `newList.Loc` in place. A synthetic list fixes its `Loc` when
// it is made, so the hooks make a new list with the same nodes and the new
// `Loc`. The returned list is the same as in Go.
fn get_deep_clone_visitor<'a>(f: &'a NodeFactory, synthetic_location: bool) -> NodeVisitor<'a, ()> {
    new_node_visitor(
        move |node: Node, visitor: &mut NodeVisitor<'a, ()>| {
            let visited = visitor.visit_each_child(node);
            if visited != node {
                if synthetic_location {
                    set_node_loc(visited, TextRange::new(-1, -1));
                }
                return visited;
            }
            let c = f.clone_node(node); // forcibly clone leaf nodes, which will then cascade new nodes/arrays upwards via `update` calls
            // In strada, `factory.cloneNode` was dynamic and did _not_ clone positions for any "special cases", meanwhile
            // Node.Clone in corsa reliably uses `Update` calls for all nodes and so copies locations by default.
            // Deep clones are done to copy a node across files, so here, we explicitly make the location range synthetic on all cloned nodes
            if synthetic_location {
                set_node_loc(c, TextRange::new(-1, -1));
            }
            c
        },
        Some(f),
        NodeVisitorHooks {
            visit_nodes: Some(Rc::new(
                move |nodes: NodeList, v: &mut NodeVisitor<'a, ()>| {
                    if nodes.is_nil() {
                        return NodeList::NIL;
                    }
                    let visited = v.visit_nodes(nodes);
                    let mut new_list = if visited != nodes {
                        visited
                    } else {
                        v.factory().clone_node_list(nodes)
                    };
                    if synthetic_location {
                        let new_nodes = new_list.nodes().to_vec();
                        new_list = f.new_synthetic_node_list(&new_nodes, TextRange::new(-1, -1));
                        if nodes.has_trailing_comma() {
                            set_node_loc(new_nodes[new_nodes.len() - 1], TextRange::new(-2, -2));
                        }
                    }
                    new_list
                },
            )),
            visit_modifiers: Some(Rc::new(
                move |nodes: ModifierList, v: &mut NodeVisitor<'a, ()>| {
                    if nodes.is_nil() {
                        return ModifierList::NIL;
                    }
                    let visited = v.visit_modifiers(nodes);
                    let mut new_list = if visited != nodes {
                        visited
                    } else {
                        v.factory().clone_modifier_list(nodes)
                    };
                    if synthetic_location {
                        let new_nodes = new_list.nodes().to_vec();
                        new_list =
                            f.new_synthetic_modifier_list(&new_nodes, TextRange::new(-1, -1));
                        if nodes.node_list().has_trailing_comma() {
                            set_node_loc(new_nodes[new_nodes.len() - 1], TextRange::new(-2, -2));
                        }
                    }
                    new_list
                },
            )),
            ..NodeVisitorHooks::default()
        },
        (),
    )
}

impl NodeFactory {
    // Go: ast/deepclone.go:71 DeepCloneNode
    #[must_use]
    pub fn deep_clone_node(&self, node: Node) -> Node {
        get_deep_clone_visitor(self, true /*syntheticLocation*/).visit_node(node)
    }

    // Go: ast/deepclone.go:75 DeepCloneReparse
    #[must_use]
    pub fn deep_clone_reparse(&self, node: Node) -> Node {
        let mut node = node;
        if node.is_some() {
            node = get_deep_clone_visitor(self, false /*syntheticLocation*/).visit_node(node);
            set_parent_in_children(node);
            set_node_flags(node, node.flags() | NodeFlags::REPARSED);
        }
        node
    }

    // Go: ast/deepclone.go:84 DeepCloneReparseModifiers
    #[must_use]
    pub fn deep_clone_reparse_modifiers(&self, modifiers: ModifierList) -> ModifierList {
        get_deep_clone_visitor(self, false /*syntheticLocation*/).visit_modifiers(modifiers)
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::synthetic::synthetic_slot_count;
    use crate::frontend::parser::{
        SourceFileParseOptions, parse_source_file, parse_source_file_detached,
    };
    use crate::prelude::*;

    /// JSDoc that the reparser deep clones with lists of nodes: `@import`,
    /// `@overload`, a function type in `@type`, an object type in `@typedef`.
    const TEXT: &str = "/** @import { a } from \"./b.js\" */\n\
        /** @overload @param {number} x @returns {void} */\n\
        /** @param {any} x */\nexport function o(x) {}\n\
        /** @type {(x: number) => void} */\nexport const fn = (x) => {};\n\
        /** @typedef {{x: number}} T */\n";

    fn opts() -> SourceFileParseOptions {
        SourceFileParseOptions {
            file_name: "/a.js".to_string(),
            ..Default::default()
        }
    }

    /// A parse worker's parse of `TEXT` makes no synthetic slot and no id,
    /// so the loader can adopt it (`files_parser.rs` `prefetch_parse`). Go
    /// makes those lists with the parser's factory (ast/visitor.go
    /// `VisitNodes`, `VisitModifiers`).
    #[test]
    fn reparsed_jsdoc_lists_stay_in_the_detached_store() {
        std::thread::spawn(|| {
            let before = (synthetic_slot_count(), crate::ast::utilities_p1::next_ids());
            let parse = parse_source_file_detached(0, &opts(), TEXT, ScriptKind::JS);
            assert!(parse.store.is_self_contained());
            assert_eq!(
                (synthetic_slot_count(), crate::ast::utilities_p1::next_ids()),
                before
            );
        })
        .join()
        .unwrap();
    }

    /// A language server parse of `TEXT` (a freeable parse, whose store owns
    /// its nodes) puts those lists in its store too, not in the thread's
    /// synthetic arena, so they are freed with the file version.
    #[test]
    fn reparsed_jsdoc_lists_stay_in_the_owned_store() {
        std::thread::spawn(|| {
            let before = synthetic_slot_count();
            let file = {
                let _scope = crate::ast::enter_owned_parse();
                parse_source_file(&opts(), TEXT, ScriptKind::JS)
            };
            assert!(crate::ast::try_store_ast_node(file.root).is_none(), "owned");
            assert_eq!(synthetic_slot_count(), before);
        })
        .join()
        .unwrap();
    }
}
