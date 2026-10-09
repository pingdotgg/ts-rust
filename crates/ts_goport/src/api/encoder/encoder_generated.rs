//! Port of Go `api/encoder/encoder_generated.go` (generated in Go by
//! `_scripts/generate-encoder.ts`). One arm per kind, in Go order.
//!
//! PORT: Go `n := node.AsX()` type assertions are the per-field `Node`
//! accessors, which panic on other kinds like the Go assertion. Go
//! `n.Field != nil` is `node.field().is_some()`; a Go `*NodeList` field uses
//! the `NodeList` accessor (`statement_list`, `parameter_list`, ...).

use crate::api::encoder::prelude::*;
use crate::frontend::parser::ParsedSourceFile;

// Go: api/encoder/encoder_generated.go:11 getNodeDataType
pub fn get_node_data_type(node: Node) -> u32 {
    node_data_type_of_kind(node.kind())
}

/// `get_node_data_type` of a node of kind `kind`.
// PERF: (apiperf2) the encoder reads the kind of a node once.
#[inline]
pub fn node_data_type_of_kind(kind: SyntaxKind) -> u32 {
    match kind {
        SyntaxKind::Identifier
        | SyntaxKind::PrivateIdentifier
        | SyntaxKind::JsxText
        | SyntaxKind::JsDocText
        | SyntaxKind::JsDocLink
        | SyntaxKind::JsDocLinkPlain
        | SyntaxKind::JsDocLinkCode => NODE_DATA_TYPE_STRING,
        SyntaxKind::StringLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::RegularExpressionLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::TemplateHead
        | SyntaxKind::TemplateMiddle
        | SyntaxKind::TemplateTail
        | SyntaxKind::SourceFile => NODE_DATA_TYPE_EXTENDED_DATA,
        _ => NODE_DATA_TYPE_CHILDREN,
    }
}

// Go: api/encoder/encoder_generated.go:36 getChildrenPropertyMask
pub fn get_children_property_mask(node: Node) -> u8 {
    match node.kind() {
        SyntaxKind::QualifiedName => {
            // n := node.AsQualifiedName()
            (bool_to_byte(node.left().is_some()) << 0) | (bool_to_byte(node.right().is_some()) << 1)
        }
        SyntaxKind::ComputedPropertyName => {
            // n := node.AsComputedPropertyName()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::Decorator => {
            // n := node.AsDecorator()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::IfStatement => {
            // n := node.AsIfStatement()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.then_statement().is_some()) << 1)
                | (bool_to_byte(node.else_statement().is_some()) << 2)
        }
        SyntaxKind::DoStatement => {
            // n := node.AsDoStatement()
            (bool_to_byte(node.statement().is_some()) << 0)
                | (bool_to_byte(node.expression().is_some()) << 1)
        }
        SyntaxKind::WhileStatement => {
            // n := node.AsWhileStatement()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.statement().is_some()) << 1)
        }
        SyntaxKind::ForStatement => {
            // n := node.AsForStatement()
            (bool_to_byte(node.initializer().is_some()) << 0)
                | (bool_to_byte(node.condition().is_some()) << 1)
                | (bool_to_byte(node.incrementor().is_some()) << 2)
                | (bool_to_byte(node.statement().is_some()) << 3)
        }
        SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
            // n := node.AsForInOrOfStatement()
            (bool_to_byte(node.await_modifier().is_some()) << 0)
                | (bool_to_byte(node.initializer().is_some()) << 1)
                | (bool_to_byte(node.expression().is_some()) << 2)
                | (bool_to_byte(node.statement().is_some()) << 3)
        }
        SyntaxKind::BreakStatement => {
            // n := node.AsBreakStatement()
            bool_to_byte(node.label().is_some()) << 0
        }
        SyntaxKind::ContinueStatement => {
            // n := node.AsContinueStatement()
            bool_to_byte(node.label().is_some()) << 0
        }
        SyntaxKind::ReturnStatement => {
            // n := node.AsReturnStatement()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::WithStatement => {
            // n := node.AsWithStatement()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.statement().is_some()) << 1)
        }
        SyntaxKind::SwitchStatement => {
            // n := node.AsSwitchStatement()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.case_block().is_some()) << 1)
        }
        SyntaxKind::CaseBlock => {
            // n := node.AsCaseBlock()
            bool_to_byte(node.clauses().is_some()) << 0
        }
        SyntaxKind::CaseClause | SyntaxKind::DefaultClause => {
            // n := node.AsCaseOrDefaultClause()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.statement_list().is_some()) << 1)
        }
        SyntaxKind::ThrowStatement => {
            // n := node.AsThrowStatement()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::TryStatement => {
            // n := node.AsTryStatement()
            (bool_to_byte(node.try_block().is_some()) << 0)
                | (bool_to_byte(node.catch_clause().is_some()) << 1)
                | (bool_to_byte(node.finally_block().is_some()) << 2)
        }
        SyntaxKind::CatchClause => {
            // n := node.AsCatchClause()
            (bool_to_byte(node.variable_declaration().is_some()) << 0)
                | (bool_to_byte(node.block().is_some()) << 1)
        }
        SyntaxKind::LabeledStatement => {
            // n := node.AsLabeledStatement()
            (bool_to_byte(node.label().is_some()) << 0)
                | (bool_to_byte(node.statement().is_some()) << 1)
        }
        SyntaxKind::ExpressionStatement => {
            // n := node.AsExpressionStatement()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::Block => {
            // n := node.AsBlock()
            bool_to_byte(node.statement_list().is_some()) << 0
        }
        SyntaxKind::VariableStatement => {
            // n := node.AsVariableStatement()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.declaration_list().is_some()) << 1)
        }
        SyntaxKind::VariableDeclaration => {
            // n := node.AsVariableDeclaration()
            (bool_to_byte(node.name().is_some()) << 0)
                | (bool_to_byte(node.exclamation_token().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
                | (bool_to_byte(node.initializer().is_some()) << 3)
        }
        SyntaxKind::VariableDeclarationList => {
            // n := node.AsVariableDeclarationList()
            bool_to_byte(node.declarations().is_some()) << 0
        }
        SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => {
            // n := node.AsBindingPattern()
            bool_to_byte(node.element_list().is_some()) << 0
        }
        SyntaxKind::Parameter => {
            // n := node.AsParameterDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.dot_dot_dot_token().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.question_token().is_some()) << 3)
                | (bool_to_byte(node.type_().is_some()) << 4)
                | (bool_to_byte(node.initializer().is_some()) << 5)
        }
        SyntaxKind::BindingElement => {
            // n := node.AsBindingElement()
            (bool_to_byte(node.dot_dot_dot_token().is_some()) << 0)
                | (bool_to_byte(node.property_name().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.initializer().is_some()) << 3)
        }
        SyntaxKind::MissingDeclaration => {
            // n := node.AsMissingDeclaration()
            bool_to_byte(has_modifiers(node.modifiers())) << 0
        }
        SyntaxKind::FunctionDeclaration => {
            // n := node.AsFunctionDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.asterisk_token().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 3)
                | (bool_to_byte(node.parameter_list().is_some()) << 4)
                | (bool_to_byte(node.type_().is_some()) << 5)
                | (bool_to_byte(node.body().is_some()) << 6)
        }
        SyntaxKind::ClassDeclaration => {
            // n := node.AsClassDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.heritage_clauses().is_some()) << 3)
                | (bool_to_byte(node.member_list().is_some()) << 4)
        }
        SyntaxKind::ClassExpression => {
            // n := node.AsClassExpression()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.heritage_clauses().is_some()) << 3)
                | (bool_to_byte(node.member_list().is_some()) << 4)
        }
        SyntaxKind::HeritageClause => {
            // n := node.AsHeritageClause()
            bool_to_byte(node.types().is_some()) << 0
        }
        SyntaxKind::InterfaceDeclaration => {
            // n := node.AsInterfaceDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.heritage_clauses().is_some()) << 3)
                | (bool_to_byte(node.member_list().is_some()) << 4)
        }
        SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => {
            // n := node.AsTypeAliasDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
        }
        SyntaxKind::EnumMember => {
            // n := node.AsEnumMember()
            (bool_to_byte(node.name().is_some()) << 0)
                | (bool_to_byte(node.initializer().is_some()) << 1)
        }
        SyntaxKind::EnumDeclaration => {
            // n := node.AsEnumDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.member_list().is_some()) << 2)
        }
        SyntaxKind::ModuleBlock => {
            // n := node.AsModuleBlock()
            bool_to_byte(node.statement_list().is_some()) << 0
        }
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            // n := node.AsImportDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.import_clause().is_some()) << 1)
                | (bool_to_byte(node.module_specifier().is_some()) << 2)
                | (bool_to_byte(node.attributes().is_some()) << 3)
        }
        SyntaxKind::ExternalModuleReference => {
            // n := node.AsExternalModuleReference()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::NamespaceImport => {
            // n := node.AsNamespaceImport()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::NamedImports => {
            // n := node.AsNamedImports()
            bool_to_byte(node.element_list().is_some()) << 0
        }
        SyntaxKind::ExportAssignment => {
            // n := node.AsExportAssignment()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.type_().is_some()) << 1)
                | (bool_to_byte(node.expression().is_some()) << 2)
        }
        SyntaxKind::NamespaceExportDeclaration => {
            // n := node.AsNamespaceExportDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
        }
        SyntaxKind::NamespaceExport => {
            // n := node.AsNamespaceExport()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::NamedExports => {
            // n := node.AsNamedExports()
            bool_to_byte(node.element_list().is_some()) << 0
        }
        SyntaxKind::ExportSpecifier => {
            // n := node.AsExportSpecifier()
            (bool_to_byte(node.property_name().is_some()) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
        }
        SyntaxKind::CallSignature => {
            // n := node.AsCallSignatureDeclaration()
            (bool_to_byte(node.type_parameter_list().is_some()) << 0)
                | (bool_to_byte(node.parameter_list().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::ConstructSignature => {
            // n := node.AsConstructSignatureDeclaration()
            (bool_to_byte(node.type_parameter_list().is_some()) << 0)
                | (bool_to_byte(node.parameter_list().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::Constructor => {
            // n := node.AsConstructorDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 1)
                | (bool_to_byte(node.parameter_list().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.body().is_some()) << 4)
        }
        SyntaxKind::GetAccessor => {
            // n := node.AsGetAccessorDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.parameter_list().is_some()) << 3)
                | (bool_to_byte(node.type_().is_some()) << 4)
                | (bool_to_byte(node.body().is_some()) << 5)
        }
        SyntaxKind::SetAccessor => {
            // n := node.AsSetAccessorDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.parameter_list().is_some()) << 3)
                | (bool_to_byte(node.type_().is_some()) << 4)
                | (bool_to_byte(node.body().is_some()) << 5)
        }
        SyntaxKind::IndexSignature => {
            // n := node.AsIndexSignatureDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.parameter_list().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::MethodSignature => {
            // n := node.AsMethodSignatureDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.postfix_token().is_some()) << 2)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 3)
                | (bool_to_byte(node.parameter_list().is_some()) << 4)
                | (bool_to_byte(node.type_().is_some()) << 5)
        }
        SyntaxKind::MethodDeclaration => {
            // n := node.AsMethodDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.asterisk_token().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.postfix_token().is_some()) << 3)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 4)
                | (bool_to_byte(node.parameter_list().is_some()) << 5)
                | (bool_to_byte(node.type_().is_some()) << 6)
                | (bool_to_byte(node.body().is_some()) << 7)
        }
        SyntaxKind::PropertySignature => {
            // n := node.AsPropertySignatureDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.postfix_token().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.initializer().is_some()) << 4)
        }
        SyntaxKind::PropertyDeclaration => {
            // n := node.AsPropertyDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.postfix_token().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.initializer().is_some()) << 4)
        }
        SyntaxKind::ClassStaticBlockDeclaration => {
            // n := node.AsClassStaticBlockDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.body().is_some()) << 1)
        }
        SyntaxKind::BinaryExpression => {
            // n := node.AsBinaryExpression()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.left().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
                | (bool_to_byte(node.operator_token().is_some()) << 3)
                | (bool_to_byte(node.right().is_some()) << 4)
        }
        SyntaxKind::PrefixUnaryExpression => {
            // n := node.AsPrefixUnaryExpression()
            bool_to_byte(node.operand().is_some()) << 0
        }
        SyntaxKind::PostfixUnaryExpression => {
            // n := node.AsPostfixUnaryExpression()
            bool_to_byte(node.operand().is_some()) << 0
        }
        SyntaxKind::YieldExpression => {
            // n := node.AsYieldExpression()
            (bool_to_byte(node.asterisk_token().is_some()) << 0)
                | (bool_to_byte(node.expression().is_some()) << 1)
        }
        SyntaxKind::ArrowFunction => {
            // n := node.AsArrowFunction()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 1)
                | (bool_to_byte(node.parameter_list().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.equals_greater_than_token().is_some()) << 4)
                | (bool_to_byte(node.body().is_some()) << 5)
        }
        SyntaxKind::FunctionExpression => {
            // n := node.AsFunctionExpression()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.asterisk_token().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 3)
                | (bool_to_byte(node.parameter_list().is_some()) << 4)
                | (bool_to_byte(node.type_().is_some()) << 5)
                | (bool_to_byte(node.body().is_some()) << 6)
        }
        SyntaxKind::AsExpression => {
            // n := node.AsAsExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.type_().is_some()) << 1)
        }
        SyntaxKind::SatisfiesExpression => {
            // n := node.AsSatisfiesExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.type_().is_some()) << 1)
        }
        SyntaxKind::ConditionalExpression => {
            // n := node.AsConditionalExpression()
            (bool_to_byte(node.condition().is_some()) << 0)
                | (bool_to_byte(node.question_token().is_some()) << 1)
                | (bool_to_byte(node.when_true().is_some()) << 2)
                | (bool_to_byte(node.colon_token().is_some()) << 3)
                | (bool_to_byte(node.when_false().is_some()) << 4)
        }
        SyntaxKind::PropertyAccessExpression => {
            // n := node.AsPropertyAccessExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.question_dot_token().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
        }
        SyntaxKind::ElementAccessExpression => {
            // n := node.AsElementAccessExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.question_dot_token().is_some()) << 1)
                | (bool_to_byte(node.argument_expression().is_some()) << 2)
        }
        SyntaxKind::CallExpression => {
            // n := node.AsCallExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.question_dot_token().is_some()) << 1)
                | (bool_to_byte(node.type_argument_list().is_some()) << 2)
                | (bool_to_byte(node.argument_list().is_some()) << 3)
        }
        SyntaxKind::NewExpression => {
            // n := node.AsNewExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
                | (bool_to_byte(node.argument_list().is_some()) << 2)
        }
        SyntaxKind::MetaProperty => {
            // n := node.AsMetaProperty()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::NonNullExpression => {
            // n := node.AsNonNullExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::SpreadElement => {
            // n := node.AsSpreadElement()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::TemplateExpression => {
            // n := node.AsTemplateExpression()
            (bool_to_byte(node.head().is_some()) << 0)
                | (bool_to_byte(node.template_spans().is_some()) << 1)
        }
        SyntaxKind::TemplateSpan => {
            // n := node.AsTemplateSpan()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.literal().is_some()) << 1)
        }
        SyntaxKind::TaggedTemplateExpression => {
            // n := node.AsTaggedTemplateExpression()
            (bool_to_byte(node.tag().is_some()) << 0)
                | (bool_to_byte(node.question_dot_token().is_some()) << 1)
                | (bool_to_byte(node.type_argument_list().is_some()) << 2)
                | (bool_to_byte(node.template().is_some()) << 3)
        }
        SyntaxKind::ParenthesizedExpression => {
            // n := node.AsParenthesizedExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::ArrayLiteralExpression => {
            // n := node.AsArrayLiteralExpression()
            bool_to_byte(node.element_list().is_some()) << 0
        }
        SyntaxKind::ObjectLiteralExpression => {
            // n := node.AsObjectLiteralExpression()
            bool_to_byte(node.property_list().is_some()) << 0
        }
        SyntaxKind::SpreadAssignment => {
            // n := node.AsSpreadAssignment()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::PropertyAssignment => {
            // n := node.AsPropertyAssignment()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.postfix_token().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.initializer().is_some()) << 4)
        }
        SyntaxKind::ShorthandPropertyAssignment => {
            // n := node.AsShorthandPropertyAssignment()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.postfix_token().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
                | (bool_to_byte(node.equals_token().is_some()) << 4)
                | (bool_to_byte(node.object_assignment_initializer().is_some()) << 5)
        }
        SyntaxKind::DeleteExpression => {
            // n := node.AsDeleteExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::TypeOfExpression => {
            // n := node.AsTypeOfExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::VoidExpression => {
            // n := node.AsVoidExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::AwaitExpression => {
            // n := node.AsAwaitExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::TypeAssertionExpression => {
            // n := node.AsTypeAssertion()
            (bool_to_byte(node.type_().is_some()) << 0)
                | (bool_to_byte(node.expression().is_some()) << 1)
        }
        SyntaxKind::UnionType => {
            // n := node.AsUnionTypeNode()
            bool_to_byte(node.types().is_some()) << 0
        }
        SyntaxKind::IntersectionType => {
            // n := node.AsIntersectionTypeNode()
            bool_to_byte(node.types().is_some()) << 0
        }
        SyntaxKind::ConditionalType => {
            // n := node.AsConditionalTypeNode()
            (bool_to_byte(node.check_type().is_some()) << 0)
                | (bool_to_byte(node.extends_type().is_some()) << 1)
                | (bool_to_byte(node.true_type().is_some()) << 2)
                | (bool_to_byte(node.false_type().is_some()) << 3)
        }
        SyntaxKind::TypeOperator => {
            // n := node.AsTypeOperatorNode()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::InferType => {
            // n := node.AsInferTypeNode()
            bool_to_byte(node.type_parameter().is_some()) << 0
        }
        SyntaxKind::ArrayType => {
            // n := node.AsArrayTypeNode()
            bool_to_byte(node.element_type().is_some()) << 0
        }
        SyntaxKind::IndexedAccessType => {
            // n := node.AsIndexedAccessTypeNode()
            (bool_to_byte(node.object_type().is_some()) << 0)
                | (bool_to_byte(node.index_type().is_some()) << 1)
        }
        SyntaxKind::TypeReference => {
            // n := node.AsTypeReferenceNode()
            (bool_to_byte(node.type_name().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
        }
        SyntaxKind::ExpressionWithTypeArguments => {
            // n := node.AsExpressionWithTypeArguments()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
        }
        SyntaxKind::LiteralType => {
            // n := node.AsLiteralTypeNode()
            bool_to_byte(node.literal().is_some()) << 0
        }
        SyntaxKind::TypePredicate => {
            // n := node.AsTypePredicateNode()
            (bool_to_byte(node.asserts_modifier().is_some()) << 0)
                | (bool_to_byte(node.parameter_name().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::ImportAttribute => {
            // n := node.AsImportAttribute()
            (bool_to_byte(node.name().is_some()) << 0) | (bool_to_byte(node.value().is_some()) << 1)
        }
        SyntaxKind::ImportAttributes => {
            // n := node.AsImportAttributes()
            bool_to_byte(node.attribute_list().is_some()) << 0
        }
        SyntaxKind::TypeQuery => {
            // n := node.AsTypeQueryNode()
            (bool_to_byte(node.expr_name().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
        }
        SyntaxKind::MappedType => {
            // n := node.AsMappedTypeNode()
            (bool_to_byte(node.readonly_token().is_some()) << 0)
                | (bool_to_byte(node.type_parameter().is_some()) << 1)
                | (bool_to_byte(node.name_type().is_some()) << 2)
                | (bool_to_byte(node.question_token().is_some()) << 3)
                | (bool_to_byte(node.type_().is_some()) << 4)
                | (bool_to_byte(node.member_list().is_some()) << 5)
        }
        SyntaxKind::TypeLiteral => {
            // n := node.AsTypeLiteralNode()
            bool_to_byte(node.member_list().is_some()) << 0
        }
        SyntaxKind::TupleType => {
            // n := node.AsTupleTypeNode()
            bool_to_byte(node.element_list().is_some()) << 0
        }
        SyntaxKind::NamedTupleMember => {
            // n := node.AsNamedTupleMember()
            (bool_to_byte(node.dot_dot_dot_token().is_some()) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.question_token().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
        }
        SyntaxKind::OptionalType => {
            // n := node.AsOptionalTypeNode()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::RestType => {
            // n := node.AsRestTypeNode()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::ParenthesizedType => {
            // n := node.AsParenthesizedTypeNode()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::FunctionType => {
            // n := node.AsFunctionTypeNode()
            (bool_to_byte(node.type_parameter_list().is_some()) << 0)
                | (bool_to_byte(node.parameter_list().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::ConstructorType => {
            // n := node.AsConstructorTypeNode()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 1)
                | (bool_to_byte(node.parameter_list().is_some()) << 2)
                | (bool_to_byte(node.type_().is_some()) << 3)
        }
        SyntaxKind::TemplateLiteralType => {
            // n := node.AsTemplateLiteralTypeNode()
            (bool_to_byte(node.head().is_some()) << 0)
                | (bool_to_byte(node.template_spans().is_some()) << 1)
        }
        SyntaxKind::TemplateLiteralTypeSpan => {
            // n := node.AsTemplateLiteralTypeSpan()
            (bool_to_byte(node.type_().is_some()) << 0)
                | (bool_to_byte(node.literal().is_some()) << 1)
        }
        SyntaxKind::SyntheticExpression => {
            // n := node.AsSyntheticExpression()
            bool_to_byte(node.tuple_name_source().is_some()) << 0
        }
        SyntaxKind::PartiallyEmittedExpression => {
            // n := node.AsPartiallyEmittedExpression()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::JsxElement => {
            // n := node.AsJsxElement()
            (bool_to_byte(node.opening_element().is_some()) << 0)
                | (bool_to_byte(node.children().is_some()) << 1)
                | (bool_to_byte(node.closing_element().is_some()) << 2)
        }
        SyntaxKind::JsxAttributes => {
            // n := node.AsJsxAttributes()
            bool_to_byte(node.property_list().is_some()) << 0
        }
        SyntaxKind::JsxNamespacedName => {
            // n := node.AsJsxNamespacedName()
            (bool_to_byte(node.namespace().is_some()) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
        }
        SyntaxKind::JsxOpeningElement => {
            // n := node.AsJsxOpeningElement()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
                | (bool_to_byte(node.attributes().is_some()) << 2)
        }
        SyntaxKind::JsxSelfClosingElement => {
            // n := node.AsJsxSelfClosingElement()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_argument_list().is_some()) << 1)
                | (bool_to_byte(node.attributes().is_some()) << 2)
        }
        SyntaxKind::JsxFragment => {
            // n := node.AsJsxFragment()
            (bool_to_byte(node.opening_fragment().is_some()) << 0)
                | (bool_to_byte(node.children().is_some()) << 1)
                | (bool_to_byte(node.closing_fragment().is_some()) << 2)
        }
        SyntaxKind::JsxAttribute => {
            // n := node.AsJsxAttribute()
            (bool_to_byte(node.name().is_some()) << 0)
                | (bool_to_byte(node.initializer().is_some()) << 1)
        }
        SyntaxKind::JsxSpreadAttribute => {
            // n := node.AsJsxSpreadAttribute()
            bool_to_byte(node.expression().is_some()) << 0
        }
        SyntaxKind::JsxClosingElement => {
            // n := node.AsJsxClosingElement()
            bool_to_byte(node.tag_name().is_some()) << 0
        }
        SyntaxKind::JsxExpression => {
            // n := node.AsJsxExpression()
            (bool_to_byte(node.dot_dot_dot_token().is_some()) << 0)
                | (bool_to_byte(node.expression().is_some()) << 1)
        }
        SyntaxKind::SyntaxList => {
            // n := node.AsSyntaxList()
            bool_to_byte(!crate::ast::visitor::syntax_list_children(node).is_empty()) << 0
        }
        SyntaxKind::JsDoc => {
            // n := node.AsJSDoc()
            (bool_to_byte(node.comment().is_some()) << 0)
                | (bool_to_byte(node.tags().is_some()) << 1)
        }
        SyntaxKind::JsDocTypeExpression => {
            // n := node.AsJSDocTypeExpression()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::JsDocNonNullableType => {
            // n := node.AsJSDocNonNullableType()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::JsDocNullableType => {
            // n := node.AsJSDocNullableType()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::JsDocVariadicType => {
            // n := node.AsJSDocVariadicType()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::JsDocOptionalType => {
            // n := node.AsJSDocOptionalType()
            bool_to_byte(node.type_().is_some()) << 0
        }
        SyntaxKind::JsDocTypeTag => {
            // n := node.AsJSDocTypeTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocUnknownTag => {
            // n := node.AsJSDocUnknownTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocTemplateTag => {
            // n := node.AsJSDocTemplateTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.constraint().is_some()) << 1)
                | (bool_to_byte(node.type_parameter_list().is_some()) << 2)
                | (bool_to_byte(node.comment().is_some()) << 3)
        }
        SyntaxKind::JsDocReturnTag => {
            // n := node.AsJSDocReturnTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocPublicTag => {
            // n := node.AsJSDocPublicTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocPrivateTag => {
            // n := node.AsJSDocPrivateTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocProtectedTag => {
            // n := node.AsJSDocProtectedTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocReadonlyTag => {
            // n := node.AsJSDocReadonlyTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocOverrideTag => {
            // n := node.AsJSDocOverrideTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocDeprecatedTag => {
            // n := node.AsJSDocDeprecatedTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.comment().is_some()) << 1)
        }
        SyntaxKind::JsDocSeeTag => {
            // n := node.AsJSDocSeeTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.name_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocImplementsTag => {
            // n := node.AsJSDocImplementsTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.class_name().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocAugmentsTag => {
            // n := node.AsJSDocAugmentsTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.class_name().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocSatisfiesTag => {
            // n := node.AsJSDocSatisfiesTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocThrowsTag => {
            // n := node.AsJSDocThrowsTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocThisTag => {
            // n := node.AsJSDocThisTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocImportTag => {
            // n := node.AsJSDocImportTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.import_clause().is_some()) << 1)
                | (bool_to_byte(node.module_specifier().is_some()) << 2)
                | (bool_to_byte(node.attributes().is_some()) << 3)
                | (bool_to_byte(node.comment().is_some()) << 4)
        }
        SyntaxKind::JsDocCallbackTag => {
            // n := node.AsJSDocCallbackTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.comment().is_some()) << 3)
        }
        SyntaxKind::JsDocOverloadTag => {
            // n := node.AsJSDocOverloadTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.comment().is_some()) << 2)
        }
        SyntaxKind::JsDocTypedefTag => {
            // n := node.AsJSDocTypedefTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.type_expression().is_some()) << 1)
                | (bool_to_byte(node.name().is_some()) << 2)
                | (bool_to_byte(node.comment().is_some()) << 3)
        }
        SyntaxKind::JsDocSignature => {
            // n := node.AsJSDocSignature()
            (bool_to_byte(node.type_parameter_list().is_some()) << 0)
                | (bool_to_byte(node.parameter_list().is_some()) << 1)
                | (bool_to_byte(node.type_().is_some()) << 2)
        }
        SyntaxKind::JsDocNameReference => {
            // n := node.AsJSDocNameReference()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::ModuleDeclaration => {
            // n := node.AsModuleDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.attributes().is_some()) << 2)
                | (bool_to_byte(node.body().is_some()) << 3)
        }
        SyntaxKind::ImportEqualsDeclaration => {
            // n := node.AsImportEqualsDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.module_reference().is_some()) << 2)
        }
        SyntaxKind::ExportDeclaration => {
            // n := node.AsExportDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.export_clause().is_some()) << 1)
                | (bool_to_byte(node.module_specifier().is_some()) << 2)
                | (bool_to_byte(node.attributes().is_some()) << 3)
        }
        SyntaxKind::ImportType => {
            // n := node.AsImportTypeNode()
            (bool_to_byte(node.argument().is_some()) << 0)
                | (bool_to_byte(node.attributes().is_some()) << 1)
                | (bool_to_byte(node.qualifier().is_some()) << 2)
                | (bool_to_byte(node.type_argument_list().is_some()) << 3)
        }
        SyntaxKind::ImportClause => {
            // n := node.AsImportClause()
            (bool_to_byte(node.name().is_some()) << 0)
                | (bool_to_byte(node.named_bindings().is_some()) << 1)
        }
        SyntaxKind::ImportSpecifier => {
            // n := node.AsImportSpecifier()
            (bool_to_byte(node.property_name().is_some()) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
        }
        SyntaxKind::JsDocLink => {
            // n := node.AsJSDocLink()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::JsDocLinkPlain => {
            // n := node.AsJSDocLinkPlain()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::JsDocLinkCode => {
            // n := node.AsJSDocLinkCode()
            bool_to_byte(node.name().is_some()) << 0
        }
        SyntaxKind::TypeParameter => {
            // n := node.AsTypeParameterDeclaration()
            (bool_to_byte(has_modifiers(node.modifiers())) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.constraint().is_some()) << 2)
                | (bool_to_byte(node.expression().is_some()) << 3)
                | (bool_to_byte(node.default_type().is_some()) << 4)
        }
        SyntaxKind::SyntheticReferenceExpression => {
            // n := node.AsSyntheticReferenceExpression()
            (bool_to_byte(node.expression().is_some()) << 0)
                | (bool_to_byte(node.this_arg().is_some()) << 1)
        }
        SyntaxKind::JsDocTypeLiteral => {
            // n := node.AsJSDocTypeLiteral()
            bool_to_byte(!node.js_doc_property_tags().is_empty()) << 0
        }
        SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag => {
            // n := node.AsJSDocParameterOrPropertyTag()
            (bool_to_byte(node.tag_name().is_some()) << 0)
                | (bool_to_byte(node.name().is_some()) << 1)
                | (bool_to_byte(node.type_expression().is_some()) << 2)
                | (bool_to_byte(node.comment().is_some()) << 3)
        }
        _ => 0,
    }
}

// Go: api/encoder/encoder_generated.go:541 getNodeCommonData
pub fn get_node_common_data(node: Node) -> u32 {
    node_common_data_of_kind(node, node.kind())
}

/// `get_node_common_data` of `node`, of kind `kind`.
// PERF: (apiperf2) the encoder reads the kind of a node once. Inline: most
// kinds give 0 with no node read.
#[inline]
pub fn node_common_data_of_kind(node: Node, kind: SyntaxKind) -> u32 {
    match kind {
        SyntaxKind::Block => {
            // n := node.AsBlock()
            return (bool_to_byte(node.multi_line()) as u32) << 24;
        }
        SyntaxKind::HeritageClause => {
            // n := node.AsHeritageClause()
            let mut token_idx: u32 = 0;
            match node.token() {
                SyntaxKind::ImplementsKeyword => token_idx = 1,
                _ => {}
            }
            return token_idx << 24;
        }
        SyntaxKind::ExportAssignment => {
            // n := node.AsExportAssignment()
            return (bool_to_byte(node.is_export_equals()) as u32) << 24;
        }
        SyntaxKind::ExportSpecifier => {
            // n := node.AsExportSpecifier()
            return (bool_to_byte(node.is_type_only()) as u32) << 24;
        }
        SyntaxKind::PrefixUnaryExpression => {
            // n := node.AsPrefixUnaryExpression()
            let mut operator_idx: u32 = 0;
            match node.operator() {
                SyntaxKind::MinusToken => operator_idx = 1,
                SyntaxKind::TildeToken => operator_idx = 2,
                SyntaxKind::ExclamationToken => operator_idx = 3,
                SyntaxKind::PlusPlusToken => operator_idx = 4,
                SyntaxKind::MinusMinusToken => operator_idx = 5,
                _ => {}
            }
            return operator_idx << 24;
        }
        SyntaxKind::PostfixUnaryExpression => {
            // n := node.AsPostfixUnaryExpression()
            let mut operator_idx: u32 = 0;
            match node.operator() {
                SyntaxKind::MinusMinusToken => operator_idx = 1,
                _ => {}
            }
            return operator_idx << 24;
        }
        SyntaxKind::MetaProperty => {
            // n := node.AsMetaProperty()
            let mut keyword_token_idx: u32 = 0;
            match node.keyword_token() {
                SyntaxKind::NewKeyword => keyword_token_idx = 1,
                _ => {}
            }
            return keyword_token_idx << 24;
        }
        SyntaxKind::ArrayLiteralExpression => {
            // n := node.AsArrayLiteralExpression()
            return (bool_to_byte(node.multi_line()) as u32) << 24;
        }
        SyntaxKind::ObjectLiteralExpression => {
            // n := node.AsObjectLiteralExpression()
            return (bool_to_byte(node.multi_line()) as u32) << 24;
        }
        SyntaxKind::TypeOperator => {
            // n := node.AsTypeOperatorNode()
            let mut operator_idx: u32 = 0;
            match node.operator() {
                SyntaxKind::ReadonlyKeyword => operator_idx = 1,
                SyntaxKind::UniqueKeyword => operator_idx = 2,
                _ => {}
            }
            return operator_idx << 24;
        }
        SyntaxKind::ImportAttributes => {
            // n := node.AsImportAttributes()
            let mut token_idx: u32 = 0;
            match node.token() {
                SyntaxKind::AssertKeyword => token_idx = 1,
                _ => {}
            }
            return (bool_to_byte(node.multi_line()) as u32) << 24 | token_idx << 25;
        }
        SyntaxKind::SyntheticExpression => {
            return get_node_common_data_synthetic_expression(node);
        }
        SyntaxKind::JsxText => {
            // n := node.AsJsxText()
            return (bool_to_byte(node.contains_only_trivia_white_spaces()) as u32) << 24;
        }
        SyntaxKind::ModuleDeclaration => {
            // n := node.AsModuleDeclaration()
            let mut keyword_idx: u32 = 0;
            match node.keyword() {
                SyntaxKind::NamespaceKeyword => keyword_idx = 1,
                _ => {}
            }
            return keyword_idx << 24;
        }
        SyntaxKind::ImportEqualsDeclaration => {
            // n := node.AsImportEqualsDeclaration()
            return (bool_to_byte(node.is_type_only()) as u32) << 24;
        }
        SyntaxKind::ExportDeclaration => {
            // n := node.AsExportDeclaration()
            return (bool_to_byte(node.is_type_only()) as u32) << 24;
        }
        SyntaxKind::ImportType => {
            // n := node.AsImportTypeNode()
            return (bool_to_byte(node.is_type_of()) as u32) << 24;
        }
        SyntaxKind::ImportClause => {
            // n := node.AsImportClause()
            let mut phase_modifier_idx: u32 = 0;
            match node.phase_modifier() {
                SyntaxKind::TypeKeyword => phase_modifier_idx = 1,
                SyntaxKind::DeferKeyword => phase_modifier_idx = 2,
                // ts#63915
                SyntaxKind::SourceKeyword => phase_modifier_idx = 3,
                _ => {}
            }
            return phase_modifier_idx << 24;
        }
        SyntaxKind::ImportSpecifier => {
            // n := node.AsImportSpecifier()
            return (bool_to_byte(node.is_type_only()) as u32) << 24;
        }
        SyntaxKind::JsDocTypeLiteral => {
            // n := node.AsJSDocTypeLiteral()
            return (bool_to_byte(node.is_array_type()) as u32) << 24;
        }
        SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag => {
            // n := node.AsJSDocParameterOrPropertyTag()
            return (bool_to_byte(node.is_bracketed()) as u32) << 24
                | (bool_to_byte(node.is_name_first()) as u32) << 25;
        }
        _ => {}
    }
    0
}

// Go: api/encoder/encoder_generated.go:661 recordNodeStrings
pub fn record_node_strings(node: Node, strs: &mut StringTable) -> u32 {
    match node.kind() {
        // node.AsIdentifier().Text
        SyntaxKind::Identifier => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsPrivateIdentifier().Text
        SyntaxKind::PrivateIdentifier => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsJsxText().Text
        SyntaxKind::JsxText => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsJSDocText().Text()
        SyntaxKind::JsDocText => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsJSDocLink().Text()
        SyntaxKind::JsDocLink => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsJSDocLinkPlain().Text()
        SyntaxKind::JsDocLinkPlain => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        // node.AsJSDocLinkCode().Text()
        SyntaxKind::JsDocLinkCode => strs.add(node.text(), node.kind(), node.pos(), node.end()),
        _ => panic!(
            "Unexpected node kind {}",
            go_kind_string(node.kind() as i16)
        ),
    }
}

// Go: api/encoder/encoder_generated.go:682 recordExtendedData
// PORT: `given` goes to the SourceFile arm (see `get_node_data`).
pub fn record_extended_data(
    node: Node,
    strs: &mut StringTable,
    position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    structured_data: &mut Vec<u8>,
    given: Option<&Rc<ParsedSourceFile>>,
) -> u32 {
    let offset = extended_data.len() as u32;
    match node.kind() {
        SyntaxKind::StringLiteral => record_extended_data_string_literal(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::NumericLiteral => record_extended_data_numeric_literal(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::BigIntLiteral => record_extended_data_big_int_literal(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::RegularExpressionLiteral => record_extended_data_regular_expression_literal(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::NoSubstitutionTemplateLiteral => {
            record_extended_data_no_substitution_template_literal(
                node,
                strs,
                position_map,
                extended_data,
                structured_data,
            )
        }
        SyntaxKind::TemplateHead => record_extended_data_template_head(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::TemplateMiddle => record_extended_data_template_middle(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::TemplateTail => record_extended_data_template_tail(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
        ),
        SyntaxKind::SourceFile => record_extended_data_source_file(
            node,
            strs,
            position_map,
            extended_data,
            structured_data,
            given,
        ),
        _ => panic!(
            "unknown extended data node kind {}",
            go_kind_string(node.kind() as i16)
        ),
    }
    offset
}
